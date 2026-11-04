//! Discord DAVE end-to-end voice encryption session.
//!
//! This module wraps the upstream Rust `davey` MLS implementation and exposes
//! the state transitions needed by the Discord voice gateway adapter.

use davey::{CommitWelcome, DAVE_PROTOCOL_VERSION, DaveSession, ProposalsOperationType};
use serde_json::{Value, json};
use std::{borrow::Cow, num::NonZeroU16};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaveVoiceError {
    pub message: String,
}

impl DaveVoiceError {
    fn from_error(error: impl std::fmt::Display) -> Self {
        Self {
            message: error.to_string(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DaveVoiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DaveVoiceError {}

/// Per-guild DAVE state for the authenticated Discord bot user.
pub struct DaveVoiceSession {
    session: DaveSession,
    user_id: u64,
    channel_id: u64,
    protocol_version: u16,
}

impl DaveVoiceSession {
    pub fn new(user_id: u64, channel_id: u64) -> Result<Self, DaveVoiceError> {
        let version = NonZeroU16::new(DAVE_PROTOCOL_VERSION)
            .ok_or_else(|| DaveVoiceError::invalid("DAVE protocol version must be non-zero"))?;
        let session = DaveSession::new(version, user_id, channel_id, None)
            .map_err(DaveVoiceError::from_error)?;
        Ok(Self {
            session,
            user_id,
            channel_id,
            protocol_version: DAVE_PROTOCOL_VERSION,
        })
    }

    pub fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    pub fn is_ready(&self) -> bool {
        self.session.is_ready()
    }

    /// Process voice-gateway opcode 25 and return the key package for opcode 26.
    pub fn set_external_sender(
        &mut self,
        external_sender: &[u8],
    ) -> Result<Vec<u8>, DaveVoiceError> {
        self.session
            .set_external_sender(external_sender)
            .map_err(DaveVoiceError::from_error)?;
        self.session
            .create_key_package()
            .map_err(DaveVoiceError::from_error)
    }

    /// Process voice-gateway opcode 27.
    ///
    /// Operation 0 appends proposals; operation 1 revokes proposal references.
    /// The expected-user list prevents accepting unexpected MLS members.
    pub fn process_proposals(
        &mut self,
        operation: u8,
        proposals: &[u8],
        expected_user_ids: &[u64],
    ) -> Result<Option<CommitWelcome>, DaveVoiceError> {
        let operation = match operation {
            0 => ProposalsOperationType::APPEND,
            1 => ProposalsOperationType::REVOKE,
            _ => {
                return Err(DaveVoiceError::invalid(format!(
                    "unknown DAVE proposal operation: {operation}"
                )));
            }
        };
        self.session
            .process_proposals(operation, proposals, Some(expected_user_ids))
            .map_err(DaveVoiceError::from_error)
    }

    /// Process voice-gateway opcode 29 and prepare the corresponding epoch transition.
    pub fn process_commit(
        &mut self,
        transition_id: u16,
        commit: &[u8],
    ) -> Result<bool, DaveVoiceError> {
        let should_transition = self
            .session
            .process_commit(commit)
            .map_err(DaveVoiceError::from_error)?;
        if should_transition {
            self.session
                .prepare_transition(transition_id, self.protocol_version)
                .map_err(DaveVoiceError::from_error)?;
        }
        Ok(should_transition)
    }

    /// Process voice-gateway opcode 30 and prepare the corresponding epoch transition.
    pub fn process_welcome(
        &mut self,
        transition_id: u16,
        welcome: &[u8],
        recognized_user_ids: &[u64],
    ) -> Result<(), DaveVoiceError> {
        self.session
            .process_welcome(welcome, Some(recognized_user_ids))
            .map_err(DaveVoiceError::from_error)?;
        self.session
            .prepare_transition(transition_id, self.protocol_version)
            .map_err(DaveVoiceError::from_error)
    }

    /// Process voice-gateway opcode 21.
    pub fn prepare_transition(
        &mut self,
        transition_id: u16,
        protocol_version: u16,
    ) -> Result<(), DaveVoiceError> {
        self.protocol_version = protocol_version;
        self.session
            .prepare_transition(transition_id, protocol_version)
            .map_err(DaveVoiceError::from_error)
    }

    /// Process voice-gateway opcode 22.
    pub fn execute_transition(&mut self, transition_id: u16) -> Result<bool, DaveVoiceError> {
        self.session
            .execute_transition(transition_id)
            .map_err(DaveVoiceError::from_error)
    }

    /// Process voice-gateway opcode 24. An epoch of 1 creates a fresh MLS group.
    pub fn prepare_epoch(
        &mut self,
        epoch: u64,
        protocol_version: u16,
    ) -> Result<Option<Vec<u8>>, DaveVoiceError> {
        self.protocol_version = protocol_version;
        if epoch != 1 || protocol_version == 0 {
            return Ok(None);
        }
        let version = NonZeroU16::new(protocol_version)
            .ok_or_else(|| DaveVoiceError::invalid("DAVE protocol version must be non-zero"))?;
        self.session
            .reinit(version, self.user_id, self.channel_id, None)
            .map_err(DaveVoiceError::from_error)?;
        self.session
            .create_key_package()
            .map(Some)
            .map_err(DaveVoiceError::from_error)
    }

    /// Encrypt an Opus frame with DAVE before transport-level RTP encryption.
    pub fn encrypt_opus<'a>(&mut self, frame: &'a [u8]) -> Result<Cow<'a, [u8]>, DaveVoiceError> {
        if self.protocol_version == 0 {
            return Ok(Cow::Borrowed(frame));
        }
        self.session
            .encrypt_opus(frame)
            .map_err(DaveVoiceError::from_error)
    }

    pub const fn max_protocol_version() -> u16 {
        DAVE_PROTOCOL_VERSION
    }

    pub fn passthrough(&self) -> bool {
        self.protocol_version == 0
    }
}

/// Outbound command generated while processing a Discord DAVE gateway event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaveGatewayCommand {
    Json(Value),
    Binary { opcode: u8, payload: Vec<u8> },
}

/// Owns a DAVE session and translates Discord's JSON/binary gateway events
/// into MLS operations and outbound gateway commands.
pub struct DaveVoiceController {
    session: DaveVoiceSession,
    user_id: u64,
    connected_user_ids: Vec<u64>,
}

impl DaveVoiceController {
    pub fn new(
        user_id: u64,
        channel_id: u64,
        protocol_version: u16,
    ) -> Result<Self, DaveVoiceError> {
        if protocol_version > DAVE_PROTOCOL_VERSION {
            return Err(DaveVoiceError::invalid(format!(
                "DAVE protocol version {protocol_version} is newer than supported version {DAVE_PROTOCOL_VERSION}"
            )));
        }
        let mut session = DaveVoiceSession::new(user_id, channel_id)?;
        if protocol_version == 0 {
            session.prepare_epoch(0, 0)?;
        }
        Ok(Self {
            session,
            user_id,
            connected_user_ids: vec![user_id],
        })
    }

