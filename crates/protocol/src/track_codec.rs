//! Lavalink/Lavaplayer track message encoding (track info versions 1-3).

use crate::{Track, TrackInfo};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackCodecError {
    pub message: String,
}

impl TrackCodecError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TrackCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TrackCodecError {}

/// Decode a Lavalink track string using the Lavaplayer message framing and
/// versioned track-info layout. Source managers with custom binary payloads
/// must be explicitly supported before they can be decoded.
pub fn decode_track(encoded: &str) -> Result<Track, TrackCodecError> {
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|error| TrackCodecError::new(format!("invalid base64 track: {error}")))?;
    if bytes.len() < 5 {
        return Err(TrackCodecError::new("encoded track is too short"));
    }

    let header = u32::from_be_bytes(bytes[0..4].try_into().expect("four-byte header"));
    let flags = header >> 30;
    let message_size = (header & 0x3fff_ffff) as usize;
    if message_size != bytes.len() - 4 {
        return Err(TrackCodecError::new(
            "track message length does not match payload",
        ));
    }
    let payload = &bytes[4..];
    let mut cursor = 0;
    let version = if flags & 1 != 0 {
        read_u8(payload, &mut cursor)?
    } else {
        1
    };
    if !(1..=3).contains(&version) {
        return Err(TrackCodecError::new(format!(
            "unsupported Lavaplayer track-info version: {version}"
        )));
    }

    let title = read_utf(payload, &mut cursor)?;
    let author = read_utf(payload, &mut cursor)?;
    let length = read_i64(payload, &mut cursor)?;
    let identifier = read_utf(payload, &mut cursor)?;
    let is_stream = read_u8(payload, &mut cursor)? != 0;
    let uri = if version >= 2 {
        read_nullable_utf(payload, &mut cursor)?
    } else {
        None
    };
    let artwork_url = if version >= 3 {
        read_nullable_utf(payload, &mut cursor)?
    } else {
        None
    };
    let isrc = if version >= 3 {
        read_nullable_utf(payload, &mut cursor)?
    } else {
        None
    };
    let source_name = read_utf(payload, &mut cursor)?;

    match source_name.as_str() {
        "http" | "local" => {
            // ProbingAudioSourceManager persists the container probe name and
            // optional probe parameters as a single modified-UTF string.
            let _probe_info = read_utf(payload, &mut cursor)?;
        }
        "youtube" | "soundcloud" | "bandcamp" | "vimeo" | "twitch" | "nico" | "getyarn"
        | "yandexmusic" | "beam" | "spotify" | "applemusic" | "deezer" => {}
        other => {
            return Err(TrackCodecError::new(format!(
                "unsupported Lavaplayer source manager: {other}"
            )));
        }
    }

    let position = read_u64(payload, &mut cursor)?;
    if cursor != payload.len() {
        return Err(TrackCodecError::new(
            "unexpected trailing bytes in encoded track",
        ));
    }

    Ok(Track {
        encoded: encoded.to_owned(),
        info: TrackInfo {
            identifier,
            is_seekable: !is_stream,
            author,
            length,
            is_stream,
            position,
            title,
            uri,
            source_name,
            artwork_url,
            isrc,
        },
        plugin_info: json!({}),
        user_data: json!({}),
    })
}

/// Encode a track using Lavaplayer's version-3 track-info message envelope.
///
/// This emits the standard no-extra-data format used by sources such as
/// YouTube, SoundCloud, Bandcamp, Vimeo and Twitch. The HTTP source manager
/// additionally requires its container probe descriptor.
pub fn encode_track(track: &Track) -> Result<String, TrackCodecError> {
    encode_track_with_source_data(track, None)
}

