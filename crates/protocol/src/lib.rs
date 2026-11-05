//! Wire-level data structures for the Lavalink v4-compatible RavaLink API.
//!
//! Keep this crate independent from HTTP, WebSocket and audio implementation details.

use serde::{Deserialize, Serialize};

pub mod events;
pub mod filters;
pub mod omissible;
pub mod player;
pub mod track_codec;

pub use events::*;
pub use filters::*;
pub use omissible::Omissible;
pub use player::*;
pub use track_codec::*;

pub const API_MAJOR: u32 = 4;
pub const RAVALINK_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub semver: String,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub pre_release: Option<String>,
}

impl Default for VersionInfo {
    fn default() -> Self {
        Self {
            semver: format!("4.0.0-ravalink.{}", RAVALINK_VERSION),
            major: 4,
            minor: 0,
            patch: 0,
            pre_release: Some(format!("ravalink.{}", RAVALINK_VERSION)),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub branch: String,
    pub commit: String,
    pub commit_time: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub version: VersionInfo,
    pub build_time: u64,
    pub git: GitInfo,
    /// Retained for Lavalink v4 wire compatibility. RavaLink does not run a JVM.
    pub jvm: String,
    /// Retained for Lavalink v4 wire compatibility. RavaLink does not use Lavaplayer.
    pub lavaplayer: String,
    pub source_managers: Vec<String>,
    pub filters: Vec<String>,
    pub plugins: Vec<PluginInfo>,
}

impl Default for ServerInfo {
    fn default() -> Self {
        Self {
            version: VersionInfo::default(),
            build_time: 0,
            git: GitInfo {
                branch: "master".into(),
                commit: "unknown".into(),
                commit_time: 0,
            },
            jvm: "Not used (native Rust runtime)".into(),
            lavaplayer: "Not used (native RavaLink audio engine)".into(),
            source_managers: vec!["http".into()],
            filters: vec![
                "volume".into(),
                "equalizer".into(),
                "karaoke".into(),
                "timescale".into(),
                "tremolo".into(),
                "vibrato".into(),
                "distortion".into(),
                "rotation".into(),
                "channelMix".into(),
                "lowPass".into(),
            ],
            plugins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    pub free: u64,
    pub used: u64,
    pub allocated: u64,
    pub reservable: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CpuStats {
    pub cores: usize,
    pub system_load: f64,
    pub lavalink_load: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_stats: Option<FrameStats>,
    pub players: usize,
    pub playing_players: usize,
    pub uptime: u64,
    pub memory: MemoryStats,
    pub cpu: CpuStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrameStats {
    pub sent: u64,
    pub nulled: u64,
    pub deficit: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub resuming: bool,
    pub timeout: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdate {
    pub resuming: Option<bool>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadResult {
    pub load_type: &'static str,
    pub data: Option<serde_json::Value>,
}

impl LoadResult {
    pub fn empty() -> Self {
        Self {
            load_type: "empty",
            data: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub timestamp: u64,
    pub status: u16,
    pub error: String,
    pub message: String,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_uses_lavalink_camel_case_field_names() {
        let value = serde_json::to_value(ServerInfo::default()).unwrap();
        assert!(value.get("buildTime").is_some());
        assert!(value.get("sourceManagers").is_some());
        assert!(value.get("lavaplayer").is_some());
        assert!(
            value["filters"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("vibrato"))
        );
        assert!(
            value["filters"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("timescale"))
        );
    }

    #[test]
    fn stats_omit_frame_stats_when_unavailable_like_lavalink_v4() {
        let stats = NodeStats {
            frame_stats: None,
            players: 0,
            playing_players: 0,
            uptime: 1,
            memory: MemoryStats {
                free: 1,
                used: 0,
                allocated: 1,
                reservable: 1,
            },
            cpu: CpuStats {
                cores: 1,
                system_load: 0.0,
                lavalink_load: 0.0,
            },
        };
        let value = serde_json::to_value(stats).unwrap();
        assert!(value.get("frameStats").is_none());
        assert_eq!(value["players"], 0);
        assert_eq!(value["playingPlayers"], 0);
    }

    #[test]
    fn empty_load_result_matches_v4_shape() {
        let value = serde_json::to_value(LoadResult::empty()).unwrap();
        assert_eq!(value["loadType"], "empty");
        assert!(value["data"].is_null());
    }
}
