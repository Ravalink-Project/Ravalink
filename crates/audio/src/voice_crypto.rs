//! Discord voice RTP transport encryption.
//!
//! Supports the modern RTP-size AEAD modes required by Discord and retains
//! legacy XSalsa modes for compatibility with older gateways.

use crate::{AudioError, RtpPacketizer};
use aes_gcm::{
    Aes256Gcm, Nonce as AesNonce,
    aead::{Aead, KeyInit, Payload},
};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use crypto_secretbox::{Nonce as SalsaNonce, XSalsa20Poly1305};

const RTP_HEADER_BYTES: usize = 12;

/// Discord voice transport encryption modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEncryptionMode {
    AeadAes256GcmRtpSize,
    AeadXChaCha20Poly1305RtpSize,
    Xsalsa20Poly1305LiteRtpSize,
    Xsalsa20Poly1305,
    Xsalsa20Poly1305Suffix,
    Xsalsa20Poly1305Lite,
}

enum VoiceCipher {
    Aes256Gcm(Aes256Gcm),
    XChaCha20Poly1305(XChaCha20Poly1305),
    Xsalsa20Poly1305(XSalsa20Poly1305),
}

/// Stateful RTP encryptor. Create a new instance for each negotiated voice key.
pub struct VoicePacketEncryptor {
    cipher: VoiceCipher,
    mode: VoiceEncryptionMode,
    nonce_counter: u32,
}

impl VoicePacketEncryptor {
    pub fn new(secret_key: [u8; 32], mode: VoiceEncryptionMode) -> Result<Self, AudioError> {
        let cipher = match mode {
            VoiceEncryptionMode::AeadAes256GcmRtpSize => {
                VoiceCipher::Aes256Gcm(Aes256Gcm::new_from_slice(&secret_key).map_err(|error| {
                    AudioError::invalid(format!("invalid AES-256-GCM voice key: {error}"))
                })?)
            }
            VoiceEncryptionMode::AeadXChaCha20Poly1305RtpSize => VoiceCipher::XChaCha20Poly1305(
                XChaCha20Poly1305::new_from_slice(&secret_key).map_err(|error| {
                    AudioError::invalid(format!("invalid XChaCha20 voice key: {error}"))
                })?,
            ),
            _ => VoiceCipher::Xsalsa20Poly1305(
                XSalsa20Poly1305::new_from_slice(&secret_key).map_err(|error| {
                    AudioError::invalid(format!("invalid XSalsa20 voice key: {error}"))
                })?,
            ),
        };
        Ok(Self {
            cipher,
            mode,
            nonce_counter: 0,
        })
    }

    pub fn mode(&self) -> VoiceEncryptionMode {
        self.mode
    }