/// Encode a track with an optional source-manager payload. HTTP and local
/// Lavaplayer tracks require their probing container descriptor (for example
/// "MPEG" or "WAV|parameters").
pub fn encode_track_with_source_data(
    track: &Track,
    source_data: Option<&str>,
) -> Result<String, TrackCodecError> {
    let source_name = track.info.source_name.as_str();
    let mut payload = Vec::new();
    payload.push(3);
    write_utf(&mut payload, &track.info.title)?;
    write_utf(&mut payload, &track.info.author)?;
    payload.extend_from_slice(&track.info.length.to_be_bytes());
    write_utf(&mut payload, &track.info.identifier)?;
    payload.push(u8::from(track.info.is_stream));
    write_nullable_utf(&mut payload, track.info.uri.as_deref())?;
    write_nullable_utf(&mut payload, track.info.artwork_url.as_deref())?;
    write_nullable_utf(&mut payload, track.info.isrc.as_deref())?;
    write_utf(&mut payload, source_name)?;

    match source_name {
        "http" | "local" => {
            let source_data = source_data.ok_or_else(|| {
                TrackCodecError::new("HTTP/local tracks require a container probe descriptor")
            })?;
            write_utf(&mut payload, source_data)?;
        }
        "youtube" | "soundcloud" | "bandcamp" | "vimeo" | "twitch" | "nico" | "getyarn"
        | "yandexmusic" | "beam" | "spotify" | "applemusic" | "deezer" => {}
        other => {
            return Err(TrackCodecError::new(format!(
                "unsupported Lavaplayer source manager: {other}"
            )));
        }
    }
    payload.extend_from_slice(&track.info.position.to_be_bytes());

    let header = (payload.len() as u32) | (1 << 30);
    let mut message = Vec::with_capacity(payload.len() + 4);
    message.extend_from_slice(&header.to_be_bytes());
    message.extend_from_slice(&payload);
    Ok(STANDARD.encode(message))
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, TrackCodecError> {
    let value = *bytes
        .get(*cursor)
        .ok_or_else(|| TrackCodecError::new("truncated encoded track"))?;
    *cursor += 1;
    Ok(value)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, TrackCodecError> {
    let end = cursor.saturating_add(8);
    let slice = bytes
        .get(*cursor..end)
        .ok_or_else(|| TrackCodecError::new("truncated encoded track"))?;
    *cursor = end;
    Ok(u64::from_be_bytes(
        slice.try_into().expect("eight-byte integer"),
    ))
}

fn read_i64(bytes: &[u8], cursor: &mut usize) -> Result<i64, TrackCodecError> {
    let end = cursor.saturating_add(8);
    let slice = bytes
        .get(*cursor..end)
        .ok_or_else(|| TrackCodecError::new("truncated encoded track"))?;
    *cursor = end;
    Ok(i64::from_be_bytes(
        slice.try_into().expect("eight-byte integer"),
    ))
}

fn read_utf(bytes: &[u8], cursor: &mut usize) -> Result<String, TrackCodecError> {
    let length_bytes = bytes
        .get(*cursor..cursor.saturating_add(2))
        .ok_or_else(|| TrackCodecError::new("truncated modified-UTF length"))?;
    let length = u16::from_be_bytes(length_bytes.try_into().expect("two-byte length")) as usize;
    *cursor += 2;
    let end = cursor.saturating_add(length);
    let encoded = bytes
        .get(*cursor..end)
        .ok_or_else(|| TrackCodecError::new("truncated modified-UTF string"))?;
    *cursor = end;

    let mut units = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        let first = encoded[index];
        if first & 0x80 == 0 {
            if first == 0 {
                return Err(TrackCodecError::new("invalid zero byte in modified UTF"));
            }
            units.push(u16::from(first));
            index += 1;
        } else if first & 0xe0 == 0xc0 {
            let second = *encoded
                .get(index + 1)
                .ok_or_else(|| TrackCodecError::new("truncated modified-UTF sequence"))?;
            if second & 0xc0 != 0x80 {
                return Err(TrackCodecError::new(
                    "invalid modified-UTF continuation byte",
                ));
            }
            units.push((u16::from(first & 0x1f) << 6) | u16::from(second & 0x3f));
            index += 2;
        } else if first & 0xf0 == 0xe0 {
            let second = *encoded
                .get(index + 1)
                .ok_or_else(|| TrackCodecError::new("truncated modified-UTF sequence"))?;
            let third = *encoded
                .get(index + 2)
                .ok_or_else(|| TrackCodecError::new("truncated modified-UTF sequence"))?;
            if second & 0xc0 != 0x80 || third & 0xc0 != 0x80 {
                return Err(TrackCodecError::new(
                    "invalid modified-UTF continuation byte",
                ));
            }
            units.push(
                (u16::from(first & 0x0f) << 12)
                    | (u16::from(second & 0x3f) << 6)
                    | u16::from(third & 0x3f),
            );
            index += 3;
        } else {
            return Err(TrackCodecError::new("invalid modified-UTF leading byte"));
        }
    }
    String::from_utf16(&units)
        .map_err(|error| TrackCodecError::new(format!("invalid UTF-16 track text: {error}")))
}

fn read_nullable_utf(bytes: &[u8], cursor: &mut usize) -> Result<Option<String>, TrackCodecError> {
    if read_u8(bytes, cursor)? == 0 {
        Ok(None)
    } else {
        read_utf(bytes, cursor).map(Some)
    }
}

fn write_utf(output: &mut Vec<u8>, value: &str) -> Result<(), TrackCodecError> {
    let mut encoded = Vec::new();
    for unit in value.encode_utf16() {
        match unit {
            0x0001..=0x007f => encoded.push(unit as u8),
            0x0000..=0x07ff => {
                encoded.push(0xc0 | ((unit >> 6) as u8));
                encoded.push(0x80 | ((unit & 0x3f) as u8));
            }
            _ => {
                encoded.push(0xe0 | ((unit >> 12) as u8));
                encoded.push(0x80 | (((unit >> 6) & 0x3f) as u8));
                encoded.push(0x80 | ((unit & 0x3f) as u8));
            }
        }
    }
    let length = u16::try_from(encoded.len())
        .map_err(|_| TrackCodecError::new("track text exceeds modified-UTF length limit"))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&encoded);
    Ok(())
}

