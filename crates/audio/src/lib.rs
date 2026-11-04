//! Native, allocation-conscious PCM processing primitives.
//!
//! This crate deliberately has no JVM, Lavaplayer or Java dependency. The DSP
//! processor operates on interleaved floating-point PCM frames. Codec and
//! Discord voice transport adapters can feed frames into this layer.

use crate::voice_pcm::VoicePcmPacketizer;
use ravalink_protocol::{Filters, Omissible};

pub mod dave;
pub mod http_source;
pub mod opus;
pub mod rtp;
pub mod voice_crypto;
pub mod voice_gateway;
pub mod voice_gateway_client;
pub use voice_gateway_client::DiscordVoiceConnection;
pub mod voice_pcm;
pub mod voice_udp;
pub use dave::{DaveGatewayCommand, DaveVoiceController, DaveVoiceError, DaveVoiceSession};
pub use http_source::{CachedHttpSource, HttpSourceManager, ResolvedHttpTrack};
pub use opus::OpusFrameEncoder;
pub use rtp::RtpPacketizer;
pub use voice_crypto::{VoiceEncryptionMode, VoicePacketEncryptor, VoicePacketPipeline};
pub use voice_pcm::{VOICE_FRAME_SAMPLES_PER_CHANNEL, normalize_for_voice};
pub use voice_udp::DiscordUdpTransport;

/// Interleaved floating-point PCM audio.
#[derive(Debug, Clone, PartialEq)]
pub struct PcmFrame {
    pub samples: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
}

/// A source-independent description of a resolved audio item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    pub identifier: String,
    pub title: String,
    pub author: String,
    pub duration_ms: Option<u64>,
    pub is_stream: bool,
}

/// Lifecycle contract for a native playback pipeline.
pub trait AudioPipeline: Send {
    fn start(&mut self, track: AudioTrack) -> Result<(), AudioError>;
    fn pause(&mut self, paused: bool) -> Result<(), AudioError>;
    fn seek(&mut self, position_ms: u64) -> Result<(), AudioError>;
    fn stop(&mut self) -> Result<(), AudioError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioError {
    pub message: String,
}

impl AudioError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AudioError {}

#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn peaking(sample_rate: u32, frequency: f32, gain_db: f32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let frequency = frequency.clamp(10.0, (rate * 0.49).max(10.0));
        let a = 10.0_f32.powf(gain_db.clamp(-24.0, 24.0) / 40.0);
        let omega = std::f32::consts::TAU * frequency / rate;
        let alpha = omega.sin() / (2.0 * 1.4);
        let cos = omega.cos();
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        output
    }
}

/// Decodes an in-memory media resource into interleaved f32 PCM packets.
///
/// Symphonia supplies native demuxing and codec decoding for common formats
/// (MP3, AAC, FLAC, Ogg/Vorbis, WAV and others enabled by the feature set).
/// The caller owns source resolution and should run this CPU-bound function
/// outside Tokio's async executor.
pub fn decode_bytes(bytes: Vec<u8>, extension: Option<&str>) -> Result<Vec<PcmFrame>, AudioError> {
    decode_bytes_limited(bytes, extension, usize::MAX)
}

/// Decode audio while enforcing a hard upper bound on retained interleaved PCM.
pub fn decode_bytes_limited(
    bytes: impl Into<std::sync::Arc<[u8]>>,
    extension: Option<&str>,
    max_pcm_bytes: usize,
) -> Result<Vec<PcmFrame>, AudioError> {
    let mut frames = Vec::new();
    let mut retained_bytes = 0_usize;
    decode_bytes_with(bytes, extension, max_pcm_bytes, |frame| {
        let frame_bytes = frame
            .samples
            .len()
            .saturating_mul(std::mem::size_of::<f32>());
        retained_bytes = retained_bytes
            .checked_add(frame_bytes)
            .ok_or_else(|| AudioError::invalid("decoded PCM size overflow"))?;
        if retained_bytes > max_pcm_bytes {
            return Err(AudioError::invalid(
                "decoded audio exceeds the configured PCM memory limit",
            ));
        }
        frames.push(frame);
        Ok(())
    })?;
    Ok(frames)
}

/// Decode media incrementally, yielding each decoded packet to a caller.
///
/// The PCM limit bounds each decoded packet. Callbacks that retain frames must
/// enforce their own cumulative storage budget; callbacks that stream or
/// discard packets can process tracks longer than that limit. Returning an
/// error stops demuxing/decoding immediately and drops decoder state.
pub fn decode_bytes_with<F>(
    bytes: impl Into<std::sync::Arc<[u8]>>,
    extension: Option<&str>,
    max_pcm_bytes: usize,
    mut on_frame: F,
) -> Result<(), AudioError>
where
    F: FnMut(PcmFrame) -> Result<(), AudioError>,
{
    use std::io::{Cursor, ErrorKind};
    use symphonia::core::{
        audio::SampleBuffer, codecs::DecoderOptions, errors::Error as DecodeError,
        formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
    };

    let bytes: std::sync::Arc<[u8]> = bytes.into();
    let mut hint = Hint::new();
    if let Some(extension) = extension {
        hint.with_extension(extension);
    }
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| AudioError::invalid(format!("media probe failed: {error}")))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| AudioError::invalid("media contains no default audio track"))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| AudioError::invalid(format!("codec initialization failed: {error}")))?;

    let mut emitted_frames = 0_usize;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(DecodeError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(DecodeError::ResetRequired) => {
                return Err(AudioError::invalid(
                    "media stream requires an unsupported decoder reset",
                ));
            }
            Err(error) => {
                return Err(AudioError::invalid(format!("media demux failed: {error}")));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(DecodeError::DecodeError(_)) => continue,
            Err(error) => {
                return Err(AudioError::invalid(format!("audio decode failed: {error}")));
            }
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count() as u16;
        let sample_rate = spec.rate;
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        let pcm = samples.samples().to_vec();
        if pcm.is_empty() {
            continue;
        }

        let frame_bytes = pcm.len().saturating_mul(std::mem::size_of::<f32>());
        if frame_bytes > max_pcm_bytes {
            return Err(AudioError::invalid(
                "decoded PCM packet exceeds the configured frame memory limit",
            ));
        }

        on_frame(PcmFrame {
            samples: pcm,
            channels,
            sample_rate,
        })?;
        emitted_frames += 1;
    }

    if emitted_frames == 0 {
        return Err(AudioError::invalid(
            "media decoded successfully but contained no audio frames",
        ));
    }
    Ok(())
}