    /// Encrypt an RTP packet and append the negotiated mode's nonce suffix.
    pub fn encrypt_rtp_packet(&mut self, packet: &[u8]) -> Result<Vec<u8>, AudioError> {
        if packet.len() <= RTP_HEADER_BYTES || packet[0] >> 6 != 2 {
            return Err(AudioError::invalid(
                "voice encryption requires a valid RTP v2 packet",
            ));
        }

        let header = &packet[..RTP_HEADER_BYTES];
        let plaintext = &packet[RTP_HEADER_BYTES..];
        let mut output = Vec::with_capacity(packet.len() + 20);
        output.extend_from_slice(header);

        match (&self.cipher, self.mode) {
            (VoiceCipher::Aes256Gcm(cipher), VoiceEncryptionMode::AeadAes256GcmRtpSize) => {
                let nonce_bytes = nonce_bytes::<12>(self.nonce_counter);
                let ciphertext = cipher
                    .encrypt(
                        AesNonce::from_slice(&nonce_bytes),
                        Payload {
                            msg: plaintext,
                            aad: header,
                        },
                    )
                    .map_err(|_| AudioError::invalid("AES-256-GCM voice encryption failed"))?;
                output.extend_from_slice(&ciphertext);
                output.extend_from_slice(&self.nonce_counter.to_be_bytes());
                self.advance_nonce()?;
            }
            (
                VoiceCipher::XChaCha20Poly1305(cipher),
                VoiceEncryptionMode::AeadXChaCha20Poly1305RtpSize,
            ) => {
                let nonce_bytes = nonce_bytes::<24>(self.nonce_counter);
                let ciphertext = cipher
                    .encrypt(
                        XNonce::from_slice(&nonce_bytes),
                        Payload {
                            msg: plaintext,
                            aad: header,
                        },
                    )
                    .map_err(|_| {
                        AudioError::invalid("XChaCha20-Poly1305 voice encryption failed")
                    })?;
                output.extend_from_slice(&ciphertext);
                output.extend_from_slice(&self.nonce_counter.to_be_bytes());
                self.advance_nonce()?;
            }
            (
                VoiceCipher::Xsalsa20Poly1305(cipher),
                VoiceEncryptionMode::Xsalsa20Poly1305Lite
                | VoiceEncryptionMode::Xsalsa20Poly1305LiteRtpSize,
            ) => {
                let nonce_bytes = nonce_bytes::<24>(self.nonce_counter);
                let nonce = SalsaNonce::from_slice(&nonce_bytes);
                let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|_| {
                    AudioError::invalid("XSalsa20-Poly1305 voice encryption failed")
                })?;
                output.extend_from_slice(&ciphertext);
                output.extend_from_slice(&self.nonce_counter.to_be_bytes());
                self.advance_nonce()?;
            }
            (VoiceCipher::Xsalsa20Poly1305(cipher), VoiceEncryptionMode::Xsalsa20Poly1305) => {
                let mut nonce_bytes = [0_u8; 24];
                nonce_bytes[..RTP_HEADER_BYTES].copy_from_slice(header);
                let nonce = SalsaNonce::from_slice(&nonce_bytes);
                let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|_| {
                    AudioError::invalid("XSalsa20-Poly1305 voice encryption failed")
                })?;
                output.extend_from_slice(&ciphertext);
            }
            (
                VoiceCipher::Xsalsa20Poly1305(cipher),
                VoiceEncryptionMode::Xsalsa20Poly1305Suffix,
            ) => {
                let mut nonce_bytes = [0_u8; 24];
                getrandom::fill(&mut nonce_bytes).map_err(|error| {
                    AudioError::invalid(format!("nonce generation failed: {error}"))
                })?;
                let nonce = SalsaNonce::from_slice(&nonce_bytes);
                let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|_| {
                    AudioError::invalid("XSalsa20-Poly1305 voice encryption failed")
                })?;
                output.extend_from_slice(&ciphertext);
                output.extend_from_slice(&nonce_bytes);
            }
            _ => {
                return Err(AudioError::invalid(
                    "voice encryption cipher does not match negotiated mode",
                ));
            }
        }

        Ok(output)
    }

    fn advance_nonce(&mut self) -> Result<(), AudioError> {
        self.nonce_counter = self
            .nonce_counter
            .checked_add(1)
            .ok_or_else(|| AudioError::invalid("voice nonce counter exhausted"))?;
        Ok(())
    }
}

/// Combines RTP sequence/timestamp framing with the negotiated encryptor.
pub struct VoicePacketPipeline {
    packetizer: RtpPacketizer,
    encryptor: VoicePacketEncryptor,
}

impl VoicePacketPipeline {
    pub fn new(
        ssrc: u32,
        initial_sequence: u16,
        initial_timestamp: u32,
        secret_key: [u8; 32],
        mode: VoiceEncryptionMode,
    ) -> Result<Self, AudioError> {
        Ok(Self {
            packetizer: RtpPacketizer::new(ssrc, initial_sequence, initial_timestamp),
            encryptor: VoicePacketEncryptor::new(secret_key, mode)?,
        })
    }

    pub fn packetize_opus(&mut self, opus_payload: &[u8]) -> Result<Vec<u8>, AudioError> {
        let rtp_packet = self.packetizer.packetize(opus_payload)?;
        self.encryptor.encrypt_rtp_packet(&rtp_packet)
    }
}

