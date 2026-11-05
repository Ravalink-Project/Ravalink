//! Lavalink v4 WebSocket event payloads.
use crate::{CpuStats, FrameStats, MemoryStats, Track};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum WebSocketMessage {
    Ready(ReadyMessage),
    PlayerUpdate(PlayerUpdateMessage),
    Stats(StatsMessage),
    Event(Box<TrackEvent>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReadyMessage {
    pub resumed: bool,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerUpdateMessage {
    pub guild_id: String,
    pub state: PlayerStateUpdate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStateUpdate {
    pub time: u64,
    pub position: u64,
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ping: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatsMessage {
    pub players: usize,
    pub playing_players: usize,
    pub uptime: u64,
    pub memory: MemoryStats,
    pub cpu: CpuStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_stats: Option<FrameStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum TrackEvent {
    #[serde(rename = "TrackStartEvent")]
    TrackStart(TrackEventBase),
    #[serde(rename = "TrackEndEvent")]
    TrackEnd(TrackEndEvent),
    #[serde(rename = "TrackExceptionEvent")]
    TrackException(TrackExceptionEvent),
    #[serde(rename = "TrackStuckEvent")]
    TrackStuck(TrackStuckEvent),
    #[serde(rename = "WebSocketClosedEvent")]
    WebSocketClosed(WebSocketClosedEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackEventBase {
    pub guild_id: String,
    pub track: Track,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackEndEvent {
    pub guild_id: String,
    pub track: Track,
    pub reason: TrackEndReason,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TrackEndReason {
    Finished,
    LoadFailed,
    Stopped,
    Replaced,
    Cleanup,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackExceptionEvent {
    pub guild_id: String,
    pub track: Track,
    pub exception: TrackException,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackException {
    pub message: Option<String>,
    pub severity: ExceptionSeverity,
    pub cause: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExceptionSeverity {
    Common,
    Suspicious,
    Fault,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackStuckEvent {
    pub guild_id: String,
    pub track: Track,
    pub threshold_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketClosedEvent {
    pub guild_id: String,
    pub code: u16,
    pub reason: String,
    pub by_remote: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_track() -> Track {
        Track {
            encoded: "encoded-track".into(),
            info: crate::TrackInfo {
                identifier: "test-id".into(),
                is_seekable: true,
                author: "test artist".into(),
                length: 1_000,
                is_stream: false,
                position: 0,
                title: "test track".into(),
                uri: Some("https://www.youtube.com/watch?v=test-id".into()),
                source_name: "youtube".into(),
                artwork_url: None,
                isrc: None,
            },
            plugin_info: serde_json::json!({}),
            user_data: serde_json::json!({}),
        }
    }

    #[test]
    fn ready_message_matches_lavalink_wire_shape() {
        let message = WebSocketMessage::Ready(ReadyMessage {
            resumed: true,
            session_id: "abc".into(),
        });
        let json = serde_json::to_value(message).unwrap();
        assert_eq!(json["op"], "ready");
        assert_eq!(json["sessionId"], "abc");
    }

    #[test]
    fn stats_message_matches_lavalink_wire_shape() {
        let message = WebSocketMessage::Stats(StatsMessage {
            players: 2,
            playing_players: 1,
            uptime: 60_000,
            memory: MemoryStats {
                free: 100,
                used: 200,
                allocated: 300,
                reservable: 400,
            },
            cpu: CpuStats {
                cores: 4,
                system_load: 0.25,
                lavalink_load: 0.1,
            },
            frame_stats: None,
        });
        let json = serde_json::to_value(message).unwrap();
        assert_eq!(json["op"], "stats");
        assert_eq!(json["players"], 2);
        assert_eq!(json["playingPlayers"], 1);
        assert_eq!(json["memory"]["reservable"], 400);
        assert_eq!(json["cpu"]["systemLoad"], 0.25);
        assert!(json.get("frameStats").is_none());
    }

    #[test]
    fn player_update_message_matches_lavalink_wire_shape() {
        let message = WebSocketMessage::PlayerUpdate(PlayerUpdateMessage {
            guild_id: "123".into(),
            state: PlayerStateUpdate {
                time: 1_700_000_000_000,
                position: 42_000,
                connected: false,
                ping: Some(-1),
            },
        });
        let json = serde_json::to_value(message).unwrap();
        assert_eq!(json["op"], "playerUpdate");
        assert_eq!(json["guildId"], "123");
        assert_eq!(json["state"]["position"], 42_000);
        assert_eq!(json["state"]["connected"], false);
        assert_eq!(json["state"]["ping"], -1);
    }

    #[test]
    fn track_end_event_uses_expected_discriminator_and_reason() {
        let event = WebSocketMessage::Event(Box::new(TrackEvent::TrackEnd(TrackEndEvent {
            guild_id: "1".into(),
            track: test_track(),
            reason: TrackEndReason::Finished,
        })));
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["op"], "event");
        assert_eq!(json["type"], "TrackEndEvent");
        assert_eq!(json["reason"], "finished");
        assert_eq!(json["track"]["encoded"], "encoded-track");
        assert_eq!(json["track"]["info"]["title"], "test track");
    }

    #[test]
    fn websocket_closed_event_matches_lavalink_wire_shape() {
        let event = WebSocketMessage::Event(Box::new(TrackEvent::WebSocketClosed(
            WebSocketClosedEvent {
                guild_id: "123".into(),
                code: 4014,
                reason: "voice channel changed".into(),
                by_remote: true,
            },
        )));
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["op"], "event");
        assert_eq!(json["type"], "WebSocketClosedEvent");
        assert_eq!(json["guildId"], "123");
        assert_eq!(json["code"], 4014);
        assert_eq!(json["reason"], "voice channel changed");
        assert_eq!(json["byRemote"], true);
    }

    #[test]
    fn track_exception_and_stuck_events_embed_track_objects() {
        let exception =
            WebSocketMessage::Event(Box::new(TrackEvent::TrackException(TrackExceptionEvent {
                guild_id: "1".into(),
                track: test_track(),
                exception: TrackException {
                    message: Some("decoder failed".into()),
                    severity: ExceptionSeverity::Fault,
                    cause: "DecodeError".into(),
                },
            })));
        let stuck = WebSocketMessage::Event(Box::new(TrackEvent::TrackStuck(TrackStuckEvent {
            guild_id: "1".into(),
            track: test_track(),
            threshold_ms: 10_000,
        })));

        let exception_json = serde_json::to_value(exception).unwrap();
        let stuck_json = serde_json::to_value(stuck).unwrap();
        assert_eq!(exception_json["type"], "TrackExceptionEvent");
        assert_eq!(exception_json["track"]["info"]["identifier"], "test-id");
        assert_eq!(stuck_json["type"], "TrackStuckEvent");
        assert_eq!(stuck_json["track"]["encoded"], "encoded-track");
    }
}