    pub fn session(&self) -> &DaveVoiceSession {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut DaveVoiceSession {
        &mut self.session
    }

    pub fn can_send_audio(&self) -> bool {
        self.session.passthrough() || self.session.is_ready()
    }

    pub fn on_clients_connect(&mut self, user_ids: impl IntoIterator<Item = u64>) {
        for user_id in user_ids {
            if !self.connected_user_ids.contains(&user_id) {
                self.connected_user_ids.push(user_id);
            }
        }
    }

    pub fn on_client_disconnect(&mut self, user_id: u64) {
        self.connected_user_ids
            .retain(|connected| *connected != user_id);
        if !self.connected_user_ids.contains(&self.user_id) {
            self.connected_user_ids.push(self.user_id);
        }
    }

    /// Handle JSON DAVE opcodes 21, 22 and 24.
    pub fn handle_json_event(
        &mut self,
        opcode: u8,
        data: &Value,
    ) -> Result<Vec<DaveGatewayCommand>, DaveVoiceError> {
        match opcode {
            21 => {
                let transition_id = required_u16(data, "transition_id")?;
                let protocol_version = required_u16(data, "protocol_version")?;
                self.session
                    .prepare_transition(transition_id, protocol_version)?;
                if transition_id == 0 {
                    Ok(Vec::new())
                } else {
                    Ok(vec![DaveGatewayCommand::Json(json!({
                        "op": 23,
                        "d": {"transition_id": transition_id}
                    }))])
                }
            }
            22 => {
                let transition_id = required_u16(data, "transition_id")?;
                self.session.execute_transition(transition_id)?;
                Ok(Vec::new())
            }
            24 => {
                let epoch = data.get("epoch").and_then(Value::as_u64).ok_or_else(|| {
                    DaveVoiceError::invalid("DAVE PREPARE_EPOCH is missing a valid epoch")
                })?;
                let protocol_version = required_u16(data, "protocol_version")?;
                match self.session.prepare_epoch(epoch, protocol_version)? {
                    Some(key_package) => Ok(vec![DaveGatewayCommand::Binary {
                        opcode: 26,
                        payload: key_package,
                    }]),
                    None => Ok(Vec::new()),
                }
            }
            _ => Err(DaveVoiceError::invalid(format!(
                "unsupported JSON DAVE gateway opcode: {opcode}"
            ))),
        }
    }

    /// Handle binary DAVE opcodes 25, 27, 29 and 30. The caller strips the
    /// gateway sequence number and opcode before passing the MLS payload.
    pub fn handle_binary_event(
        &mut self,
        opcode: u8,
        payload: &[u8],
    ) -> Result<Vec<DaveGatewayCommand>, DaveVoiceError> {
        match opcode {
            25 => {
                let key_package = self.session.set_external_sender(payload)?;
                Ok(vec![DaveGatewayCommand::Binary {
                    opcode: 26,
                    payload: key_package,
                }])
            }
            27 => {
                let (&operation, proposals) = payload.split_first().ok_or_else(|| {
                    DaveVoiceError::invalid("DAVE MLS_PROPOSALS payload is empty")
                })?;
                match self.session.process_proposals(
                    operation,
                    proposals,
                    &self.connected_user_ids,
                )? {
                    Some(commit_welcome) => {
                        let mut payload = commit_welcome.commit;
                        if let Some(welcome) = commit_welcome.welcome {
                            payload.extend_from_slice(&welcome);
                        }
                        Ok(vec![DaveGatewayCommand::Binary {
                            opcode: 28,
                            payload,
                        }])
                    }
                    None => Ok(Vec::new()),
                }
            }
            29 => {
                let (transition_id, commit) = split_transition_payload(payload)?;
                let should_transition = self.session.process_commit(transition_id, commit)?;
                if should_transition && transition_id != 0 {
                    Ok(vec![DaveGatewayCommand::Json(json!({
                        "op": 23,
                        "d": {"transition_id": transition_id}
                    }))])
                } else {
                    Ok(Vec::new())
                }
            }
            30 => {
                let (transition_id, welcome) = split_transition_payload(payload)?;
                self.session
                    .process_welcome(transition_id, welcome, &self.connected_user_ids)?;
                if transition_id == 0 {
                    Ok(Vec::new())
                } else {
                    Ok(vec![DaveGatewayCommand::Json(json!({
                        "op": 23,
                        "d": {"transition_id": transition_id}
                    }))])
                }
            }
            _ => Err(DaveVoiceError::invalid(format!(
                "unsupported binary DAVE gateway opcode: {opcode}"
            ))),
        }
    }

    pub fn encrypt_opus<'a>(&mut self, frame: &'a [u8]) -> Result<Cow<'a, [u8]>, DaveVoiceError> {
        self.session.encrypt_opus(frame)
    }
}

