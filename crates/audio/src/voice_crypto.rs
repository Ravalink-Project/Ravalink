//! Legacy Discord voice RTP encryption modes (XSalsa20-Poly1305).

use crate::AudioError;
use crypto_secretbox::{
    aead::{Aead, KeyInit},
    Nonce, XSalsa20Poly1305,
};

const RTP_HEADER_BYTES: usize = 12;

/// Legacy encryption modes supported by Discord voice servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEncryptionMode {
    Xsalsa20Poly1305,
    Xsalsa20Poly1305Suffix,
    Xsalsa20Poly1305Lite,
}

/// Stateful RTP encryptor. Create a new instance for each negotiated voice key.
pub struct VoicePacketEncryptor {
    cipher: XSalsa20Poly1305,
    mode: VoiceEncryptionMode,
    lite_nonce: u32,
}

impl VoicePacketEncryptor {
    pub fn new(
        secret_key: [u8; 32],
        mode: VoiceEncryptionMode,
    ) -> Result<Self, AudioError> {
        let cipher = XSalsa20Poly1305::new_from_slice(&secret_key)
            .map_err(|error| AudioError::invalid(format!("invalid voice secret key: {error}")))?;
        Ok(Self {
            cipher,
            mode,
            lite_nonce: 0,
        })
    }

    pub fn mode(&self) -> VoiceEncryptionMode {
        self.mode
    }

    /// Encrypt an RTP packet and append the negotiated mode's nonce suffix.
    pub fn encrypt_rtp_packet(&mut self, packet: &[u8]) -> Result<Vec<u8>, AudioError> {
        if packet.len() <= RTP_HEADER_BYTES || packet[0] >> 6 != 2 {
            return Err(AudioError::invalid("voice encryption requires a valid RTP v2 packet"));
        }

        let header = &packet[..RTP_HEADER_BYTES];
        let plaintext = &packet[RTP_HEADER_BYTES..];
        let mut output = Vec::with_capacity(packet.len() + 40);
        output.extend_from_slice(header);

        match self.mode {
            VoiceEncryptionMode::Xsalsa20Poly1305 => {
                let mut nonce_bytes = [0_u8; 24];
                nonce_bytes[..RTP_HEADER_BYTES].copy_from_slice(header);
                let nonce = Nonce::from_slice(&nonce_bytes);
                let ciphertext = self
                    .cipher
                    .encrypt(nonce, plaintext)
                    .map_err(|_| AudioError::invalid("voice RTP encryption failed"))?;
                output.extend_from_slice(&ciphertext);
            }
            VoiceEncryptionMode::Xsalsa20Poly1305Suffix => {
                let mut nonce_bytes = [0_u8; 24];
                getrandom::fill(&mut nonce_bytes)
                    .map_err(|error| AudioError::invalid(format!("nonce generation failed: {error}")))?;
                let nonce = Nonce::from_slice(&nonce_bytes);
                let ciphertext = self
                    .cipher
                    .encrypt(nonce, plaintext)
                    .map_err(|_| AudioError::invalid("voice RTP encryption failed"))?;
                output.extend_from_slice(&ciphertext);
                output.extend_from_slice(&nonce_bytes);
            }
            VoiceEncryptionMode::Xsalsa20Poly1305Lite => {
                let next_nonce = self
                    .lite_nonce
                    .checked_add(1)
                    .ok_or_else(|| AudioError::invalid("voice lite nonce counter exhausted"))?;
                let nonce_bytes = lite_nonce_bytes(self.lite_nonce);
                let nonce = Nonce::from_slice(&nonce_bytes);
                let ciphertext = self
                    .cipher
                    .encrypt(nonce, plaintext)
                    .map_err(|_| AudioError::invalid("voice RTP encryption failed"))?;
                output.extend_from_slice(&ciphertext);
                output.extend_from_slice(&self.lite_nonce.to_be_bytes());
                self.lite_nonce = next_nonce;
            }
        }

        Ok(output)
    }
}

fn lite_nonce_bytes(counter: u32) -> [u8; 24] {
    let mut nonce = [0_u8; 24];
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
            VoicePacketEncryptor::new([7; 32], VoiceEncryptionMode::Xsalsa20Poly1305Lite)
                .unwrap();
        let first = encryptor.encrypt_rtp_packet(&rtp_packet()).unwrap();
        let second = encryptor.encrypt_rtp_packet(&rtp_packet()).unwrap();

        assert_eq!(&first[first.len() - 4..], &[0, 0, 0, 0]);
        assert_eq!(&second[second.len() - 4..], &[0, 0, 0, 1]);
        assert_eq!(&first[..12], &rtp_packet()[..12]);
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
            .decrypt(Nonce::from_slice(&nonce_bytes), &encrypted[12..])
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
    fn rejects_non_rtp_input() {
        let mut encryptor =
            VoicePacketEncryptor::new([1; 32], VoiceEncryptionMode::Xsalsa20Poly1305Lite)
                .unwrap();
        assert!(encryptor.encrypt_rtp_packet(&[1, 2, 3]).is_err());
    }
}