/// Decode a source into a bounded Tokio channel on a blocking worker.
///
/// The channel capacity is chosen by the caller. When it fills, the decoder
/// worker blocks until the consumer advances, providing real backpressure
/// instead of accumulating an unbounded decoded-PCM vector.
pub async fn decode_bytes_to_channel(
    bytes: Vec<u8>,
    extension: Option<String>,
    max_pcm_bytes: usize,
    sender: tokio::sync::mpsc::Sender<Result<PcmFrame, AudioError>>,
) -> Result<(), AudioError> {
    decode_bytes_to_channel_at(
        std::sync::Arc::from(bytes),
        extension,
        max_pcm_bytes,
        0,
        sender,
    )
    .await
}

/// Decode a buffered source into a bounded channel, discarding PCM before the
/// requested position. This re-decodes from the beginning of the compressed
/// source, but does not retain the decoded prefix in memory.
pub async fn decode_bytes_to_channel_at(
    bytes: std::sync::Arc<[u8]>,
    extension: Option<String>,
    max_pcm_bytes: usize,
    position_ms: u64,
    sender: tokio::sync::mpsc::Sender<Result<PcmFrame, AudioError>>,
) -> Result<(), AudioError> {
    tokio::task::spawn_blocking(move || {
        let mut elapsed_samples = 0_u64;
        let decoded = decode_bytes_with(bytes, extension.as_deref(), max_pcm_bytes, |mut frame| {
            if frame.channels == 0 || frame.sample_rate == 0 {
                return Err(AudioError::invalid(
                    "decoded PCM frame has an invalid layout",
                ));
            }
            let channels = usize::from(frame.channels);
            let frame_samples = frame.samples.len() / channels;
            let target_samples = position_ms.saturating_mul(u64::from(frame.sample_rate)) / 1_000;
            if elapsed_samples < target_samples {
                let samples_to_skip = target_samples
                    .saturating_sub(elapsed_samples)
                    .min(frame_samples as u64) as usize;
                frame.samples.drain(..samples_to_skip * channels);
                elapsed_samples = elapsed_samples.saturating_add(frame_samples as u64);
            }
            if !frame.samples.is_empty() {
                sender
                    .blocking_send(Ok(frame))
                    .map_err(|_| AudioError::invalid("PCM consumer closed the decode channel"))?;
            }
            Ok(())
        });
        if let Err(error) = decoded {
            let _ = sender.blocking_send(Err(AudioError::invalid(format!(
                "audio decode failed: {}",
                error.message
            ))));
            return Err(error);
        }
        Ok(())
    })
    .await
    .map_err(|error| AudioError::invalid(format!("audio decoder task failed: {error}")))?
}

/// Stateful DSP chain for Lavalink's built-in PCM filters.
///
/// The processor preserves filter history between frames, so callers should
/// keep one instance per active player rather than constructing one per packet.
#[derive(Default)]
pub struct DspProcessor {
    filters: Filters,
    sample_rate: u32,
    channels: u16,
    equalizers: Vec<Vec<Biquad>>,
    low_pass_history: Vec<f32>,
    vibrato_delay_lines: Vec<Vec<f32>>,
    vibrato_write_index: usize,
    tremolo_phase: f64,
    rotation_phase: f64,
    vibrato_phase: f64,
}

impl DspProcessor {
    pub fn new(filters: Filters) -> Self {
        Self {
            filters,
            sample_rate: 0,
            channels: 0,
            equalizers: Vec::new(),
            low_pass_history: Vec::new(),
            vibrato_delay_lines: Vec::new(),
            vibrato_write_index: 0,
            tremolo_phase: 0.0,
            rotation_phase: 0.0,
            vibrato_phase: 0.0,
        }
    }

    pub fn set_filters(&mut self, filters: Filters) {
        self.filters = filters;
        self.reset_state();
    }

    pub fn filters(&self) -> &Filters {
        &self.filters
    }

    pub fn reset_state(&mut self) {
        self.sample_rate = 0;
        self.channels = 0;
        self.equalizers.clear();
        self.low_pass_history.clear();
        self.vibrato_delay_lines.clear();
        self.vibrato_write_index = 0;
        self.tremolo_phase = 0.0;
        self.rotation_phase = 0.0;
        self.vibrato_phase = 0.0;
    }

    /// Applies configured filters in-place. Invalid PCM layouts are rejected.
    pub fn process(&mut self, frame: &mut PcmFrame) -> Result<(), AudioError> {
        if frame.channels == 0 {
            return Err(AudioError::invalid(
                "PCM frame must have at least one channel",
            ));
        }
        if frame.sample_rate == 0 {
            return Err(AudioError::invalid(
                "PCM sample rate must be greater than zero",
            ));
        }
        let channels = usize::from(frame.channels);
        if !frame.samples.len().is_multiple_of(channels) {
            return Err(AudioError::invalid(
                "interleaved PCM sample count is not channel-aligned",
            ));
        }
        if self.sample_rate != frame.sample_rate || self.channels != frame.channels {
            self.rebuild_state(frame.sample_rate, frame.channels);
        }
        if frame.samples.is_empty() {
            return Ok(());
        }

        let volume = present(&self.filters.volume)
            .copied()
            .unwrap_or(1.0)
            .clamp(0.0, 5.0);
        for sample in &mut frame.samples {
            *sample *= volume;
        }

        self.apply_equalizer(frame);
        self.apply_karaoke(frame);
        self.apply_channel_mix(frame);
        self.apply_low_pass(frame);
        self.apply_vibrato(frame);
        self.apply_tremolo(frame);
        self.apply_distortion(frame);
        self.apply_rotation(frame);

        for sample in &mut frame.samples {
            *sample = sample.clamp(-1.0, 1.0);
        }
        Ok(())
    }

