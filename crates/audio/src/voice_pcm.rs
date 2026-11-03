//! PCM normalization and fixed-duration packetization for Discord voice output.
//!
//! Opus expects 48 kHz audio. This module converts decoded PCM into stereo
//! 48 kHz samples and groups it into 20 ms frames (960 samples per channel).
//! It intentionally stops before Opus encoding and RTP transport.

use crate::{AudioError, PcmFrame};
use std::collections::VecDeque;

pub const VOICE_SAMPLE_RATE: u32 = 48_000;
pub const VOICE_CHANNELS: u16 = 2;
pub const VOICE_FRAME_DURATION_MS: u32 = 20;
pub const VOICE_FRAME_SAMPLES_PER_CHANNEL: usize =
    (VOICE_SAMPLE_RATE as usize * VOICE_FRAME_DURATION_MS as usize) / 1_000;
const VOICE_FRAME_SAMPLE_COUNT: usize =
    VOICE_FRAME_SAMPLES_PER_CHANNEL * VOICE_CHANNELS as usize;

/// Convert an interleaved PCM frame to stereo 48 kHz floating-point PCM.
///
/// Mono input is duplicated to both output channels. Multichannel input uses
/// its first two channels; callers can apply the channel-mix filter beforehand.
pub fn normalize_for_voice(frame: &PcmFrame) -> Result<PcmFrame, AudioError> {
    if frame.channels == 0 {
        return Err(AudioError::invalid("PCM frame must have at least one channel"));
    }
    if frame.sample_rate == 0 {
        return Err(AudioError::invalid("PCM sample rate must be greater than zero"));
    }
    let input_channels = usize::from(frame.channels);
    if !frame.samples.len().is_multiple_of(input_channels) {
        return Err(AudioError::invalid(
            "interleaved PCM sample count is not channel-aligned",
        ));
    }

    let input_frames = frame.samples.len() / input_channels;
    if input_frames == 0 {
        return Ok(PcmFrame {
            samples: Vec::new(),
            channels: VOICE_CHANNELS,
            sample_rate: VOICE_SAMPLE_RATE,
        });
    }

    let output_frames = ((input_frames as u128 * u128::from(VOICE_SAMPLE_RATE)
        + u128::from(frame.sample_rate) / 2)
        / u128::from(frame.sample_rate)) as usize;
    let mut output = Vec::with_capacity(output_frames.saturating_mul(2));

    for output_index in 0..output_frames {
        let source_position =
            output_index as f64 * f64::from(frame.sample_rate) / f64::from(VOICE_SAMPLE_RATE);
        let left_index = (source_position.floor() as usize).min(input_frames - 1);
        let right_index = (left_index + 1).min(input_frames - 1);
        let fraction = (source_position - left_index as f64) as f32;

        let left_start = left_index * input_channels;
        let right_start = right_index * input_channels;
        let left_a = frame.samples[left_start];
        let left_b = frame.samples[right_start];
        let left = left_a + (left_b - left_a) * fraction;

        let right = if input_channels == 1 {
            left
        } else {
            let right_a = frame.samples[left_start + 1];
            let right_b = frame.samples[right_start + 1];
            right_a + (right_b - right_a) * fraction
        };

        output.push(left.clamp(-1.0, 1.0));
        output.push(right.clamp(-1.0, 1.0));
    }

    Ok(PcmFrame {
        samples: output,
        channels: VOICE_CHANNELS,
        sample_rate: VOICE_SAMPLE_RATE,
    })
}

/// Accumulates normalized PCM and yields exact 20 ms stereo voice frames.
#[derive(Debug, Default)]
pub struct VoicePcmPacketizer {
    samples: VecDeque<f32>,
}

impl VoicePcmPacketizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, frame: &PcmFrame) -> Result<usize, AudioError> {
        let normalized = normalize_for_voice(frame)?;
        self.samples.extend(normalized.samples);
        Ok(self.pending_frames())
    }

    pub fn pop_frame(&mut self) -> Option<PcmFrame> {
        if self.samples.len() < VOICE_FRAME_SAMPLE_COUNT {
            return None;
        }
        let samples = self
            .samples
            .drain(..VOICE_FRAME_SAMPLE_COUNT)
            .collect();
        Some(PcmFrame {
            samples,
            channels: VOICE_CHANNELS,
            sample_rate: VOICE_SAMPLE_RATE,
        })
    }

    pub fn pending_frames(&self) -> usize {
        self.samples.len() / VOICE_FRAME_SAMPLE_COUNT
    }

    pub fn buffered_samples(&self) -> usize {
        self.samples.len()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(samples: Vec<f32>, channels: u16, sample_rate: u32) -> PcmFrame {
        PcmFrame {
            samples,
            channels,
            sample_rate,
        }
    }

    #[test]
    fn mono_audio_is_duplicated_to_stereo() {
        let output = normalize_for_voice(&frame(vec![0.25, -0.25], 1, 48_000)).unwrap();
        assert_eq!(output.channels, 2);
        assert_eq!(output.sample_rate, 48_000);
        assert_eq!(output.samples, vec![0.25, 0.25, -0.25, -0.25]);
    }

    #[test]
    fn resamples_44100_hz_to_48000_hz() {
        let input = frame(vec![0.5; 441], 1, 44_100);
        let output = normalize_for_voice(&input).unwrap();
        assert_eq!(output.samples.len(), 960);
        assert_eq!(output.channels, 2);
        assert_eq!(output.sample_rate, 48_000);
    }

    #[test]
    fn packetizer_emits_exact_twenty_millisecond_frames() {
        let mut packetizer = VoicePcmPacketizer::new();
        let first = frame(vec![0.1; 960], 2, 48_000);
        assert_eq!(packetizer.push(&first).unwrap(), 0);
        let second = frame(vec![0.2; 960], 2, 48_000);
        assert_eq!(packetizer.push(&second).unwrap(), 1);

        let packet = packetizer.pop_frame().unwrap();
        assert_eq!(packet.samples.len(), VOICE_FRAME_SAMPLE_COUNT);
        assert_eq!(packet.channels, 2);
        assert_eq!(packet.sample_rate, 48_000);
        assert_eq!(packetizer.pop_frame(), None);
        assert_eq!(packetizer.buffered_samples(), 0);
    }

    #[test]
    fn packetizer_preserves_partial_frame_between_pushes() {
        let mut packetizer = VoicePcmPacketizer::new();
        let half_frame = frame(vec![0.1; 960], 2, 48_000);
        packetizer.push(&half_frame).unwrap();
        assert_eq!(packetizer.pending_frames(), 0);
        packetizer.push(&half_frame).unwrap();
        assert_eq!(packetizer.pending_frames(), 1);
    }

    #[test]
    fn rejects_invalid_pcm_layouts() {
        assert!(normalize_for_voice(&frame(vec![0.1], 0, 48_000)).is_err());
        assert!(normalize_for_voice(&frame(vec![0.1], 1, 0)).is_err());
        assert!(normalize_for_voice(&frame(vec![0.1], 2, 48_000)).is_err());
    }
}
