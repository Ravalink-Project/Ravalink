//! Discord DAVE end-to-end voice encryption session.
//!
//! This module wraps the upstream Rust `davey` MLS implementation and exposes
//! the state transitions needed by the Discord voice gateway adapter.

use davey::{
    CommitWelcome, DAVE_PROTOCOL_VERSION, DaveSession, ProposalsOperationType,
};
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
    fn rejects_unknown_proposal_operations() {
        let mut session = DaveVoiceSession::new(1234, 5678).unwrap();
        assert!(session.process_proposals(7, &[], &[]).is_err());
    }
}