    fn rebuild_state(&mut self, sample_rate: u32, channels: u16) {
        self.sample_rate = sample_rate;
        self.channels = channels;
        let bands = present(&self.filters.equalizer)
            .cloned()
            .unwrap_or_default();
        let mut sorted = bands;
        sorted.sort_by_key(|band| band.band);
        self.equalizers = (0..usize::from(channels))
            .map(|_| {
                sorted
                    .iter()
                    .map(|band| {
                        Biquad::peaking(sample_rate, eq_frequency(band.band), band.gain * 12.0)
                    })
                    .collect()
            })
            .collect();
        self.low_pass_history = vec![0.0; usize::from(channels)];
        let delay_capacity = (sample_rate as usize / 50).max(4) + 2;
        self.vibrato_delay_lines = vec![vec![0.0; delay_capacity]; usize::from(channels)];
        self.vibrato_write_index = 0;
    }

    fn apply_equalizer(&mut self, frame: &mut PcmFrame) {
        for (index, sample) in frame.samples.iter_mut().enumerate() {
            let channel = index % usize::from(frame.channels);
            for filter in &mut self.equalizers[channel] {
                *sample = filter.process(*sample);
            }
        }
    }

    fn apply_karaoke(&self, frame: &mut PcmFrame) {
        let Some(karaoke) = present(&self.filters.karaoke).and_then(Option::as_ref) else {
            return;
        };
        if frame.channels < 2 {
            return;
        }
        let amount = (karaoke.level * karaoke.mono_level).clamp(0.0, 2.0);
        for pair in frame.samples.chunks_exact_mut(usize::from(frame.channels)) {
            let center = (pair[0] + pair[1]) * 0.5;
            pair[0] -= center * amount;
            pair[1] -= center * amount;
        }
    }

    fn apply_channel_mix(&self, frame: &mut PcmFrame) {
        let Some(mix) = present(&self.filters.channel_mix).and_then(Option::as_ref) else {
            return;
        };
        if frame.channels < 2 {
            return;
        }
        for pair in frame.samples.chunks_exact_mut(usize::from(frame.channels)) {
            let left = pair[0];
            let right = pair[1];
            pair[0] = left * mix.left_to_left + right * mix.right_to_left;
            pair[1] = left * mix.left_to_right + right * mix.right_to_right;
        }
    }

    fn apply_low_pass(&mut self, frame: &mut PcmFrame) {
        let Some(filter) = present(&self.filters.low_pass).and_then(Option::as_ref) else {
            return;
        };
        let smoothing = filter.smoothing.max(0.01);
        let coefficient = (-std::f32::consts::TAU * smoothing / frame.sample_rate as f32).exp();
        for (index, sample) in frame.samples.iter_mut().enumerate() {
            let channel = index % usize::from(frame.channels);
            let output =
                (1.0 - coefficient) * *sample + coefficient * self.low_pass_history[channel];
            self.low_pass_history[channel] = output;
            *sample = output;
        }
    }

    fn apply_vibrato(&mut self, frame: &mut PcmFrame) {
        let Some(filter) = present(&self.filters.vibrato).and_then(Option::as_ref) else {
            return;
        };
        let frequency = filter.frequency.max(0.0) as f64;
        let depth = filter.depth.clamp(0.0, 1.0);
        let phase_step = std::f64::consts::TAU * frequency / frame.sample_rate as f64;
        let channels = usize::from(frame.channels);
        let capacity = self.vibrato_delay_lines.first().map_or(0, Vec::len);
        if capacity < 4 {
            return;
        }

        for samples in frame.samples.chunks_exact_mut(channels) {
            let modulation = (0.5 + 0.5 * self.vibrato_phase.sin()) as f32;
            let delay = (frame.sample_rate as f32 * (0.001 + 0.009 * depth * modulation))
                .clamp(1.0, (capacity - 2) as f32);
            let read_position =
                (self.vibrato_write_index as f32 - delay).rem_euclid(capacity as f32);
            let read_index = read_position.floor() as usize;
            let next_index = (read_index + 1) % capacity;
            let fraction = read_position - read_index as f32;

            for (channel, sample) in samples.iter_mut().enumerate() {
                let line = &mut self.vibrato_delay_lines[channel];
                let delayed = line[read_index] * (1.0 - fraction) + line[next_index] * fraction;
                line[self.vibrato_write_index] = *sample;
                *sample = delayed;
            }

            self.vibrato_write_index = (self.vibrato_write_index + 1) % capacity;
            self.vibrato_phase =
                (self.vibrato_phase + phase_step).rem_euclid(std::f64::consts::TAU);
        }
    }

    fn apply_tremolo(&mut self, frame: &mut PcmFrame) {
        let Some(filter) = present(&self.filters.tremolo).and_then(Option::as_ref) else {
            return;
        };
        let frequency = filter.frequency.max(0.0) as f64;
        let depth = filter.depth.clamp(0.0, 1.0);
        let phase_step = std::f64::consts::TAU * frequency / frame.sample_rate as f64;
        let channels = usize::from(frame.channels);
        for samples in frame.samples.chunks_exact_mut(channels) {
            let modulation = (0.5 + 0.5 * self.tremolo_phase.sin()) as f32;
            let gain = 1.0 - depth + depth * modulation;
            for sample in samples {
                *sample *= gain;
            }
            self.tremolo_phase =
                (self.tremolo_phase + phase_step).rem_euclid(std::f64::consts::TAU);
        }
    }

    fn apply_distortion(&self, frame: &mut PcmFrame) {
        let Some(filter) = present(&self.filters.distortion).and_then(Option::as_ref) else {
            return;
        };
        for sample in &mut frame.samples {
            let x = *sample;
            let y = (filter.sin_scale * (x + filter.sin_offset)).sin()
                + (filter.cos_scale * (x + filter.cos_offset)).cos()
                + (filter.tan_scale * (x + filter.tan_offset)).tan() * 0.1;
            *sample = (y * filter.scale + filter.offset).tanh();
        }
    }

    fn apply_rotation(&mut self, frame: &mut PcmFrame) {
        let Some(filter) = present(&self.filters.rotation).and_then(Option::as_ref) else {
            return;
        };
        if frame.channels < 2 {
            return;
        }
        let speed = filter.rotation_hz;
        let step = std::f64::consts::TAU * speed / frame.sample_rate as f64;
        for pair in frame.samples.chunks_exact_mut(usize::from(frame.channels)) {
            let (sin, cos) = self.rotation_phase.sin_cos();
            let left = pair[0];
            let right = pair[1];
            pair[0] = left * cos as f32 - right * sin as f32;
            pair[1] = left * sin as f32 + right * cos as f32;
            self.rotation_phase = (self.rotation_phase + step).rem_euclid(std::f64::consts::TAU);
        }
    }
}

