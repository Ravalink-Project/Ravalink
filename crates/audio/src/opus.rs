//! Native Opus encoding for fixed-size Discord voice frames.

use crate::{AudioError, PcmFrame};
use opus2::{Application, Bitrate, Channels, Encoder};

const OPUS_MAX_PACKET_BYTES: usize = 4_000;

/// Stateful stereo 48 kHz Opus encoder.
///
/// Each call encodes one 20 ms frame (960 samples per channel), as required by
/// the Discord voice packetization path.
pub struct OpusFrameEncoder {
    encoder: Encoder,
    bitrate_bps: u32,
    complexity: u8,
}

impl OpusFrameEncoder {
    pub fn new(bitrate_bps: u32, complexity: u8) -> Result<Self, AudioError> {
        if !(6_000..=512_000).contains(&bitrate_bps) {
            return Err(AudioError::invalid(
                "Opus bitrate must be between 6000 and 512000 bits per second",
            ));
        }
        if complexity > 10 {
            return Err(AudioError::invalid(
                "Opus complexity must be between 0 and 10",
            ));
        }

        let mut encoder = Encoder::new(48_000, Channels::Stereo, Application::Audio)
            .map_err(|error| AudioError::invalid(format!("Opus encoder init failed: {error}")))?;
        encoder
            .set_bitrate(Bitrate::Bits(bitrate_bps as i32))
            .map_err(|error| AudioError::invalid(format!("Opus bitrate setup failed: {error}")))?;
        encoder
            .set_complexity(i32::from(complexity))
            .map_err(|error| {
                AudioError::invalid(format!("Opus complexity setup failed: {error}"))
            })?;

        Ok(Self {
            encoder,
            bitrate_bps,
            complexity,
        })
    }

    pub fn bitrate(&self) -> u32 {
        self.bitrate_bps
    }

    pub fn complexity(&self) -> u8 {
        self.complexity
    }

    /// Encode a 20 ms stereo 48 kHz frame into the caller's reusable buffer.
    pub fn encode_into(
        &mut self,
        frame: &PcmFrame,
        output: &mut [u8],
    ) -> Result<usize, AudioError> {
        if frame.sample_rate != 48_000 || frame.channels != 2 || frame.samples.len() != 1_920 {
            return Err(AudioError::invalid(
                "Opus input must be a 20 ms stereo 48 kHz PCM frame",
            ));
        }
        if output.len() < OPUS_MAX_PACKET_BYTES {
            return Err(AudioError::invalid(
                "Opus output buffer must contain at least 4000 bytes",
            ));
        }

        self.encoder
            .encode_float(&frame.samples, output)
            .map_err(|error| AudioError::invalid(format!("Opus encoding failed: {error}")))
    }

    pub fn encode(&mut self, frame: &PcmFrame) -> Result<Vec<u8>, AudioError> {
        let mut output = vec![0; OPUS_MAX_PACKET_BYTES];
        let length = self.encode_into(frame, &mut output)?;
        output.truncate(length);
        Ok(output)
    }

    pub fn reset(&mut self) -> Result<(), AudioError> {
        self.encoder
            .reset_state()
            .map_err(|error| AudioError::invalid(format!("Opus encoder reset failed: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice_frame() -> PcmFrame {
        PcmFrame {
            samples: (0..1_920)
                .map(|index| {
                    let phase = index as f32 * std::f32::consts::TAU * 440.0 / 48_000.0;
                    phase.sin() * 0.2
                })
                .collect(),
            channels: 2,
            sample_rate: 48_000,
        }
    }

    #[test]
    fn encodes_a_voice_frame_to_a_valid_opus_packet() {
        let mut encoder = OpusFrameEncoder::new(128_000, 10).unwrap();
        let packet = encoder.encode(&voice_frame()).unwrap();
        assert!(!packet.is_empty());
        assert!(packet.len() <= OPUS_MAX_PACKET_BYTES);
        assert_eq!(encoder.bitrate(), 128_000);
        assert_eq!(encoder.complexity(), 10);
    }

    #[test]
    fn rejects_invalid_opus_configuration() {
        assert!(OpusFrameEncoder::new(5_000, 5).is_err());
        assert!(OpusFrameEncoder::new(128_000, 11).is_err());
    }

    #[test]
    fn rejects_non_voice_pcm_and_small_output_buffers() {
        let mut encoder = OpusFrameEncoder::new(128_000, 5).unwrap();
        let invalid = PcmFrame {
            samples: vec![0.0; 960],
            channels: 1,
            sample_rate: 48_000,
        };
        assert!(encoder.encode(&invalid).is_err());
        assert!(encoder.encode_into(&voice_frame(), &mut [0; 128]).is_err());
    }
}
