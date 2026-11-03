//! Native RavaLink configuration file and environment loading.

use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub logging: LoggingConfig,
    pub audio: AudioConfig,
    pub metrics: MetricsConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub address: String,
    pub port: u16,
    pub password: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            address: "0.0.0.0".into(),
            port: 2333,
            password: "youshallnotpass".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub buffer_duration_ms: u64,
    pub frame_buffer_duration_ms: u64,
    pub opus_encoding_quality: u8,
    pub resampling_quality: String,
    pub max_source_download_bytes: usize,
    pub max_decoded_cache_bytes: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            buffer_duration_ms: 400,
            frame_buffer_duration_ms: 5_000,
            opus_encoding_quality: 10,
            resampling_quality: "low".into(),
            max_source_download_bytes: 128 * 1024 * 1024,
            max_decoded_cache_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    pub prometheus_enabled: bool,
    pub prometheus_endpoint: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            prometheus_enabled: false,
            prometheus_endpoint: "/metrics".into(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let explicit_path = env::var_os("RAVALINK_CONFIG").map(PathBuf::from);
        let path = explicit_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("config/ravalink.toml"));
        let mut config = if path.exists() {
            let contents = fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            toml::from_str(&contents)
                .map_err(|error| format!("invalid TOML in {}: {error}", path.display()))?
        } else if explicit_path.is_some() {
            return Err(format!(
                "configuration file {} does not exist",
                path.display()
            ));
        } else {
            Self::default()
        };

        if let Ok(value) = env::var("RAVALINK_ADDRESS") {
            config.server.address = value;
        }
        if let Ok(value) = env::var("RAVALINK_PORT") {
            config.server.port = value
                .parse()
                .map_err(|_| "RAVALINK_PORT must be a valid u16".to_owned())?;
        }
        if let Ok(value) = env::var("RAVALINK_PASSWORD") {
            config.server.password = value;
        }
        if let Ok(value) = env::var("RAVALINK_LOG_LEVEL") {
            config.logging.level = value;
        }
        if let Ok(value) = env::var("RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES") {
            config.audio.max_source_download_bytes = value.parse().map_err(|_| {
                "RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES must be a valid usize".to_owned()
            })?;
        }
        if let Ok(value) = env::var("RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES") {
            config.audio.max_decoded_cache_bytes = value.parse().map_err(|_| {
                "RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES must be a valid usize".to_owned()
            })?;
        }
        validate_metrics_endpoint(
            config.metrics.prometheus_enabled,
            &config.metrics.prometheus_endpoint,
        )?;
        if config.audio.opus_encoding_quality > 10 {
            return Err("audio.opus_encoding_quality must be between 0 and 10".to_owned());
        }
        Ok(config)
    }
}

fn validate_metrics_endpoint(enabled: bool, endpoint: &str) -> Result<(), String> {
    if enabled
        && (!endpoint.starts_with('/')
            || endpoint.starts_with("//")
            || endpoint.contains('?')
            || endpoint.contains('#'))
    {
        return Err(
            "metrics.prometheus_endpoint must be an absolute path without query or fragment"
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_source_memory_limits_are_configurable_in_toml() {
        let config: Config = toml::from_str(
            r#"
                [audio]
                max_source_download_bytes = 16777216
                max_decoded_cache_bytes = 33554432
            "#,
        )
        .unwrap();

        assert_eq!(config.audio.max_source_download_bytes, 16 * 1024 * 1024);
        assert_eq!(config.audio.max_decoded_cache_bytes, 32 * 1024 * 1024);
    }

    #[test]
    fn prometheus_endpoint_must_be_an_absolute_path() {
        assert!(validate_metrics_endpoint(true, "/metrics").is_ok());
        assert!(validate_metrics_endpoint(false, "metrics").is_ok());
        assert!(validate_metrics_endpoint(true, "metrics").is_err());
        assert!(validate_metrics_endpoint(true, "//metrics").is_err());
        assert!(validate_metrics_endpoint(true, "/metrics?format=text").is_err());
        assert!(validate_metrics_endpoint(true, "/metrics#fragment").is_err());
    }

    #[test]
    fn partial_toml_configuration_keeps_defaults() {
        let config: Config = toml::from_str(
            r#"
                [server]
                port = 2444

                [logging]
                level = "debug"
            "#,
        )
        .unwrap();

        assert_eq!(config.server.port, 2444);
        assert_eq!(config.server.address, "0.0.0.0");
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.audio.opus_encoding_quality, 10);
        assert_eq!(config.audio.max_source_download_bytes, 128 * 1024 * 1024);
        assert_eq!(config.audio.max_decoded_cache_bytes, 256 * 1024 * 1024);
        assert!(!config.metrics.prometheus_enabled);
    }
}