/// Snapshot of the playback state exposed by a native player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub track: Option<AudioTrack>,
    pub paused: bool,
    pub position_ms: u64,
    pub volume: u16,
}

/// Per-player playback state and DSP pipeline.
///
/// This is the engine-side player primitive. It is deliberately independent of
/// Discord voice transport so it can be tested without a gateway connection.
pub struct NativePlayer {
    track: Option<AudioTrack>,
    paused: bool,
    position_ms: u64,
    end_time_ms: Option<u64>,
    volume: u16,
    decoded_frames: Vec<PcmFrame>,
    streamed_frames: Option<tokio::sync::mpsc::Receiver<Result<PcmFrame, AudioError>>>,
    next_frame_index: usize,
    processor: DspProcessor,
    voice_packetizer: VoicePcmPacketizer,
    opus_encoder: Option<OpusFrameEncoder>,
    opus_complexity: u8,
    completed_track: Option<AudioTrack>,
}

impl Default for NativePlayer {
    fn default() -> Self {
        Self::new(Filters::default())
    }
}

impl NativePlayer {
    pub fn new(filters: Filters) -> Self {
        Self::new_with_opus_complexity(filters, 10)
    }

    pub fn new_with_opus_complexity(filters: Filters, opus_complexity: u8) -> Self {
        Self {
            track: None,
            paused: false,
            position_ms: 0,
            end_time_ms: None,
            volume: 100,
            decoded_frames: Vec::new(),
            streamed_frames: None,
            next_frame_index: 0,
            processor: DspProcessor::new(filters),
            voice_packetizer: VoicePcmPacketizer::new(),
            opus_encoder: None,
            opus_complexity: opus_complexity.min(10),
            completed_track: None,
        }
    }

    /// Take a track that ended naturally during audio processing.
    /// Explicit stops and replacements do not populate this slot.
    pub fn take_completed_track(&mut self) -> Option<AudioTrack> {
        let completed = self.completed_track.take();
        if completed.is_some() {
            self.decoded_frames.clear();
            self.next_frame_index = 0;
            self.voice_packetizer.clear();
        }
        completed
    }

    pub fn snapshot(&self) -> PlayerSnapshot {
        PlayerSnapshot {
            track: self.track.clone(),
            paused: self.paused,
            position_ms: self.position_ms,
            volume: self.volume,
        }
    }

    pub fn set_filters(&mut self, filters: Filters) {
        self.processor.set_filters(filters);
    }

    pub fn load_decoded_track(
        &mut self,
        track: AudioTrack,
        frames: Vec<PcmFrame>,
    ) -> Result<(), AudioError> {
        if frames.is_empty() {
            return Err(AudioError::invalid("decoded track contains no PCM frames"));
        }
        self.start(track)?;
        self.decoded_frames = frames;
        self.streamed_frames = None;
        self.next_frame_index = 0;
        Ok(())
    }

    /// Attach a bounded PCM receiver as the active playback source.
    /// Empty queues are temporary underruns; a closed queue ends the track.
    pub fn load_streamed_track(
        &mut self,
        track: AudioTrack,
        receiver: tokio::sync::mpsc::Receiver<Result<PcmFrame, AudioError>>,
    ) -> Result<(), AudioError> {
        self.start(track)?;
        self.streamed_frames = Some(receiver);
        Ok(())
    }

    /// Attach a bounded PCM stream and begin it at a requested position.
    pub fn load_streamed_track_at(
        &mut self,
        track: AudioTrack,
        position_ms: u64,
        receiver: tokio::sync::mpsc::Receiver<Result<PcmFrame, AudioError>>,
    ) -> Result<(), AudioError> {
        if track.is_stream && position_ms > 0 {
            return Err(AudioError::invalid("live streams are not seekable"));
        }
        if track
            .duration_ms
            .is_some_and(|duration| position_ms > duration)
        {
            return Err(AudioError::invalid("seek position exceeds track duration"));
        }
        self.start(track)?;
        self.position_ms = position_ms;
        self.streamed_frames = Some(receiver);
        Ok(())
    }

    /// Restart the current buffered source at a new position using a fresh bounded decoder.
    pub fn restart_streamed_source(
        &mut self,
        position_ms: u64,
        receiver: tokio::sync::mpsc::Receiver<Result<PcmFrame, AudioError>>,
    ) -> Result<(), AudioError> {
        let Some(track) = self.track.as_ref() else {
            return Err(AudioError::invalid(
                "cannot seek a player without an active track",
            ));
        };
        if track.is_stream {
            return Err(AudioError::invalid("live streams are not seekable"));
        }
        if track
            .duration_ms
            .is_some_and(|duration| position_ms > duration)
            || self
                .end_time_ms
                .is_some_and(|end_time| position_ms > end_time)
        {
            return Err(AudioError::invalid("seek position exceeds track duration"));
        }
        self.streamed_frames = Some(receiver);
        self.decoded_frames.clear();
        self.next_frame_index = 0;
        self.position_ms = position_ms;
        self.voice_packetizer.clear();
        self.processor.reset_state();
        if let Some(encoder) = self.opus_encoder.as_mut() {
            encoder.reset()?;
        }
        Ok(())
    }

    /// Return the next decoded frame after applying the player's DSP chain.
    /// Build and encrypt the next Discord-compatible legacy voice packet.
    pub fn next_encrypted_voice_packet(
        &mut self,
        pipeline: &mut crate::voice_crypto::VoicePacketPipeline,
    ) -> Result<Option<Vec<u8>>, AudioError> {
        let Some(opus_packet) = self.next_opus_packet()? else {
            return Ok(None);
        };
        pipeline.packetize_opus(&opus_packet).map(Some)
    }

