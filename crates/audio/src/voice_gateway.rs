//! Discord Voice Gateway v8 protocol state machine.
//!
//! This module separates the JSON control plane from socket and UDP I/O.
//! Network code executes the validated actions and forwards DAVE MLS events
//! to the per-guild DaveVoiceSession adapter.

use crate::voice_crypto::VoiceEncryptionMode;
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceGatewayError {
    InvalidJson(String),
    InvalidPayload(String),
    UnexpectedOpcode { opcode: u8, state: &'static str },
}
impl std::fmt::Display for VoiceGatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(e) => write!(f, "invalid voice gateway JSON: {e}"),
            Self::InvalidPayload(e) => f.write_str(e),
            Self::UnexpectedOpcode { opcode, state } => {
                write!(f, "unexpected voice gateway opcode {opcode} while {state}")
            }
        }
    }
}
impl std::error::Error for VoiceGatewayError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceGatewayState {
    Connecting,
    Identifying,
    DiscoveringUdp,
    AwaitingSessionDescription,
    Ready,
    Closed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceGatewayAction {
    Send(Value),
    StartHeartbeat {
        interval_ms: u64,
    },
    DiscoverUdp {
        remote: SocketAddr,
        ssrc: u32,
    },
    StartVoiceOutput {
        ssrc: u32,
        mode: VoiceEncryptionMode,
        secret_key: [u8; 32],
        dave_protocol_version: u16,
    },
    DaveEvent {
        opcode: u8,
        data: Value,
    },
    DaveBinaryEvent {
        sequence: u16,
        opcode: u8,
        payload: Vec<u8>,
    },
    ClientsConnect {
        user_ids: Vec<u64>,
    },
    ClientDisconnect {
        user_id: u64,
    },
    Resumed,
    HeartbeatAck,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VoiceReady {
    pub ssrc: u32,
    pub ip: String,
    pub port: u16,
    pub modes: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VoiceSessionDescription {
    pub mode: String,
    pub secret_key: Vec<u8>,
    #[serde(default)]
    pub dave_protocol_version: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct VoiceGatewayMachine {
    state: VoiceGatewayState,
    guild_id: String,
    user_id: String,
    discord_session_id: String,
    token: String,
    ssrc: Option<u32>,
    remote: Option<SocketAddr>,
    selected_mode: Option<VoiceEncryptionMode>,
    connected_user_ids: Vec<u64>,
    last_sequence: Option<u16>,
    resume_requested: bool,
}
impl VoiceGatewayMachine {
    pub fn new(
        guild_id: impl Into<String>,
        user_id: impl Into<String>,
        discord_session_id: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, VoiceGatewayError> {
        let guild_id = guild_id.into();
        let user_id = user_id.into();
        let discord_session_id = discord_session_id.into();
        let token = token.into();
        if [
            guild_id.as_str(),
            user_id.as_str(),
            discord_session_id.as_str(),
            token.as_str(),
        ]
        .iter()
        .any(|value| value.is_empty())
        {
            return Err(VoiceGatewayError::InvalidPayload(
                "voice gateway identify fields cannot be empty".into(),
            ));
        }
        Ok(Self {
            state: VoiceGatewayState::Connecting,
            guild_id,
            user_id,
            discord_session_id,
            token,
            ssrc: None,
            remote: None,
            selected_mode: None,
            connected_user_ids: Vec::new(),
            last_sequence: None,
            resume_requested: false,
        })
    }
    pub fn state(&self) -> VoiceGatewayState {
        self.state
    }
    pub fn ssrc(&self) -> Option<u32> {
        self.ssrc
    }
    pub fn remote_addr(&self) -> Option<SocketAddr> {
        self.remote
    }
    pub fn selected_mode(&self) -> Option<VoiceEncryptionMode> {
        self.selected_mode
    }
    pub fn connected_user_ids(&self) -> &[u64] {
        &self.connected_user_ids
    }

    pub fn request_resume(&mut self) {
        self.resume_requested = true;
        self.state = VoiceGatewayState::Connecting;
    }

    pub fn handle_message(
        &mut self,
        message: &str,
        now_ms: u64,
    ) -> Result<Vec<VoiceGatewayAction>, VoiceGatewayError> {
        let envelope: Value = serde_json::from_str(message)
            .map_err(|e| VoiceGatewayError::InvalidJson(e.to_string()))?;
        let opcode = envelope
            .get("op")
            .and_then(Value::as_u64)
            .and_then(|v| u8::try_from(v).ok())
            .ok_or_else(|| {
                VoiceGatewayError::InvalidPayload(
                    "voice gateway envelope is missing a valid opcode".into(),
                )
            })?;
        if let Some(sequence) = envelope
            .get("seq")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
        {
            self.last_sequence = Some(sequence);
        }
        let data = envelope.get("d").cloned().unwrap_or(Value::Null);
        match opcode {
            8 => {
                if self.state != VoiceGatewayState::Connecting {
                    return Err(self.unexpected(opcode));
                }
                let interval = data
                    .get("heartbeat_interval")
                    .and_then(Value::as_u64)
                    .filter(|v| *v > 0)
                    .ok_or_else(|| {
                        VoiceGatewayError::InvalidPayload(
                            "voice gateway HELLO has an invalid heartbeat_interval".into(),
                        )
                    })?;
                self.state = VoiceGatewayState::Identifying;
                Ok(vec![
                    VoiceGatewayAction::StartHeartbeat {
                        interval_ms: interval,
                    },
                    VoiceGatewayAction::Send(if self.resume_requested {
                        json!({"op":7,"d":{
                            "server_id":self.guild_id,
                            "session_id":self.discord_session_id,
                            "token":self.token,
                            "seq_ack":self.last_sequence.map_or(-1, i64::from)
                        }})
                    } else {
                        json!({"op":0,"d":{
                            "server_id":self.guild_id,"user_id":self.user_id,
                            "session_id":self.discord_session_id,"token":self.token,
                            "max_dave_protocol_version":crate::dave::DaveVoiceSession::max_protocol_version()
                        }})
                    }),
                    VoiceGatewayAction::Send(self.heartbeat(now_ms)),
                ])
            }
            2 => {
                if self.state != VoiceGatewayState::Identifying {
                    return Err(self.unexpected(opcode));
                }
                let ready: VoiceReady = serde_json::from_value(data).map_err(|e| {
                    VoiceGatewayError::InvalidPayload(format!(
                        "invalid voice gateway READY payload: {e}"
                    ))
                })?;
                if ready.ssrc == 0 || ready.port == 0 {
                    return Err(VoiceGatewayError::InvalidPayload(
                        "voice gateway READY contains a zero SSRC or UDP port".into(),
                    ));
                }
                let ip: IpAddr = ready.ip.parse().map_err(|_| {
                    VoiceGatewayError::InvalidPayload(
                        "voice gateway READY contains an invalid IP address".into(),
                    )
                })?;
                if !ip.is_ipv4() || ip.is_unspecified() || ip.is_multicast() {
                    return Err(VoiceGatewayError::InvalidPayload(
                        "voice gateway READY must contain a routable IPv4 address".into(),
                    ));
                }
                let mode = choose_encryption_mode(&ready.modes).ok_or_else(|| {
                    VoiceGatewayError::InvalidPayload(
                        "Discord offered no supported voice encryption mode".into(),
                    )
                })?;
                let remote = SocketAddr::new(ip, ready.port);
                self.ssrc = Some(ready.ssrc);
                self.remote = Some(remote);
                self.selected_mode = Some(mode);
                self.state = VoiceGatewayState::DiscoveringUdp;
                Ok(vec![VoiceGatewayAction::DiscoverUdp {
                    remote,
                    ssrc: ready.ssrc,
                }])
            }
            4 => {
                if self.state != VoiceGatewayState::AwaitingSessionDescription {
                    return Err(self.unexpected(opcode));
                }
                let description: VoiceSessionDescription =
                    serde_json::from_value(data).map_err(|e| {
                        VoiceGatewayError::InvalidPayload(format!(
                            "invalid voice session description: {e}"
                        ))
                    })?;
                let mode = parse_encryption_mode(&description.mode).ok_or_else(|| {
                    VoiceGatewayError::InvalidPayload(format!(
                        "Discord selected unsupported voice encryption mode '{}'",
                        description.mode
                    ))
                })?;
                if Some(mode) != self.selected_mode {
                    return Err(VoiceGatewayError::InvalidPayload(
                        "Discord selected a voice encryption mode different from the negotiated mode".into(),
                    ));
                }
                let secret_key: [u8; 32] = description.secret_key.try_into().map_err(|_| {
                    VoiceGatewayError::InvalidPayload(
                        "voice session secret_key must contain exactly 32 bytes".into(),
                    )
                })?;
                let dave_protocol_version = description.dave_protocol_version.unwrap_or(0);
                if dave_protocol_version > crate::dave::DaveVoiceSession::max_protocol_version() {
                    return Err(VoiceGatewayError::InvalidPayload(format!(
                        "Discord negotiated unsupported DAVE protocol version {dave_protocol_version}"
                    )));
                }
                let ssrc = self.ssrc.ok_or_else(|| {
                    VoiceGatewayError::InvalidPayload("voice session has no SSRC".into())
                })?;
                self.state = VoiceGatewayState::Ready;
                Ok(vec![VoiceGatewayAction::StartVoiceOutput {
                    ssrc,
                    mode,
                    secret_key,
                    dave_protocol_version,
                }])
            }
            3 => Ok(vec![VoiceGatewayAction::Send(self.heartbeat(now_ms))]),
            6 => Ok(vec![VoiceGatewayAction::HeartbeatAck]),
            9 => {
                if self.state == VoiceGatewayState::Closed {
                    return Err(self.unexpected(opcode));
                }
                self.state = VoiceGatewayState::Ready;
                self.resume_requested = false;
                Ok(vec![VoiceGatewayAction::Resumed])
            }
            21 | 22 | 24 => {
                if self.state != VoiceGatewayState::Ready {
                    return Err(self.unexpected(opcode));
                }
                Ok(vec![VoiceGatewayAction::DaveEvent { opcode, data }])
            }
            11 => {
                let user_ids = data
                    .get("user_ids")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        VoiceGatewayError::InvalidPayload(
                            "CLIENTS_CONNECT is missing its user_ids array".into(),
                        )
                    })?
                    .iter()
                    .map(parse_snowflake)
                    .collect::<Result<Vec<_>, _>>()?;
                for user_id in &user_ids {
                    if !self.connected_user_ids.contains(user_id) {
                        self.connected_user_ids.push(*user_id);
                    }
                }
                Ok(vec![VoiceGatewayAction::ClientsConnect { user_ids }])
            }
            13 => {
                let user_id = data
                    .get("user_id")
                    .map(parse_snowflake)
                    .transpose()?
                    .ok_or_else(|| {
                        VoiceGatewayError::InvalidPayload(
                            "CLIENT_DISCONNECT is missing user_id".into(),
                        )
                    })?;
                self.connected_user_ids
                    .retain(|connected| *connected != user_id);
                Ok(vec![VoiceGatewayAction::ClientDisconnect { user_id }])
            }
            5 | 12 | 14 => Ok(Vec::new()),
            _ => Ok(Vec::new()),
        }
    }

    /// Process a binary voice-gateway frame. DAVE MLS messages use a
    /// two-byte big-endian sequence number, one-byte opcode, then opaque MLS
    /// payload; they are not JSON envelopes.
    pub fn handle_binary_message(
        &mut self,
        message: &[u8],
    ) -> Result<Vec<VoiceGatewayAction>, VoiceGatewayError> {
        if self.state != VoiceGatewayState::Ready {
            return Err(VoiceGatewayError::InvalidPayload(
                "received a binary DAVE frame before voice transport was ready".into(),
            ));
        }
        if message.len() < 3 {
            return Err(VoiceGatewayError::InvalidPayload(
                "binary voice gateway frame is shorter than its sequence and opcode".into(),
            ));
        }
        let sequence = u16::from_be_bytes([message[0], message[1]]);
        self.last_sequence = Some(sequence);
        let opcode = message[2];
        if !matches!(opcode, 25 | 27 | 29 | 30) {
            return Err(VoiceGatewayError::InvalidPayload(format!(
                "unexpected binary voice gateway opcode {opcode}"
            )));
        }
        Ok(vec![VoiceGatewayAction::DaveBinaryEvent {
            sequence,
            opcode,
            payload: message[3..].to_vec(),
        }])
    }

    pub fn on_udp_discovered(
        &mut self,
        external_address: &str,
        external_port: u16,
    ) -> Result<Value, VoiceGatewayError> {
        if self.state != VoiceGatewayState::DiscoveringUdp {
            return Err(VoiceGatewayError::UnexpectedOpcode {
                opcode: 1,
                state: self.state_name(),
            });
        }
        if external_port == 0
            || external_address.is_empty()
            || external_address.len() > 63
            || external_address.chars().any(|c| c.is_control())
        {
            return Err(VoiceGatewayError::InvalidPayload(
                "UDP discovery returned an invalid external address or port".into(),
            ));
        }
        let mode = self.selected_mode.ok_or_else(|| {
            VoiceGatewayError::InvalidPayload("voice encryption mode was not selected".into())
        })?;
        self.state = VoiceGatewayState::AwaitingSessionDescription;
        Ok(json!({"op":1,"d":{"protocol":"udp","data":{
            "address":external_address,"port":external_port,
            "mode":encryption_mode_name(mode)
        }}}))
    }
    pub fn heartbeat(&self, now_ms: u64) -> Value {
        json!({
            "op": 3,
            "d": {
                "t": now_ms,
                "seq_ack": self.last_sequence.map_or(-1, i64::from)
            }
        })
    }
    pub fn speaking(&self) -> Result<Value, VoiceGatewayError> {
        self.speaking_state(true)
    }

    pub fn speaking_state(&self, speaking: bool) -> Result<Value, VoiceGatewayError> {
        let ssrc = self.ssrc.ok_or_else(|| {
            VoiceGatewayError::InvalidPayload("cannot signal speaking before READY".into())
        })?;
        Ok(json!({
            "op": 5,
            "d": {"speaking": u8::from(speaking), "delay": 0, "ssrc": ssrc}
        }))
    }
    pub fn close(&mut self) {
        self.state = VoiceGatewayState::Closed;
    }
    fn unexpected(&self, opcode: u8) -> VoiceGatewayError {
        VoiceGatewayError::UnexpectedOpcode {
            opcode,
            state: self.state_name(),
        }
    }
    fn state_name(&self) -> &'static str {
        match self.state {
            VoiceGatewayState::Connecting => "connecting",
            VoiceGatewayState::Identifying => "identifying",
            VoiceGatewayState::DiscoveringUdp => "discovering UDP",
            VoiceGatewayState::AwaitingSessionDescription => "awaiting session description",
            VoiceGatewayState::Ready => "ready",
            VoiceGatewayState::Closed => "closed",
        }
    }
}

pub fn choose_encryption_mode(modes: &[String]) -> Option<VoiceEncryptionMode> {
    [
        (
            "aead_aes256_gcm_rtpsize",
            VoiceEncryptionMode::AeadAes256GcmRtpSize,
        ),
        (
            "aead_xchacha20_poly1305_rtpsize",
            VoiceEncryptionMode::AeadXChaCha20Poly1305RtpSize,
        ),
        (
            "xsalsa20_poly1305_lite",
            VoiceEncryptionMode::Xsalsa20Poly1305Lite,
        ),
        (
            "xsalsa20_poly1305_suffix",
            VoiceEncryptionMode::Xsalsa20Poly1305Suffix,
        ),
        ("xsalsa20_poly1305", VoiceEncryptionMode::Xsalsa20Poly1305),
    ]
    .into_iter()
    .find_map(|(name, mode)| modes.iter().any(|offered| offered == name).then_some(mode))
}
pub fn parse_encryption_mode(name: &str) -> Option<VoiceEncryptionMode> {
    choose_encryption_mode(&[name.to_owned()])
}
pub fn encryption_mode_name(mode: VoiceEncryptionMode) -> &'static str {
    match mode {
        VoiceEncryptionMode::AeadAes256GcmRtpSize => "aead_aes256_gcm_rtpsize",
        VoiceEncryptionMode::AeadXChaCha20Poly1305RtpSize => "aead_xchacha20_poly1305_rtpsize",
        VoiceEncryptionMode::Xsalsa20Poly1305LiteRtpSize
        | VoiceEncryptionMode::Xsalsa20Poly1305Lite => "xsalsa20_poly1305_lite",
        VoiceEncryptionMode::Xsalsa20Poly1305 => "xsalsa20_poly1305",
        VoiceEncryptionMode::Xsalsa20Poly1305Suffix => "xsalsa20_poly1305_suffix",
    }
}
fn parse_snowflake(value: &Value) -> Result<u64, VoiceGatewayError> {
    value
        .as_str()
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| value.as_u64())
        .ok_or_else(|| {
            VoiceGatewayError::InvalidPayload(
                "voice gateway event contains an invalid user ID".into(),
            )
        })
}

