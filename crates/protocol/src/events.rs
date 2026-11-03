//! Lavalink v4 WebSocket event payloads.
use crate::{CpuStats, FrameStats, MemoryStats};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum WebSocketMessage {
    Ready(ReadyMessage),
    PlayerUpdate(PlayerUpdateMessage),
    Stats(StatsMessage),
    Event(TrackEvent),
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
    pub track: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackEndEvent {
    pub guild_id: String,
    pub track: String,
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
    pub track: String,
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
    pub track: String,
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
        let event = WebSocketMessage::Event(TrackEvent::TrackEnd(TrackEndEvent {
            guild_id: "1".into(),
            track: "encoded".into(),
            reason: TrackEndReason::Finished,
        }));
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["op"], "event");
        assert_eq!(json["type"], "TrackEndEvent");
        assert_eq!(json["reason"], "finished");
    }
}