    /// Build a voice packet with DAVE encryption followed by RTP transport encryption.
    pub fn next_dave_encrypted_voice_packet(
        &mut self,
        pipeline: &mut crate::voice_crypto::VoicePacketPipeline,
        dave: &mut crate::dave::DaveVoiceSession,
    ) -> Result<Option<Vec<u8>>, AudioError> {
        let Some(opus_packet) = self.next_opus_packet()? else {
            return Ok(None);
        };
        pipeline
            .packetize_opus_with_dave(&opus_packet, dave)
            .map(Some)
    }

    /// Build the next unencrypted RTP packet from the native Opus encoder.
    pub fn next_rtp_packet(
        &mut self,
        packetizer: &mut crate::rtp::RtpPacketizer,
    ) -> Result<Option<Vec<u8>>, AudioError> {
        let Some(opus_packet) = self.next_opus_packet()? else {
            return Ok(None);
        };
        packetizer.packetize(&opus_packet).map(Some)
    }

    /// Encode the next decoded frame as a native Opus packet.
    pub fn next_opus_packet(&mut self) -> Result<Option<Vec<u8>>, AudioError> {
        let Some(frame) = self.next_voice_frame()? else {
            return Ok(None);
        };
        if self.opus_encoder.is_none() {
            self.opus_encoder = Some(OpusFrameEncoder::new(128_000, self.opus_complexity)?);
        }
        self.opus_encoder
            .as_mut()
            .expect("Opus encoder was initialized")
            .encode(&frame)
            .map(Some)
    }

    /// Return the next complete 20 ms stereo 48 kHz frame ready for Opus encoding.
    pub fn next_voice_frame(&mut self) -> Result<Option<PcmFrame>, AudioError> {
        loop {
            if let Some(frame) = self.voice_packetizer.pop_frame() {
                return Ok(Some(frame));
            }
            let Some(frame) = self.next_frame()? else {
                if self.track.is_none() {
                    self.voice_packetizer.clear();
                }
                return Ok(None);
            };
            self.voice_packetizer.push(&frame)?;
        }
    }

    pub fn next_frame(&mut self) -> Result<Option<PcmFrame>, AudioError> {
        if self.track.is_none() || self.paused {
            return Ok(None);
        }
        if let Some(result) = self
            .streamed_frames
            .as_mut()
            .map(|receiver| receiver.try_recv())
        {
            match result {
                Ok(Ok(frame)) => {
                    let processed = self.process_frame(frame)?;
                    if self.track.is_none() {
                        self.streamed_frames = None;
                    }
                    return Ok(processed);
                }
                Ok(Err(error)) => {
                    self.streamed_frames = None;
                    self.track = None;
                    self.paused = false;
                    self.position_ms = 0;
                    self.voice_packetizer.clear();
                    return Err(error);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return Ok(None),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    self.streamed_frames = None;
                    self.completed_track = self.track.take();
                    self.position_ms = 0;
                    self.voice_packetizer.clear();
                    return Ok(None);
                }
            }
        }
        let Some(frame) = self.decoded_frames.get(self.next_frame_index).cloned() else {
            self.completed_track = self.track.take();
            self.position_ms = 0;
            self.decoded_frames.clear();
            self.next_frame_index = 0;
            return Ok(None);
        };
        self.next_frame_index += 1;
        let processed = self.process_frame(frame)?;
        if self.track.is_none() {
            self.decoded_frames.clear();
            self.next_frame_index = 0;
        }
        Ok(processed)
    }

    pub fn set_end_time(&mut self, end_time_ms: Option<u64>) -> Result<(), AudioError> {
        if end_time_ms.is_some_and(|end_time| end_time == 0) {
            return Err(AudioError::invalid("end time must be greater than zero"));
        }
        self.end_time_ms = end_time_ms;
        Ok(())
    }

    pub fn set_volume(&mut self, volume: u16) -> Result<(), AudioError> {
        if volume > 1_000 {
            return Err(AudioError::invalid(
                "player volume must be between 0 and 1000",
            ));
        }
        self.volume = volume;
        Ok(())
    }

    pub fn process_frame(&mut self, mut frame: PcmFrame) -> Result<Option<PcmFrame>, AudioError> {
        if self.track.is_none() || self.paused {
            return Ok(None);
        }
        self.processor.process(&mut frame)?;
        let gain = f32::from(self.volume) / 100.0;
        for sample in &mut frame.samples {
            *sample = (*sample * gain).clamp(-1.0, 1.0);
        }
        let frame_count = frame.samples.len() / usize::from(frame.channels);
        let elapsed_ms = (frame_count as u64 * 1_000) / u64::from(frame.sample_rate);
        self.position_ms = self.position_ms.saturating_add(elapsed_ms);
        let effective_end =
            self.track
                .as_ref()
                .and_then(|track| match (track.duration_ms, self.end_time_ms) {
                    (Some(duration), Some(end_time)) => Some(duration.min(end_time)),
                    (Some(duration), None) => Some(duration),
                    (None, end_time) => end_time,
                });
        if effective_end.is_some_and(|end_time| self.position_ms >= end_time) {
            self.completed_track = self.track.take();
            self.paused = false;
            self.position_ms = 0;
            self.end_time_ms = None;
        }
        Ok(Some(frame))
    }
}

impl AudioPipeline for NativePlayer {
    fn start(&mut self, track: AudioTrack) -> Result<(), AudioError> {
        self.processor.reset_state();
        self.track = Some(track);
        self.completed_track = None;
        self.paused = false;
        self.position_ms = 0;
        self.end_time_ms = None;
        self.decoded_frames.clear();
        self.streamed_frames = None;
        self.next_frame_index = 0;
        self.voice_packetizer.clear();
        if let Some(encoder) = self.opus_encoder.as_mut() {
            encoder.reset()?;
        }
        Ok(())
    }

    fn pause(&mut self, paused: bool) -> Result<(), AudioError> {
        if self.track.is_none() {
            return Err(AudioError::invalid(
                "cannot pause a player without an active track",
            ));
        }
        self.paused = paused;
        Ok(())
    }