fn nonce_bytes<const N: usize>(counter: u32) -> [u8; N] {
    let mut nonce = [0_u8; N];
    nonce[..4].copy_from_slice(&counter.to_be_bytes());
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RtpPacketizer;

    fn rtp_packet() -> Vec<u8> {
        RtpPacketizer::new(0x1234_5678, 42, 960)
            .packetize(&[0x11, 0x22, 0x33, 0x44])
            .unwrap()
    }

    #[test]
    fn lite_mode_appends_a_monotonic_nonce_counter() {
        let mut encryptor =
            VoicePacketEncryptor::new([7; 32], VoiceEncryptionMode::Xsalsa20Poly1305Lite).unwrap();
        let first = encryptor.encrypt_rtp_packet(&rtp_packet()).unwrap();
        let second = encryptor.encrypt_rtp_packet(&rtp_packet()).unwrap();

        assert_eq!(&first[first.len() - 4..], &[0, 0, 0, 0]);
        assert_eq!(&second[second.len() - 4..], &[0, 0, 0, 1]);
        assert_eq!(&first[..12], &rtp_packet()[..12]);
    }

    #[test]
    fn aes256_gcm_rtpsize_round_trips_and_appends_nonce_counter() {
        let key = [9; 32];
        let packet = rtp_packet();
        let mut encryptor =
            VoicePacketEncryptor::new(key, VoiceEncryptionMode::AeadAes256GcmRtpSize).unwrap();
        let encrypted = encryptor.encrypt_rtp_packet(&packet).unwrap();

        assert_eq!(&encrypted[..12], &packet[..12]);
        assert_eq!(&encrypted[encrypted.len() - 4..], &[0, 0, 0, 0]);
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = nonce_bytes::<12>(0);
        let plaintext = cipher
            .decrypt(
                AesNonce::from_slice(&nonce),
                Payload {
                    msg: &encrypted[12..encrypted.len() - 4],
                    aad: &packet[..12],
                },
            )
            .unwrap();
        assert_eq!(plaintext, packet[12..]);
    }

    #[test]
    fn xchacha20_poly1305_rtpsize_round_trips_and_appends_nonce_counter() {
        let key = [3; 32];
        let packet = rtp_packet();
        let mut encryptor =
            VoicePacketEncryptor::new(key, VoiceEncryptionMode::AeadXChaCha20Poly1305RtpSize)
                .unwrap();
        let encrypted = encryptor.encrypt_rtp_packet(&packet).unwrap();

        assert_eq!(&encrypted[..12], &packet[..12]);
        assert_eq!(&encrypted[encrypted.len() - 4..], &[0, 0, 0, 0]);
        let cipher = XChaCha20Poly1305::new_from_slice(&key).unwrap();
        let nonce = nonce_bytes::<24>(0);
        let plaintext = cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &encrypted[12..encrypted.len() - 4],
                    aad: &packet[..12],
                },
            )
            .unwrap();
        assert_eq!(plaintext, packet[12..]);
    }

    #[test]
    fn normal_mode_uses_the_rtp_header_as_its_nonce_prefix() {
        let key = [9; 32];
        let packet = rtp_packet();
        let mut encryptor =
            VoicePacketEncryptor::new(key, VoiceEncryptionMode::Xsalsa20Poly1305).unwrap();
        let encrypted = encryptor.encrypt_rtp_packet(&packet).unwrap();

        let mut nonce_bytes = [0_u8; 24];
        nonce_bytes[..12].copy_from_slice(&packet[..12]);
        let cipher = XSalsa20Poly1305::new_from_slice(&key).unwrap();
        let plaintext = cipher
            .decrypt(SalsaNonce::from_slice(&nonce_bytes), &encrypted[12..])
            .unwrap();
        assert_eq!(plaintext, packet[12..]);
    }

    #[test]
    fn suffix_mode_appends_a_fresh_24_byte_nonce() {
        let mut encryptor =
            VoicePacketEncryptor::new([5; 32], VoiceEncryptionMode::Xsalsa20Poly1305Suffix)
                .unwrap();
        let packet = rtp_packet();
        let first = encryptor.encrypt_rtp_packet(&packet).unwrap();
        let second = encryptor.encrypt_rtp_packet(&packet).unwrap();

        assert_eq!(first.len(), packet.len() + 16 + 24);
        assert_eq!(second.len(), packet.len() + 16 + 24);
        assert_ne!(&first[first.len() - 24..], &second[second.len() - 24..]);
    }

    #[test]
    fn voice_packet_pipeline_frames_and_encrypts_opus_payloads() {
        let mut pipeline = VoicePacketPipeline::new(
            0x1234_5678,
            42,
            960,
            [3; 32],
            VoiceEncryptionMode::AeadXChaCha20Poly1305RtpSize,
        )
        .unwrap();
        let packet = pipeline.packetize_opus(&[0xaa, 0xbb, 0xcc]).unwrap();

        assert_eq!(packet[0], 0x80);
        assert_eq!(packet[1], 120);
        assert_eq!(&packet[8..12], &0x1234_5678_u32.to_be_bytes());
        assert_eq!(&packet[packet.len() - 4..], &[0, 0, 0, 0]);
        assert!(packet.len() > 12 + 16 + 4);
    }

    #[test]
    fn rejects_non_rtp_input() {
        let mut encryptor =
            VoicePacketEncryptor::new([1; 32], VoiceEncryptionMode::AeadXChaCha20Poly1305RtpSize)
                .unwrap();
        assert!(encryptor.encrypt_rtp_packet(&[1, 2, 3]).is_err());
    }
}
