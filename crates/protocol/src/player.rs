//! Lavalink v4 player, track and player-update models.

use crate::{Filters, Omissible};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn empty_object() -> Value {
    Value::Object(Map::new())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub encoded: String,
    pub info: TrackInfo,
    #[serde(default = "empty_object")]
    pub plugin_info: Value,
    #[serde(default = "empty_object")]
    pub user_data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackInfo {
    pub identifier: String,
    pub is_seekable: bool,
    pub author: String,
    pub length: i64,
    pub is_stream: bool,
    pub position: u64,
    pub title: String,
    pub uri: Option<String>,
    pub source_name: String,
    pub artwork_url: Option<String>,
    pub isrc: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceState {
    pub token: String,
    pub endpoint: String,
    pub session_id: String,
    pub channel_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub time: u64,
    pub position: u64,
    pub connected: bool,
    pub ping: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub guild_id: String,
    pub track: Option<Track>,
    pub volume: u16,
    pub paused: bool,
    pub state: PlayerState,
    pub voice: VoiceState,
    pub filters: Filters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerUpdateTrack {
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub encoded: Omissible<Option<String>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub identifier: Omissible<String>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub user_data: Omissible<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerUpdate {
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub encoded_track: Omissible<Option<String>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub identifier: Omissible<String>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub track: Omissible<PlayerUpdateTrack>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub position: Omissible<u64>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub end_time: Omissible<Option<u64>>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub volume: Omissible<u16>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub paused: Omissible<bool>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub filters: Omissible<Filters>,
    #[serde(default, skip_serializing_if = "Omissible::is_omitted")]
    pub voice: Omissible<VoiceState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Players(pub Vec<Player>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tracks(pub Vec<Track>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EncodedTracks(pub Vec<String>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_update_preserves_missing_and_null_track_values() {
        let missing: PlayerUpdate = serde_json::from_str("{}").unwrap();
        let explicit_null: PlayerUpdate = serde_json::from_str(r#"{"encodedTrack":null}"#).unwrap();

        assert_eq!(missing.encoded_track, Omissible::Omitted);
        assert_eq!(explicit_null.encoded_track, Omissible::Present(None));
    }

    #[test]
    fn encoded_tracks_use_a_bare_json_array() {
        let tracks = EncodedTracks(vec!["abc".into(), "def".into()]);
        assert_eq!(serde_json::to_string(&tracks).unwrap(), r#"["abc","def"]"#);
    }

    #[test]
    fn track_fields_use_camel_case() {
        let value = serde_json::json!({
            "encoded": "abc",
            "info": {
                "identifier": "id",
                "isSeekable": true,
                "author": "artist",
                "length": 1000,
                "isStream": false,
                "position": 0,
                "title": "title",
                "uri": null,
                "sourceName": "test",
                "artworkUrl": null,
                "isrc": null
            }
        });
        let track: Track = serde_json::from_value(value).unwrap();
        let output = serde_json::to_value(track).unwrap();
        assert!(output["info"].get("sourceName").is_some());
        assert_eq!(output["pluginInfo"], serde_json::json!({}));
        assert_eq!(output["userData"], serde_json::json!({}));
    }
}