    fn seek(&mut self, position_ms: u64) -> Result<(), AudioError> {
        let Some(track) = self.track.as_ref() else {
            return Err(AudioError::invalid(
                "cannot seek a player without an active track",
            ));
        };
        if track.is_stream {
            return Err(AudioError::invalid("live streams are not seekable"));
        }
        if self.streamed_frames.is_some() {
            return Err(AudioError::invalid(
                "cannot seek while a bounded PCM stream is active",
            ));
        }
        if track
            .duration_ms
            .is_some_and(|duration| position_ms > duration)
            || self
                .end_time_ms
                .is_some_and(|end_time| position_ms > end_time)
        {
            return Err(AudioError::invalid("seek position exceeds track duration"));
        }
        self.position_ms = position_ms;
        self.voice_packetizer.clear();
        if !self.decoded_frames.is_empty() {
            let mut elapsed_ms = 0_u64;
            let mut index = 0;
            while index < self.decoded_frames.len() && elapsed_ms < position_ms {
                let frame = &self.decoded_frames[index];
                let samples_per_channel = frame.samples.len() / usize::from(frame.channels);
                elapsed_ms = elapsed_ms.saturating_add(
                    (samples_per_channel as u64 * 1_000) / u64::from(frame.sample_rate),
                );
                index += 1;
            }
            self.next_frame_index = index;
        }
        if let Some(encoder) = self.opus_encoder.as_mut() {
            encoder.reset()?;
        }
        self.processor.reset_state();
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.track = None;
        self.completed_track = None;
        self.paused = false;
        self.position_ms = 0;
        self.end_time_ms = None;
        self.decoded_frames.clear();
        self.streamed_frames = None;
        self.next_frame_index = 0;
        self.voice_packetizer.clear();
        if let Some(encoder) = self.opus_encoder.as_mut() {
            encoder.reset()?;
        }
        self.processor.reset_state();
        Ok(())
    }
}

fn present<T>(value: &Omissible<T>) -> Option<&T> {
    match value {
        Omissible::Present(value) => Some(value),
        Omissible::Omitted => None,
    }
}

