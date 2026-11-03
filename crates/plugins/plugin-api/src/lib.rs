//! Stable contracts shared by native RavaLink plugins.
//!
//! Plugins are ordinary Rust components in the workspace. The API intentionally
//! contains no JVM, class-loader, or Lavaplayer compatibility layer.

use ravalink_protocol::PluginInfo;
use std::{fmt, sync::Arc};

/// Public metadata exposed through Lavalink's /v4/info endpoint.
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

/// Common interface implemented by every built-in or third-party Rust plugin.
pub trait RavaLinkPlugin: Send + Sync {
    fn descriptor(&self) -> PluginDescriptor;

    /// Whether this plugin recognizes the supplied URL or Lavalink search prefix.
    fn supports_identifier(&self, identifier: &str) -> bool;
}

/// An owned plugin instance. The registry keeps plugins alive for the node lifetime.
pub type SharedPlugin = Arc<dyn RavaLinkPlugin>;

/// Registry for native plugins. Registration order is preserved and therefore
/// determines precedence if two plugins claim the same identifier.
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
        self.plugins.iter().map(|plugin| plugin.descriptor()).collect()
    }

    pub fn infos(&self) -> Vec<PluginInfo> {
        self.descriptors().iter().map(PluginDescriptor::info).collect()
    }

    pub fn find(&self, identifier: &str) -> Option<SharedPlugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.supports_identifier(identifier))
            .cloned()
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
    }

    #[test]
    fn registry_preserves_registration_order_and_routes_identifiers() {
        let mut registry = PluginRegistry::default();
        registry.register(TestPlugin);
        assert_eq!(registry.len(), 1);
        assert!(registry.find("test:hello").is_some());
        assert!(registry.find("https://example.com").is_none());
        assert_eq!(registry.infos()[0].name, "test");
    }
}