fn write_nullable_utf(output: &mut Vec<u8>, value: Option<&str>) -> Result<(), TrackCodecError> {
    match value {
        Some(value) => {
            output.push(1);
            write_utf(output, value)?;
        }
        None => output.push(0),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track() -> Track {
        Track {
            encoded: String::new(),
            info: TrackInfo {
                identifier: "id-123".into(),
                is_seekable: true,
                author: "Artist".into(),
                length: 123_456,
                is_stream: false,
                position: 5_000,
                title: "Titre 東京 🎵".into(),
                uri: Some("https://example.com/audio.mp3".into()),
                source_name: "youtube".into(),
                artwork_url: Some("https://example.com/art.jpg".into()),
                isrc: Some("USABC1234567".into()),
            },
            plugin_info: json!({}),
            user_data: json!({}),
        }
    }

    #[test]
    fn version_three_track_round_trips_unicode_metadata() {
        let mut track = sample_track();
        track.encoded = encode_track(&track).unwrap();
        let decoded = decode_track(&track.encoded).unwrap();
        assert_eq!(decoded.info, track.info);
        assert_eq!(decoded.encoded, track.encoded);
    }

    #[test]
    fn decodes_a_real_lavaplayer_v2_track() {
        let encoded = "QAAAkwIANFNrcmlsbGV4ICYgSGFic3RyYWt0IC0gQ2hpY2tlbiBTb3VwIFtPZmZpY2lhbCBBdWRpb10ABU9XU0xBAAAAAAADLIAACzIyTVdyV1BWX1FNAAEAK2h0dHBzOi8vd3d3LnlvdXR1YmUuY29tL3dhdGNoP3Y9MjJNV3JXUFZfUU0AB3lvdXR1YmUAAAAAAAAAAA==";
        let track = decode_track(encoded).unwrap();
        assert_eq!(
            track.info.title,
            "Skrillex & Habstrakt - Chicken Soup [Official Audio]"
        );
        assert_eq!(track.info.author, "OWSLA");
        assert_eq!(track.info.length, 208_000);
        assert_eq!(track.info.identifier, "22MWrWPV_QM");
        assert_eq!(track.info.source_name, "youtube");
        assert_eq!(track.info.position, 0);
    }

    #[test]
    fn native_music_provider_sources_round_trip_without_extra_payloads() {
        for source_name in ["spotify", "applemusic", "deezer"] {
            let mut track = sample_track();
            track.info.source_name = source_name.to_owned();
            track.encoded = encode_track(&track).unwrap();

            let decoded = decode_track(&track.encoded).unwrap();
            assert_eq!(decoded.info.source_name, source_name);
            assert_eq!(decoded.info.uri, track.info.uri);
        }
    }

    #[test]
    fn rejects_unknown_source_manager_payloads() {
        let mut track = sample_track();
        track.info.source_name = "unknown-source".into();
        assert!(encode_track(&track).is_err());
    }
}