fn eq_frequency(band: u8) -> f32 {
    const FREQUENCIES: [f32; 15] = [
        25.0, 40.0, 63.0, 100.0, 160.0, 250.0, 400.0, 630.0, 1_000.0, 1_600.0, 2_500.0, 4_000.0,
        6_300.0, 10_000.0, 16_000.0,
    ];
    FREQUENCIES
        .get(usize::from(band))
        .copied()
        .unwrap_or(1_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(samples: Vec<f32>, channels: u16) -> PcmFrame {
        PcmFrame {
            samples,
            channels,
            sample_rate: 48_000,
        }
    }

    #[test]
    fn limited_decoder_rejects_audio_that_exceeds_pcm_budget() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&192_000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        wav.extend_from_slice(&16_384i16.to_le_bytes());

        let error = decode_bytes_limited(wav, Some("wav"), 1).unwrap_err();
        assert!(error.message.contains("memory limit"));
    }

    #[tokio::test]
    async fn decoder_applies_backpressure_through_a_bounded_pcm_channel() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&192_000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        wav.extend_from_slice(&16_384i16.to_le_bytes());

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let decode = tokio::spawn(decode_bytes_to_channel(
            wav,
            Some("wav".to_owned()),
            64,
            sender,
        ));
        let frame = receiver.recv().await.unwrap();
        let frame = frame.unwrap();
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.sample_rate, 48_000);
        decode.await.unwrap().unwrap();
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn bounded_stream_decoder_can_process_more_than_one_frame_budget() {
        let sample_count = 48_000_u32;
        let data_bytes = sample_count * 2;
        let mut wav = Vec::with_capacity(44 + data_bytes as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for sample in 0..sample_count {
            wav.extend_from_slice(&((sample % 30_000) as i16).to_le_bytes());
        }

        let error = decode_bytes_limited(wav.clone(), Some("wav"), 20_000).unwrap_err();
        assert!(error.message.contains("PCM memory limit"));

        let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
        let decode = tokio::spawn(decode_bytes_to_channel(
            wav,
            Some("wav".to_owned()),
            20_000,
            sender,
        ));
        let mut decoded_bytes = 0_usize;
        while let Some(frame) = receiver.recv().await {
            let frame = frame.unwrap();
            decoded_bytes += frame.samples.len() * std::mem::size_of::<f32>();
        }
        decode.await.unwrap().unwrap();
        assert!(decoded_bytes > 20_000);
    }

    #[tokio::test]
    async fn bounded_decoder_starts_at_the_requested_position() {
        let sample_count = 48_000_u32;
        let data_bytes = sample_count * 2;
        let mut wav = Vec::with_capacity(44 + data_bytes as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for sample in 0..sample_count {
            wav.extend_from_slice(&((sample % 30_000) as i16).to_le_bytes());
        }

        let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
        let decode = tokio::spawn(decode_bytes_to_channel_at(
            wav.into(),
            Some("wav".to_owned()),
            200_000,
            500,
            sender,
        ));
        let first = receiver.recv().await.unwrap().unwrap();
        decode.await.unwrap().unwrap();
        assert_eq!(first.channels, 1);
        assert_eq!(first.sample_rate, 48_000);
        assert!((first.samples[0] - (24_000.0 / 32_768.0)).abs() < 0.0001);
    }

    #[test]
    fn decoder_emits_pcm_frames_through_the_streaming_callback() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&192_000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        wav.extend_from_slice(&16_384i16.to_le_bytes());

        let mut emitted = 0;
        decode_bytes_with(wav, Some("wav"), 64, |frame| {
            emitted += 1;
            assert_eq!(frame.channels, 2);
            assert_eq!(frame.sample_rate, 48_000);
            assert_eq!(frame.samples.len(), 2);
            Ok(())
        })
        .unwrap();
        assert_eq!(emitted, 1);
    }

    #[test]
    fn decodes_pcm_wave_audio_without_a_jvm() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&192_000u32.to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        wav.extend_from_slice(&16_384i16.to_le_bytes());

        let frames = decode_bytes(wav, Some("wav")).unwrap();
        assert!(!frames.is_empty());
        assert_eq!(frames[0].channels, 2);
        assert_eq!(frames[0].sample_rate, 48_000);
    }

    #[test]
    fn streamed_source_restart_preserves_pause_and_player_controls() {
        let mut player = NativePlayer::default();
        let track = AudioTrack {
            identifier: "buffered".into(),
            title: "Buffered".into(),
            author: "Test".into(),
            duration_ms: Some(20_000),
            is_stream: false,
        };
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        player
            .load_streamed_track_at(track, 5_000, receiver)
            .unwrap();
        player.set_volume(250).unwrap();
        player.pause(true).unwrap();

        let (next_sender, next_receiver) = tokio::sync::mpsc::channel(1);
        player
            .restart_streamed_source(8_000, next_receiver)
            .unwrap();
        let snapshot = player.snapshot();
        assert_eq!(snapshot.position_ms, 8_000);
        assert!(snapshot.paused);
        assert_eq!(snapshot.volume, 250);
        drop((sender, next_sender));
    }

    #[test]
    fn invalid_initial_stream_position_does_not_replace_the_current_track() {
        let mut player = NativePlayer::default();
        let current = AudioTrack {
            identifier: "current".into(),
            title: "Current".into(),
            author: "Test".into(),
            duration_ms: Some(10_000),
            is_stream: false,
        };
        player.start(current.clone()).unwrap();
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let requested = AudioTrack {
            identifier: "requested".into(),
            title: "Requested".into(),
            author: "Test".into(),
            duration_ms: Some(100),
            is_stream: false,
        };
        assert!(
            player
                .load_streamed_track_at(requested, 101, receiver)
                .is_err()
        );
        assert_eq!(player.snapshot().track, Some(current));
        drop(sender);
    }

    #[test]
    fn native_player_obeys_start_pause_seek_and_stop_lifecycle() {
        let mut player = NativePlayer::default();
        let track = AudioTrack {
            identifier: "track-1".into(),
            title: "Test".into(),
            author: "RavaLink".into(),
            duration_ms: Some(30_000),
            is_stream: false,
        };
        player.start(track.clone()).unwrap();
        assert_eq!(player.snapshot().track, Some(track));
        player.pause(true).unwrap();
        assert!(
            player
                .process_frame(frame(vec![0.1, 0.1], 2))
                .unwrap()
                .is_none()
        );
        player.pause(false).unwrap();
        player.seek(5_000).unwrap();
        assert_eq!(player.snapshot().position_ms, 5_000);
        player.stop().unwrap();
        assert!(player.snapshot().track.is_none());
    }

    #[test]
    fn native_player_yields_decoded_frames_until_track_ends() {
        let mut player = NativePlayer::default();
        player
            .load_decoded_track(
                AudioTrack {
                    identifier: "short".into(),
                    title: "Short".into(),
                    author: "Test".into(),
                    duration_ms: Some(40),
                    is_stream: false,
                },
                vec![frame(vec![0.5, 0.5], 2), frame(vec![0.25, 0.25], 2)],
            )
            .unwrap();
        assert!(player.next_frame().unwrap().is_some());
        assert!(player.next_frame().unwrap().is_some());
        assert!(player.next_frame().unwrap().is_none());
        assert!(player.snapshot().track.is_none());
    }

    #[test]
    fn native_player_emits_voice_ready_twenty_millisecond_frames() {
        let mut player = NativePlayer::default();
        player
            .load_decoded_track(
                AudioTrack {
                    identifier: "short".into(),
                    title: "Short".into(),
                    author: "Test".into(),
                    duration_ms: Some(20),
                    is_stream: false,
                },
                vec![frame(vec![0.1; 960], 2), frame(vec![0.2; 960], 2)],
            )
            .unwrap();

        let voice_frame = player.next_voice_frame().unwrap().unwrap();
        assert_eq!(voice_frame.samples.len(), 1_920);
        assert_eq!(voice_frame.channels, 2);
        assert_eq!(voice_frame.sample_rate, 48_000);
        assert!(player.next_voice_frame().unwrap().is_none());
    }

    #[test]
    fn pausing_preserves_partial_voice_frame_buffer() {
        let mut player = NativePlayer::default();
        player
            .start(AudioTrack {
                identifier: "short".into(),
                title: "Short".into(),
                author: "Test".into(),
                duration_ms: Some(20),
                is_stream: false,
            })
            .unwrap();
        player
            .voice_packetizer
            .push(&frame(vec![0.1; 960], 2))
            .unwrap();
        player.pause(true).unwrap();

        assert!(player.next_voice_frame().unwrap().is_none());
        assert_eq!(player.voice_packetizer.buffered_samples(), 960);
    }

    #[test]
    fn native_player_encodes_loaded_audio_as_opus_packets() {
        let mut player = NativePlayer::default();
        player
            .load_decoded_track(
                AudioTrack {
                    identifier: "short".into(),
                    title: "Short".into(),
                    author: "Test".into(),
                    duration_ms: Some(20),
                    is_stream: false,
                },
                vec![frame(vec![0.15; 960], 2), frame(vec![0.15; 960], 2)],
            )
            .unwrap();

        let packet = player.next_opus_packet().unwrap().unwrap();
        assert!(!packet.is_empty());
        assert!(packet.len() <= 4_000);
        assert!(player.next_opus_packet().unwrap().is_none());
    }

    #[test]
    fn native_player_emits_rtp_packets_from_decoded_audio() {
        let mut player = NativePlayer::default();
        player
            .load_decoded_track(
                AudioTrack {
                    identifier: "short".into(),
                    title: "Short".into(),
                    author: "Test".into(),
                    duration_ms: Some(20),
                    is_stream: false,
                },
                vec![frame(vec![0.1; 960], 2), frame(vec![0.1; 960], 2)],
            )
            .unwrap();
        let mut packetizer = crate::rtp::RtpPacketizer::new(0x1234_5678, 1, 960);

        let packet = player.next_rtp_packet(&mut packetizer).unwrap().unwrap();
        assert_eq!(packet[0], 0x80);
        assert_eq!(packet[1], 120);
        assert_eq!(&packet[8..12], &0x1234_5678_u32.to_be_bytes());
        assert!(packet.len() > crate::rtp::RTP_HEADER_BYTES);
    }

    #[test]
    fn end_time_stops_playback_before_the_full_track_duration() {
        let mut player = NativePlayer::default();
        player
            .load_decoded_track(
                AudioTrack {
                    identifier: "long".into(),
                    title: "Long".into(),
                    author: "Test".into(),
                    duration_ms: Some(10_000),
                    is_stream: false,
                },
                vec![frame(vec![0.1; 960], 2), frame(vec![0.1; 960], 2)],
            )
            .unwrap();
        player.set_end_time(Some(10)).unwrap();

        assert!(player.next_frame().unwrap().is_some());
        assert!(player.snapshot().track.is_none());
    }

    #[test]
    fn native_player_rejects_seeking_live_streams() {
        let mut player = NativePlayer::default();
        player
            .start(AudioTrack {
                identifier: "live".into(),
                title: "Live".into(),
                author: "Radio".into(),
                duration_ms: None,
                is_stream: true,
            })
            .unwrap();
        assert!(player.seek(1_000).is_err());
    }

    #[test]
    fn rejects_invalid_pcm_layout() {
        let mut processor = DspProcessor::default();
        assert!(processor.process(&mut frame(vec![0.0], 0)).is_err());
        assert!(processor.process(&mut frame(vec![0.0], 2)).is_err());
    }

    #[test]
    fn rotation_filter_rotates_the_stereo_field() {
        let filters: Filters = serde_json::from_value(json!({
            "rotation": {"rotationHz": 0.0}
        }))
        .unwrap();
        let mut processor = DspProcessor::new(filters);
        let mut input = frame(vec![1.0, 0.0], 2);
        processor.process(&mut input).unwrap();
        assert!((input.samples[0] - 1.0).abs() < 0.0001);
        assert!(input.samples[1].abs() < 0.0001);
    }

    #[test]
    fn applies_volume_gain_in_place() {
        let filters: Filters = serde_json::from_value(json!({"volume": 0.5})).unwrap();
        let mut processor = DspProcessor::new(filters);
        let mut input = frame(vec![0.8, -0.4], 2);
        processor.process(&mut input).unwrap();
        assert!((input.samples[0] - 0.4).abs() < 0.0001);
        assert!((input.samples[1] + 0.2).abs() < 0.0001);
    }

    #[test]
    fn low_pass_preserves_state_between_frames() {
        let filters: Filters =
            serde_json::from_value(json!({"lowPass": {"smoothing": 20.0}})).unwrap();
        let mut processor = DspProcessor::new(filters);
        let mut first = frame(vec![1.0, 1.0], 1);
        let mut second = frame(vec![0.0, 0.0], 1);
        processor.process(&mut first).unwrap();
        processor.process(&mut second).unwrap();
        assert!(second.samples[0] > 0.0);
    }

    #[test]
    fn vibrato_uses_a_bounded_delay_line_and_keeps_finite_samples() {
        let filters: Filters = serde_json::from_value(json!({
            "vibrato": {"frequency": 5.0, "depth": 0.5}
        }))
        .unwrap();
        let mut processor = DspProcessor::new(filters);
        let mut input = frame(vec![0.25; 4_800], 1);
        processor.process(&mut input).unwrap();
        assert!(input.samples.iter().all(|sample| sample.is_finite()));
        assert!(input.samples.iter().any(|sample| *sample > 0.0));
    }

    #[test]
    fn channel_mix_is_applied_to_stereo_frames() {
        let filters: Filters = serde_json::from_value(json!({
            "channelMix": {"leftToLeft": 0.0, "leftToRight": 1.0, "rightToLeft": 1.0, "rightToRight": 0.0}
        })).unwrap();
        let mut processor = DspProcessor::new(filters);
        let mut input = frame(vec![0.25, 0.75], 2);
        processor.process(&mut input).unwrap();
        assert!((input.samples[0] - 0.75).abs() < 0.0001);
        assert!((input.samples[1] - 0.25).abs() < 0.0001);
    }
    #[test]
    fn natural_completion_preserves_the_track_for_end_event_reporting() {
        let mut player = NativePlayer::default();
        player
            .load_decoded_track(
                AudioTrack {
                    identifier: "finished".into(),
                    title: "Finished".into(),
                    author: "Test".into(),
                    duration_ms: Some(20),
                    is_stream: false,
                },
                vec![frame(vec![0.1; 1_920], 2)],
            )
            .unwrap();

        assert!(player.next_frame().unwrap().is_some());
        assert!(player.snapshot().track.is_none());
        assert_eq!(
            player.take_completed_track().unwrap().identifier,
            "finished"
        );
        assert!(player.take_completed_track().is_none());
    }
}

