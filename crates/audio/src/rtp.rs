//! RTP header and sequence/timestamp construction for Opus voice packets.
//!
//! Encryption and UDP transport are deliberately separate so packet framing
//! can be tested independently of Discord voice negotiation.

use crate::AudioError;

pub const RTP_HEADER_BYTES: usize = 12;
pub const RTP_OPUS_PAYLOAD_TYPE: u8 = 120;
pub const OPUS_SAMPLES_PER_PACKET: u32 = 960;
pub const MAX_OPUS_PACKET_BYTES: usize = 1_275;

#[derive(Debug, Clone)]
pub struct RtpPacketizer {
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
}

impl RtpPacketizer {
    pub fn new(ssrc: u32, initial_sequence: u16, initial_timestamp: u32) -> Self {
        Self {
            ssrc,
            sequence: initial_sequence,
            timestamp: initial_timestamp,
        }
    }

    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    pub fn sequence(&self) -> u16 {
        self.sequence
    }

    pub fn timestamp(&self) -> u32 {
        self.timestamp
    }

    /// Construct an RTP v2 packet containing one Opus frame.
    ///
    /// The payload type and timestamp increment follow Discord's 20 ms
    /// 48 kHz stereo voice packet cadence. The returned packet is not encrypted.
    pub fn packetize(&mut self, opus_payload: &[u8]) -> Result<Vec<u8>, AudioError> {
        if opus_payload.is_empty() {
            return Err(AudioError::invalid("Opus RTP payload cannot be empty"));
        }
        if opus_payload.len() > MAX_OPUS_PACKET_BYTES {
            return Err(AudioError::invalid("Opus RTP payload exceeds 1275 bytes"));
        }

        let mut packet = Vec::with_capacity(RTP_HEADER_BYTES + opus_payload.len());
        packet.push(0x80); // RTP version 2, no padding, extension or CSRC list.
        packet.push(RTP_OPUS_PAYLOAD_TYPE & 0x7f);
        packet.extend_from_slice(&self.sequence.to_be_bytes());
        packet.extend_from_slice(&self.timestamp.to_be_bytes());
        packet.extend_from_slice(&self.ssrc.to_be_bytes());
        packet.extend_from_slice(opus_payload);

        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(OPUS_SAMPLES_PER_PACKET);
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_rtp_v2_header_and_appends_opus_payload() {
        let mut packetizer = RtpPacketizer::new(0x1234_5678, 0x0102, 0x0304_0506);
        let packet = packetizer.packetize(&[0xaa, 0xbb, 0xcc]).unwrap();

        assert_eq!(&packet[..12], &[
            0x80, 120, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x12, 0x34, 0x56, 0x78,
        ]);
        assert_eq!(&packet[12..], &[0xaa, 0xbb, 0xcc]);
        assert_eq!(packetizer.sequence(), 0x0103);
        assert_eq!(packetizer.timestamp(), 0x0304_08c6);
    }

    #[test]
    fn sequence_and_timestamp_wrap_without_panicking() {
        let mut packetizer = RtpPacketizer::new(1, u16::MAX, u32::MAX - 500);
        packetizer.packetize(&[1]).unwrap();
        assert_eq!(packetizer.sequence(), 0);
        assert_eq!(packetizer.timestamp(), 459);
    }

    #[test]
    fn rejects_empty_or_oversized_opus_payloads() {
        let mut packetizer = RtpPacketizer::new(1, 0, 0);
        assert!(packetizer.packetize(&[]).is_err());
        assert!(packetizer.packetize(&vec![0; MAX_OPUS_PACKET_BYTES + 1]).is_err());
    }
}