pub fn voice_gateway_url(endpoint: &str) -> Result<String, VoiceGatewayError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty()
        || endpoint.contains("://")
        || endpoint.chars().any(char::is_whitespace)
        || endpoint
            .chars()
            .any(|c| matches!(c, '/' | '?' | '#' | '@' | '\r' | '\n'))
    {
        return Err(VoiceGatewayError::InvalidPayload(
            "voice gateway endpoint must be a hostname with an optional port".into(),
        ));
    }

    let url = reqwest::Url::parse(&format!("wss://{endpoint}/?v=8")).map_err(|_| {
        VoiceGatewayError::InvalidPayload("voice gateway endpoint is not a valid host".into())
    })?;
    if url.scheme() != "wss"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query() != Some("v=8")
        || url.fragment().is_some()
    {
        return Err(VoiceGatewayError::InvalidPayload(
            "voice gateway endpoint must be a hostname with an optional port".into(),
        ));
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> VoiceGatewayMachine {
        VoiceGatewayMachine::new("guild", "1234", "discord-session", "secret").unwrap()
    }

    #[test]
    fn hello_identifies_and_starts_heartbeat() {
        let mut m = machine();
        let actions = m
            .handle_message(r#"{"op":8,"d":{"heartbeat_interval":41250}}"#, 123)
            .unwrap();
        assert_eq!(
            actions,
            vec![
                VoiceGatewayAction::StartHeartbeat { interval_ms: 41250 },
                VoiceGatewayAction::Send(json!({"op":0,"d":{
                    "server_id":"guild","user_id":"1234","session_id":"discord-session","token":"secret",
                    "max_dave_protocol_version":1
                }})),
                VoiceGatewayAction::Send(json!({"op":3,"d":{"t":123,"seq_ack":-1}}))
            ]
        );
        assert_eq!(m.state(), VoiceGatewayState::Identifying);
    }
    #[test]
    fn resumes_voice_gateway_with_last_server_sequence_acknowledged() {
        let mut m = machine();
        m.handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 0)
            .unwrap();
        m.handle_message(
            r#"{"op":2,"d":{"ssrc":42,"ip":"203.0.113.10","port":50000,"modes":["aead_aes256_gcm_rtpsize"]}}"#,
            1,
        )
        .unwrap();
        m.on_udp_discovered("198.51.100.4", 40000).unwrap();
        m.handle_message(
            &json!({"op":4,"d":{"mode":"aead_aes256_gcm_rtpsize","secret_key":vec![1;32]}})
                .to_string(),
            2,
        )
        .unwrap();
        m.handle_binary_message(&[0x12, 0x34, 29, 0xaa]).unwrap();
        m.request_resume();
        let actions = m
            .handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 3)
            .unwrap();
        assert!(actions.contains(&VoiceGatewayAction::Send(json!({
            "op":7,
            "d":{
                "server_id":"guild",
                "session_id":"discord-session",
                "token":"secret",
                "seq_ack":0x1234
            }
        }))));
        assert_eq!(
            m.handle_message(r#"{"op":9,"d":null}"#, 4).unwrap(),
            vec![VoiceGatewayAction::Resumed]
        );
        assert_eq!(m.state(), VoiceGatewayState::Ready);
    }

    #[test]
    fn reports_heartbeat_acknowledgements_to_the_transport_driver() {
        let mut m = machine();
        m.handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 0)
            .unwrap();
        assert_eq!(
            m.handle_message(r#"{"op":6,"d":null}"#, 456).unwrap(),
            vec![VoiceGatewayAction::HeartbeatAck]
        );
    }

    #[test]
    fn responds_immediately_to_gateway_heartbeat_requests() {
        let mut m = machine();
        m.handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 0)
            .unwrap();
        assert_eq!(
            m.handle_message(r#"{"op":3,"d":null}"#, 456).unwrap(),
            vec![VoiceGatewayAction::Send(
                json!({"op":3,"d":{"t":456,"seq_ack":-1}})
            )]
        );
    }

    #[test]
    fn ready_requests_udp_discovery_and_prefers_aead() {
        let mut m = machine();
        m.handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 0)
            .unwrap();
        let actions = m.handle_message(
            r#"{"op":2,"d":{"ssrc":42,"ip":"203.0.113.10","port":50000,"modes":["xsalsa20_poly1305_lite","aead_aes256_gcm_rtpsize"]}}"#,
            1,
        ).unwrap();
        assert_eq!(
            actions,
            vec![VoiceGatewayAction::DiscoverUdp {
                remote: "203.0.113.10:50000".parse().unwrap(),
                ssrc: 42
            }]
        );
        assert_eq!(
            m.selected_mode(),
            Some(VoiceEncryptionMode::AeadAes256GcmRtpSize)
        );
    }
    #[test]
    fn discovery_and_session_description_complete_transport_negotiation() {
        let mut m = machine();
        m.handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 0)
            .unwrap();
        m.handle_message(
            r#"{"op":2,"d":{"ssrc":42,"ip":"203.0.113.10","port":50000,"modes":["aead_aes256_gcm_rtpsize"]}}"#,
            1,
        ).unwrap();
        let select = m.on_udp_discovered("198.51.100.4", 40000).unwrap();
        assert_eq!(select["op"], 1);
        assert_eq!(select["d"]["data"]["mode"], "aead_aes256_gcm_rtpsize");
        let description = json!({"op":4,"d":{
            "mode":"aead_aes256_gcm_rtpsize","secret_key":vec![7;32],"dave_protocol_version":1
        }});
        let actions = m.handle_message(&description.to_string(), 2).unwrap();
        assert!(matches!(
            &actions[0],
            VoiceGatewayAction::StartVoiceOutput {
                ssrc: 42,
                mode: VoiceEncryptionMode::AeadAes256GcmRtpSize,
                secret_key,
                dave_protocol_version: 1
            } if secret_key.iter().all(|b| *b == 7)
        ));
        assert_eq!(m.state(), VoiceGatewayState::Ready);
    }
    #[test]
    fn rejects_out_of_order_messages_and_invalid_key_sizes() {
        let mut m = machine();
        assert!(m.handle_message(r#"{"op":2,"d":{}}"#, 0).is_err());
        let mut m = machine();
        m.handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 0)
            .unwrap();
        m.handle_message(
            r#"{"op":2,"d":{"ssrc":42,"ip":"203.0.113.10","port":50000,"modes":["xsalsa20_poly1305_lite"]}}"#,
            1,
        ).unwrap();
        m.on_udp_discovered("198.51.100.4", 40000).unwrap();
        assert!(
            m.handle_message(
                r#"{"op":4,"d":{"mode":"xsalsa20_poly1305_lite","secret_key":[1,2,3]}}"#,
                2
            )
            .is_err()
        );
    }
    #[test]
    fn dave_json_events_and_binary_mls_frames_are_dispatched_separately() {
        let mut m = machine();
        assert!(m.handle_binary_message(&[0, 1, 25, 9]).is_err());
        m.handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 0)
            .unwrap();
        m.handle_message(
            r#"{"op":2,"d":{"ssrc":42,"ip":"203.0.113.10","port":50000,"modes":["aead_aes256_gcm_rtpsize"]}}"#,
            1,
        )
        .unwrap();
        m.on_udp_discovered("198.51.100.4", 40000).unwrap();
        m.handle_message(
            &json!({"op":4,"d":{"mode":"aead_aes256_gcm_rtpsize","secret_key":vec![1;32]}})
                .to_string(),
            2,
        )
        .unwrap();
        let actions = m
            .handle_message(
                r#"{"op":21,"d":{"transition_id":0,"protocol_version":1}}"#,
                3,
            )
            .unwrap();
        assert!(matches!(
            actions.as_slice(),
            [VoiceGatewayAction::DaveEvent { opcode: 21, .. }]
        ));
        let actions = m
            .handle_binary_message(&[0x12, 0x34, 29, 0xaa, 0xbb])
            .unwrap();
        assert_eq!(
            actions,
            vec![VoiceGatewayAction::DaveBinaryEvent {
                sequence: 0x1234,
                opcode: 29,
                payload: vec![0xaa, 0xbb],
            }]
        );
        assert_eq!(m.heartbeat(4)["d"]["seq_ack"], 0x1234);
    }

    #[test]
    fn tracks_clients_connect_and_disconnect_events() {
        let mut m = machine();
        let connected = m
            .handle_message(r#"{"op":11,"d":{"user_ids":["111","222"]}}"#, 0)
            .unwrap();
        assert_eq!(
            connected,
            vec![VoiceGatewayAction::ClientsConnect {
                user_ids: vec![111, 222],
            }]
        );
        assert_eq!(m.connected_user_ids(), &[111, 222]);

        let disconnected = m
            .handle_message(r#"{"op":13,"d":{"user_id":"111"}}"#, 1)
            .unwrap();
        assert_eq!(
            disconnected,
            vec![VoiceGatewayAction::ClientDisconnect { user_id: 111 }]
        );
        assert_eq!(m.connected_user_ids(), &[222]);
    }

    #[test]
    fn rejects_ipv6_until_voice_discovery_supports_it() {
        let mut m = machine();
        m.handle_message(r#"{"op":8,"d":{"heartbeat_interval":10000}}"#, 0)
            .unwrap();
        assert!(m.handle_message(
            r#"{"op":2,"d":{"ssrc":42,"ip":"2001:db8::1","port":50000,"modes":["aead_aes256_gcm_rtpsize"]}}"#,
            1,
        ).is_err());
    }
    #[test]
    fn only_known_encryption_modes_are_selected() {
        assert_eq!(
            choose_encryption_mode(&["unknown".into(), "xsalsa20_poly1305".into()]),
            Some(VoiceEncryptionMode::Xsalsa20Poly1305)
        );
        assert_eq!(choose_encryption_mode(&["unknown".into()]), None);
    }
    #[test]
    fn validates_gateway_endpoint() {
        assert_eq!(
            voice_gateway_url("eu-central123.discord.media:443").unwrap(),
            "wss://eu-central123.discord.media:443/?v=8"
        );
        assert!(voice_gateway_url("wss://evil.example/path").is_err());
        assert!(voice_gateway_url("user@host").is_err());
        assert!(voice_gateway_url("host.example/path").is_err());
        assert!(voice_gateway_url("host.example?query=1").is_err());
        assert!(voice_gateway_url("host.example#fragment").is_err());
        assert!(voice_gateway_url("host with spaces.example").is_err());
        assert!(voice_gateway_url("host.example\\\\evil").is_err());
    }
}