#[cfg(test)]
mod streamed_queue_tests {
    use super::*;

    #[tokio::test]
    async fn native_player_consumes_bounded_stream_and_completes_on_channel_close() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let mut player = NativePlayer::default();
        let track = AudioTrack {
            identifier: "streamed".into(),
            title: "Streamed track".into(),
            author: "Test".into(),
            duration_ms: None,
            is_stream: false,
        };
        player.load_streamed_track(track.clone(), receiver).unwrap();
        assert!(player.next_frame().unwrap().is_none());

        sender
            .send(Ok(PcmFrame {
                samples: vec![0.1, -0.1],
                channels: 2,
                sample_rate: 48_000,
            }))
            .await
            .unwrap();
        assert!(player.next_frame().unwrap().is_some());
        drop(sender);

        assert!(player.next_frame().unwrap().is_none());
        assert_eq!(player.take_completed_track(), Some(track));
    }

    #[tokio::test]
    async fn native_player_propagates_decoder_errors_without_finishing_track() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let mut player = NativePlayer::default();
        let track = AudioTrack {
            identifier: "broken-stream".into(),
            title: "Broken stream".into(),
            author: "Test".into(),
            duration_ms: None,
            is_stream: false,
        };
        player.load_streamed_track(track, receiver).unwrap();
        sender
            .send(Err(AudioError::invalid(
                "audio decode failed: malformed media",
            )))
            .await
            .unwrap();

        let error = player.next_frame().unwrap_err();
        assert!(error.message.contains("malformed media"));
        assert!(player.take_completed_track().is_none());
    }

    #[tokio::test]
    async fn streamed_queue_rejects_seek_until_a_seekable_source_is_reopened() {
        let (_sender, receiver) = tokio::sync::mpsc::channel(1);
        let mut player = NativePlayer::default();
        player
            .load_streamed_track(
                AudioTrack {
                    identifier: "streamed".into(),
                    title: "Streamed track".into(),
                    author: "Test".into(),
                    duration_ms: Some(60_000),
                    is_stream: false,
                },
                receiver,
            )
            .unwrap();
        assert!(player.seek(1_000).is_err());
    }
}