fn required_u16(data: &Value, field: &str) -> Result<u16, DaveVoiceError> {
    data.get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| {
            DaveVoiceError::invalid(format!("DAVE gateway event is missing a valid {field}",))
        })
}

fn split_transition_payload(payload: &[u8]) -> Result<(u16, &[u8]), DaveVoiceError> {
    let (transition_id, message) = payload.split_first_chunk::<2>().ok_or_else(|| {
        DaveVoiceError::invalid("DAVE MLS transition payload is shorter than its transition ID")
    })?;
    Ok((u16::from_be_bytes(*transition_id), message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_a_pending_dave_session() {
        let session = DaveVoiceSession::new(1234, 5678).unwrap();
        assert_eq!(session.protocol_version(), DAVE_PROTOCOL_VERSION);
        assert!(!session.is_ready());
        assert!(!session.passthrough());
    }

    #[test]
    fn transport_only_epoch_bypasses_dave_encryption() {
        let mut session = DaveVoiceSession::new(1234, 5678).unwrap();
        assert!(session.prepare_epoch(0, 0).unwrap().is_none());
        let frame = [1_u8, 2, 3, 4];
        let encrypted = session.encrypt_opus(&frame).unwrap();
        assert_eq!(encrypted.as_ref(), frame);
        assert!(session.passthrough());
    }

    #[test]
    fn controller_acknowledges_non_initial_transitions() {
        let mut controller = DaveVoiceController::new(1234, 5678, 0).unwrap();
        let commands = controller
            .handle_json_event(21, &json!({"transition_id": 7, "protocol_version": 0}))
            .unwrap();
        assert_eq!(
            commands,
            vec![DaveGatewayCommand::Json(json!({
                "op": 23,
                "d": {"transition_id": 7}
            }))]
        );
        assert!(
            controller
                .handle_json_event(22, &json!({"transition_id": 7}))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn controller_rejects_unsupported_protocol_versions() {
        assert!(DaveVoiceController::new(1234, 5678, DAVE_PROTOCOL_VERSION + 1).is_err());
    }

    #[test]
    fn controller_rejects_malformed_binary_transition_payloads() {
        let mut controller = DaveVoiceController::new(1234, 5678, 1).unwrap();
        assert!(controller.handle_binary_event(29, &[0]).is_err());
        assert!(controller.handle_binary_event(27, &[]).is_err());
    }

    #[test]
    fn rejects_unknown_proposal_operations() {
        let mut session = DaveVoiceSession::new(1234, 5678).unwrap();
        assert!(session.process_proposals(7, &[], &[]).is_err());
    }
}
