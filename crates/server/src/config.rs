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
    pub player_update_interval: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            address: "0.0.0.0".into(),
            port: 2333,
            password: "youshallnotpass".into(),
            player_update_interval: 5,
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
        if let Ok(value) = env::var("RAVALINK_PLAYER_UPDATE_INTERVAL") {
            config.server.player_update_interval = value
                .parse()
                .map_err(|_| "RAVALINK_PLAYER_UPDATE_INTERVAL must be a valid u64".to_owned())?;
        }
        if let Ok(value) = env::var("RAVALINK_LOG_LEVEL") {
            config.logging.level = value;
        }
        if let Ok(value) = env::var("RAVALINK_AUDIO_BUFFER_DURATION_MS") {
            config.audio.buffer_duration_ms = parse_env("RAVALINK_AUDIO_BUFFER_DURATION_MS", &value)?;
        }
        if let Ok(value) = env::var("RAVALINK_AUDIO_FRAME_BUFFER_DURATION_MS") {
            config.audio.frame_buffer_duration_ms =
                parse_env("RAVALINK_AUDIO_FRAME_BUFFER_DURATION_MS", &value)?;
        }
        if let Ok(value) = env::var("RAVALINK_AUDIO_OPUS_ENCODING_QUALITY") {
            config.audio.opus_encoding_quality =
                parse_env("RAVALINK_AUDIO_OPUS_ENCODING_QUALITY", &value)?;
        }
        if let Ok(value) = env::var("RAVALINK_AUDIO_RESAMPLING_QUALITY") {
            config.audio.resampling_quality = value;
        }
        if let Ok(value) = env::var("RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES") {
            config.audio.max_source_download_bytes =
                parse_env("RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES", &value)?;
        }
        if let Ok(value) = env::var("RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES") {
            config.audio.max_decoded_cache_bytes =
                parse_env("RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES", &value)?;
        }
        if let Ok(value) = env::var("RAVALINK_METRICS_PROMETHEUS_ENABLED") {
            config.metrics.prometheus_enabled =
                parse_env("RAVALINK_METRICS_PROMETHEUS_ENABLED", &value)?;
        }
        if let Ok(value) = env::var("RAVALINK_METRICS_PROMETHEUS_ENDPOINT") {
            config.metrics.prometheus_endpoint = value;
        }

        validate_player_update_interval(config.server.player_update_interval)?;
        validate_server_password(&config.server.password)?;
        validate_audio_config(&config.audio)?;
        validate_metrics_endpoint(
            config.metrics.prometheus_enabled,
            &config.metrics.prometheus_endpoint,
        )?;
        Ok(config)
    }
}

fn parse_env<T>(name: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| format!("{name} has an invalid value: {value}"))
}

fn validate_audio_config(audio: &AudioConfig) -> Result<(), String> {
    if audio.buffer_duration_ms == 0 {
        return Err("audio.buffer_duration_ms must be greater than zero".to_owned());
    }
    if audio.frame_buffer_duration_ms == 0 {
        return Err("audio.frame_buffer_duration_ms must be greater than zero".to_owned());
    }
    if audio.opus_encoding_quality > 10 {
        return Err("audio.opus_encoding_quality must be between 0 and 10".to_owned());
    }
    if audio.max_source_download_bytes == 0 {
        return Err("audio.max_source_download_bytes must be greater than zero".to_owned());
    }
    if audio.max_decoded_cache_bytes == 0 {
        return Err("audio.max_decoded_cache_bytes must be greater than zero".to_owned());
    }
    if audio.resampling_quality.trim().is_empty() {
        return Err("audio.resampling_quality must not be empty".to_owned());
    }
    Ok(())
}

fn validate_server_password(password: &str) -> Result<(), String> {
    if password.trim().is_empty() {
        return Err("server.password must not be empty".to_owned());
    }
    Ok(())
}

fn validate_player_update_interval(interval: u64) -> Result<(), String> {
    if interval == 0 {
        return Err("server.player_update_interval must be greater than zero".to_owned());
    }
    Ok(())
}

fn validate_metrics_endpoint(enabled: bool, endpoint: &str) -> Result<(), String> {
    if enabled
        && (!endpoint.starts_with('/')
            || endpoint.starts_with("//")
            || endpoint.contains('?')
            || endpoint.contains('#')
            || endpoint == "/version"
            || endpoint == "/v4"
            || endpoint.starts_with("/v4/"))
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
    fn audio_configuration_rejects_zero_limits_and_empty_quality() {
        let mut audio = AudioConfig::default();
        assert!(validate_audio_config(&audio).is_ok());

        audio.buffer_duration_ms = 0;
        assert!(validate_audio_config(&audio).is_err());
        audio.buffer_duration_ms = 400;

        audio.frame_buffer_duration_ms = 0;
        assert!(validate_audio_config(&audio).is_err());
        audio.frame_buffer_duration_ms = 5_000;

        audio.max_source_download_bytes = 0;
        assert!(validate_audio_config(&audio).is_err());
        audio.max_source_download_bytes = 128 * 1024 * 1024;

        audio.max_decoded_cache_bytes = 0;
        assert!(validate_audio_config(&audio).is_err());
        audio.max_decoded_cache_bytes = 256 * 1024 * 1024;

        audio.resampling_quality.clear();
        assert!(validate_audio_config(&audio).is_err());
    }

    #[test]
    fn environment_values_are_parsed_without_silent_fallbacks() {
        assert_eq!(parse_env::<u64>("INTERVAL", "25").unwrap(), 25);
        assert!(parse_env::<u64>("INTERVAL", "fast").is_err());
        assert!(parse_env::<bool>("ENABLED", "perhaps").is_err());
        assert!(parse_env::<usize>("LIMIT", "-1").is_err());
    }

    #[test]
    fn server_password_must_not_be_empty() {
        assert!(validate_server_password("secret").is_ok());
        assert!(validate_server_password("").is_err());
        assert!(validate_server_password("   ").is_err());
    }

    #[test]
    fn player_update_interval_must_be_nonzero() {
        assert!(validate_player_update_interval(5).is_ok());
        assert!(validate_player_update_interval(0).is_err());
    }

    #[test]
    fn prometheus_endpoint_must_be_an_absolute_path() {
        assert!(validate_metrics_endpoint(true, "/metrics").is_ok());
        assert!(validate_metrics_endpoint(false, "metrics").is_ok());
        assert!(validate_metrics_endpoint(true, "metrics").is_err());
        assert!(validate_metrics_endpoint(true, "//metrics").is_err());
        assert!(validate_metrics_endpoint(true, "/metrics?format=text").is_err());
        assert!(validate_metrics_endpoint(true, "/metrics#fragment").is_err());
        assert!(validate_metrics_endpoint(true, "/version").is_err());
        assert!(validate_metrics_endpoint(true, "/v4/stats").is_err());
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
        assert_eq!(config.server.player_update_interval, 5);
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.audio.opus_encoding_quality, 10);
        assert_eq!(config.audio.max_source_download_bytes, 128 * 1024 * 1024);
        assert_eq!(config.audio.max_decoded_cache_bytes, 256 * 1024 * 1024);
        assert!(!config.metrics.prometheus_enabled);
    }
}
