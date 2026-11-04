//! Stable contracts shared by native RavaLink plugins.
//!
//! Plugins are ordinary Rust components in the workspace. The API intentionally
//! contains no JVM, class-loader, or Lavaplayer compatibility layer.

use async_trait::async_trait;
use ravalink_protocol::PluginInfo;
use serde_json::Value;
use std::{fmt, sync::Arc, time::Duration};

/// Maximum default wall-clock time allowed for one plugin resolution.
pub const DEFAULT_PLUGIN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginLoadType {
    Track,
    Playlist,
    Search,
}

impl PluginLoadType {
    pub const fn as_lavalink_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Playlist => "playlist",
            Self::Search => "search",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginLoadResult {
    pub load_type: PluginLoadType,
    pub data: Value,
}

impl PluginLoadResult {
    pub fn new(load_type: PluginLoadType, data: Value) -> Self {
        Self { load_type, data }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDescriptor {
    pub name: &'static str,
    pub version: &'static str,
    pub description: &'static str,
    pub capabilities: &'static [&'static str],
}

impl PluginDescriptor {
    pub fn info(&self) -> PluginInfo {
        PluginInfo {
            name: self.name.to_owned(),
            version: self.version.to_owned(),
        }
    }
}

#[async_trait]
pub trait RavaLinkPlugin: Send + Sync {
    fn descriptor(&self) -> PluginDescriptor;

    fn supports_identifier(&self, identifier: &str) -> bool;

    /// Resolve a URL or search expression into Lavalink-compatible load data.
    /// Returning None lets the next registered plugin try the identifier.
    async fn load_tracks(
        &self,
        _identifier: &str,
    ) -> Result<Option<PluginLoadResult>, PluginError> {
        Ok(None)
    }
}

pub type SharedPlugin = Arc<dyn RavaLinkPlugin>;

#[derive(Clone, Default)]
pub struct PluginRegistry {
    plugins: Vec<SharedPlugin>,
}

impl PluginRegistry {
    pub fn register<P>(&mut self, plugin: P)
    where
        P: RavaLinkPlugin + 'static,
    {
        self.plugins.push(Arc::new(plugin));
    }

    pub fn register_shared(&mut self, plugin: SharedPlugin) {
        self.plugins.push(plugin);
    }

    pub fn descriptors(&self) -> Vec<PluginDescriptor> {
        self.plugins
            .iter()
            .map(|plugin| plugin.descriptor())
            .collect()
    }

    pub fn infos(&self) -> Vec<PluginInfo> {
        self.descriptors()
            .iter()
            .map(PluginDescriptor::info)
            .collect()
    }

    pub fn find(&self, identifier: &str) -> Option<SharedPlugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.supports_identifier(identifier))
            .cloned()
    }

    /// Resolve an identifier while bounding each plugin's execution time.
    ///
    /// A plugin timeout is isolated to that plugin call: the timed-out future is
    /// cancelled and the caller receives a structured error rather than waiting
    /// indefinitely on a third-party source manager.
    pub async fn load_tracks(
        &self,
        identifier: &str,
    ) -> Result<Option<PluginLoadResult>, PluginError> {
        self.load_tracks_with_timeout(identifier, DEFAULT_PLUGIN_TIMEOUT)
            .await
    }

    /// Resolve an identifier with an explicit per-plugin wall-clock timeout.
    pub async fn load_tracks_with_timeout(
        &self,
        identifier: &str,
        timeout: Duration,
    ) -> Result<Option<PluginLoadResult>, PluginError> {
        for plugin in &self.plugins {
            if !plugin.supports_identifier(identifier) {
                continue;
            }
            let descriptor = plugin.descriptor();
            let result = tokio::time::timeout(timeout, plugin.load_tracks(identifier))
                .await
                .map_err(|_| {
                    PluginError::new(
                        descriptor.name,
                        format!(
                            "track resolution timed out after {} ms",
                            timeout.as_millis()
                        ),
                    )
                })??;
            if result.is_some() {
                return Ok(result);
            }
        }
        Ok(None)
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError {
    pub plugin: String,
    pub message: String,
}

impl PluginError {
    pub fn new(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.plugin, self.message)
    }
}

impl std::error::Error for PluginError {}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPlugin;

    #[async_trait]
    impl RavaLinkPlugin for TestPlugin {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                name: "test",
                version: "0.1.0",
                description: "test plugin",
                capabilities: &["search"],
            }
        }

        fn supports_identifier(&self, identifier: &str) -> bool {
            identifier.starts_with("test:")
        }

        async fn load_tracks(
            &self,
            identifier: &str,
        ) -> Result<Option<PluginLoadResult>, PluginError> {
            Ok(Some(PluginLoadResult::new(
                PluginLoadType::Search,
                serde_json::json!({"query": identifier}),
            )))
        }
    }

    struct SlowPlugin;

    #[async_trait]
    impl RavaLinkPlugin for SlowPlugin {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                name: "slow",
                version: "0.1.0",
                description: "deliberately slow test plugin",
                capabilities: &["search"],
            }
        }

        fn supports_identifier(&self, identifier: &str) -> bool {
            identifier.starts_with("slow:")
        }

        async fn load_tracks(
            &self,
            _identifier: &str,
        ) -> Result<Option<PluginLoadResult>, PluginError> {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(None)
        }
    }

    #[tokio::test]
    async fn registry_cancels_plugin_resolution_after_its_timeout() {
        let mut registry = PluginRegistry::default();
        registry.register(SlowPlugin);

        let error = registry
            .load_tracks_with_timeout("slow:track", Duration::from_millis(1))
            .await
            .unwrap_err();

        assert_eq!(error.plugin, "slow");
        assert!(error.message.contains("timed out"));
    }

    #[tokio::test]
    async fn registry_routes_and_loads_identifiers() {
        let mut registry = PluginRegistry::default();
        registry.register(TestPlugin);
        assert_eq!(registry.len(), 1);
        assert!(registry.find("test:hello").is_some());
        assert!(registry.find("https://example.com").is_none());
        assert_eq!(registry.infos()[0].name, "test");
        let loaded = registry.load_tracks("test:hello").await.unwrap().unwrap();
        assert_eq!(loaded.load_type, PluginLoadType::Search);
        assert_eq!(loaded.data["query"], "test:hello");
    }
}
