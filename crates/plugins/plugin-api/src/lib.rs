//! Stable contracts shared by native RavaLink plugins.
//!
//! Plugins are ordinary Rust components in the workspace. The API intentionally
//! contains no JVM, class-loader, or Lavaplayer compatibility layer.

use async_trait::async_trait;
use ravalink_protocol::PluginInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, fmt, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Maximum default wall-clock time allowed for one plugin resolution.
pub const DEFAULT_PLUGIN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    pub name: String,
    pub version: String,
    pub description: String,
    pub capabilities: Vec<String>,
}

impl PluginDescriptor {
    pub fn info(&self) -> PluginInfo {
        PluginInfo {
            name: self.name.clone(),
            version: self.version.clone(),
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

/// Maximum JSON response accepted from an out-of-process extension.
pub const MAX_PROCESS_PLUGIN_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum concurrent child-process invocations for each external extension.
pub const MAX_CONCURRENT_PROCESS_PLUGIN_CALLS: usize = 2;

/// A standalone Rust extension hosted as a short-lived child process.
#[derive(Debug, Clone)]
pub struct ProcessPlugin {
    descriptor: PluginDescriptor,
    identifier_prefix: String,
    program: PathBuf,
    args: Vec<String>,
    environment: HashMap<String, String>,
    config: Value,
    process_slots: Arc<tokio::sync::Semaphore>,
}

impl ProcessPlugin {
    pub fn new(
        descriptor: PluginDescriptor,
        identifier_prefix: impl Into<String>,
        program: impl Into<PathBuf>,
        args: Vec<String>,
        environment: HashMap<String, String>,
    ) -> Self {
        Self {
            descriptor,
            identifier_prefix: identifier_prefix.into(),
            program: program.into(),
            args,
            environment,
            config: Value::Object(Default::default()),
            process_slots: Arc::new(tokio::sync::Semaphore::new(
                MAX_CONCURRENT_PROCESS_PLUGIN_CALLS,
            )),
        }
    }

    pub fn with_config(mut self, config: Value) -> Self {
        self.config = config;
        self
    }
}

#[async_trait]
impl RavaLinkPlugin for ProcessPlugin {
    fn descriptor(&self) -> PluginDescriptor {
        self.descriptor.clone()
    }

    fn supports_identifier(&self, identifier: &str) -> bool {
        identifier.starts_with(&self.identifier_prefix)
    }

    async fn load_tracks(&self, identifier: &str) -> Result<Option<PluginLoadResult>, PluginError> {
        let _permit = self
            .process_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| {
                PluginError::new(
                    self.descriptor.name.clone(),
                    "extension process runner is shutting down",
                )
            })?;
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .env_clear()
            .envs(&self.environment)
            .env("RAVALINK_PLUGIN_PROTOCOL", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                PluginError::new(
                    self.descriptor.name.clone(),
                    format!("cannot start extension process: {error}"),
                )
            })?;

        let request = serde_json::json!({
            "protocolVersion": 1,
            "identifier": identifier,
            "config": self.config,
        });
        let mut stdin = child.stdin.take().ok_or_else(|| {
            PluginError::new(
                self.descriptor.name.clone(),
                "extension stdin was unavailable",
            )
        })?;
        stdin
            .write_all(request.to_string().as_bytes())
            .await
            .map_err(|error| {
                PluginError::new(
                    self.descriptor.name.clone(),
                    format!("cannot write extension request: {error}"),
                )
            })?;
        drop(stdin);

        let stdout = child.stdout.take().ok_or_else(|| {
            PluginError::new(
                self.descriptor.name.clone(),
                "extension stdout was unavailable",
            )
        })?;
        let mut output = Vec::new();
        stdout
            .take((MAX_PROCESS_PLUGIN_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|error| {
                PluginError::new(
                    self.descriptor.name.clone(),
                    format!("cannot read extension response: {error}"),
                )
            })?;
        if output.len() > MAX_PROCESS_PLUGIN_OUTPUT_BYTES {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(PluginError::new(
                self.descriptor.name.clone(),
                format!(
                    "extension response exceeds the {} byte limit",
                    MAX_PROCESS_PLUGIN_OUTPUT_BYTES
                ),
            ));
        }

        let status = child.wait().await.map_err(|error| {
            PluginError::new(
                self.descriptor.name.clone(),
                format!("cannot wait for extension process: {error}"),
            )
        })?;
        if !status.success() {
            return Err(PluginError::new(
                self.descriptor.name.clone(),
                format!("extension process exited with status {status}"),
            ));
        }
        serde_json::from_slice::<Option<PluginLoadResult>>(&output).map_err(|error| {
            PluginError::new(
                self.descriptor.name.clone(),
                format!("extension returned invalid JSON: {error}"),
            )
        })
    }
}

/// Manifest for a community plugin installed in the plugins directory.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    #[serde(default = "default_plugin_api_version")]
    pub api_version: u32,
    pub name: String,
    pub version: String,
    pub description: String,
    pub identifier_prefix: String,
    pub program: PathBuf,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub config: Value,
}

fn default_plugin_api_version() -> u32 {
    1
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

    /// Discover community extensions from one directory level of plugin folders.
    ///
    /// Each plugin folder contains a `ravalink-plugin.toml` manifest and an
    /// executable. Relative executable paths are resolved against that folder.
    pub fn load_directory(&mut self, directory: &std::path::Path) -> Result<usize, PluginError> {
        if !directory.exists() {
            return Ok(0);
        }
        let entries = std::fs::read_dir(directory).map_err(|error| {
            PluginError::new(
                "plugin-loader",
                format!("cannot read plugin directory: {error}"),
            )
        })?;
        let mut folders = entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                PluginError::new(
                    "plugin-loader",
                    format!("cannot enumerate plugins: {error}"),
                )
            })?;
        folders.sort();
        let mut loaded = 0;
        for folder in folders {
            if !folder.is_dir() {
                continue;
            }
            let manifest_path = folder.join("ravalink-plugin.toml");
            if !manifest_path.is_file() {
                continue;
            }
            let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|error| {
                PluginError::new(
                    folder.display().to_string(),
                    format!("cannot read plugin manifest: {error}"),
                )
            })?;
            let manifest: PluginManifest = toml::from_str(&manifest_text).map_err(|error| {
                PluginError::new(
                    folder.display().to_string(),
                    format!("invalid plugin manifest: {error}"),
                )
            })?;
            let name = manifest.name.trim();
            let prefix = manifest.identifier_prefix.trim();
            if manifest.api_version != 1 {
                return Err(PluginError::new(
                    name,
                    format!("unsupported plugin API version {}", manifest.api_version),
                ));
            }
            if name.is_empty()
                || manifest.version.trim().is_empty()
                || manifest.description.trim().is_empty()
                || prefix.is_empty()
                || manifest
                    .capabilities
                    .iter()
                    .any(|item| item.trim().is_empty())
            {
                return Err(PluginError::new(
                    name,
                    "plugin manifest contains empty required metadata",
                ));
            }
            if self.plugins.iter().any(|plugin| {
                let descriptor = plugin.descriptor();
                descriptor.name.eq_ignore_ascii_case(name)
            }) {
                return Err(PluginError::new(name, "plugin name is already registered"));
            }
            let prefix_probe = format!("{prefix}ravalink-plugin-prefix-probe");
            if self.plugins.iter().any(|plugin| {
                plugin.supports_identifier(prefix) || plugin.supports_identifier(&prefix_probe)
            }) {
                return Err(PluginError::new(
                    name,
                    "plugin identifier prefix conflicts with an already registered source",
                ));
            }
            let program = if manifest.program.is_absolute() {
                manifest.program.clone()
            } else {
                folder.join(&manifest.program)
            };
            let program = program.canonicalize().map_err(|error| {
                PluginError::new(
                    name,
                    format!(
                        "cannot resolve plugin executable '{}': {error}",
                        program.display()
                    ),
                )
            })?;
            if !program.is_file() {
                return Err(PluginError::new(name, "plugin executable is not a file"));
            }
            self.register(
                ProcessPlugin::new(
                    PluginDescriptor {
                        name: name.to_owned(),
                        version: manifest.version,
                        description: manifest.description,
                        capabilities: manifest.capabilities,
                    },
                    prefix,
                    program,
                    manifest.args,
                    manifest.environment,
                )
                .with_config(manifest.config),
            );
            loaded += 1;
        }
        Ok(loaded)
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
                name: "test".into(),
                version: "0.1.0".into(),
                description: "test plugin".into(),
                capabilities: vec!["search".into()],
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
                name: "slow".into(),
                version: "0.1.0".into(),
                description: "deliberately slow test plugin".into(),
                capabilities: vec!["search".into()],
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

    #[cfg(unix)]
    #[tokio::test]
    async fn process_plugin_uses_the_versioned_json_protocol() {
        let plugin = ProcessPlugin::new(
            PluginDescriptor {
                name: "external-test".into(),
                version: "1.0.0".into(),
                description: "test process extension".into(),
                capabilities: vec!["search".into()],
            },
            "external:",
            "/usr/bin/printf",
            vec![r#"{"loadType":"search","data":{"query":"ok"}}"#.into()],
            HashMap::new(),
        );
        let loaded = plugin.load_tracks("external:hello").await.unwrap().unwrap();
        assert_eq!(loaded.load_type, PluginLoadType::Search);
        assert_eq!(loaded.data["query"], "ok");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn registry_timeout_cancels_external_plugin_process() {
        let plugin = ProcessPlugin::new(
            PluginDescriptor {
                name: "external-slow".into(),
                version: "1.0.0".into(),
                description: "slow process extension".into(),
                capabilities: vec!["search".into()],
            },
            "external-slow:",
            "/bin/sleep",
            vec!["5".into()],
            HashMap::new(),
        );
        let mut registry = PluginRegistry::default();
        registry.register(plugin);

        let started = tokio::time::Instant::now();
        let error = registry
            .load_tracks_with_timeout("external-slow:track", Duration::from_millis(50))
            .await
            .unwrap_err();

        assert_eq!(error.plugin, "external-slow");
        assert!(error.message.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn plugin_directory_discovers_manifests_and_resolves_relative_binaries() {
        use std::{
            fs,
            time::{SystemTime, UNIX_EPOCH},
        };

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("ravalink-plugin-test-{nonce}"));
        let plugin_dir = root.join("community-source");
        fs::create_dir_all(plugin_dir.join("bin")).unwrap();
        let executable = plugin_dir.join("bin").join("community-source");
        fs::write(&executable, b"test executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            plugin_dir.join("ravalink-plugin.toml"),
            r#"
                name = "community-source"
                version = "1.0.0"
                description = "Community source"
                capabilities = ["track", "search"]
                identifier_prefix = "community:"
                program = "bin/community-source"
            "#,
        )
        .unwrap();

        let mut registry = PluginRegistry::default();
        assert_eq!(registry.load_directory(&root).unwrap(), 1);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.infos()[0].name, "community-source");
        assert!(registry.find("community:track").is_some());

        fs::remove_dir_all(root).unwrap();
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
