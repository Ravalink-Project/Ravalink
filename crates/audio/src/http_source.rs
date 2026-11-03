//! Direct HTTP media source adapter with bounded decoded-PCM caching.

use crate::{AudioError, PcmFrame, decode_bytes_limited};
use ravalink_protocol::{Track, TrackInfo, encode_track_with_source_data};
use serde_json::json;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

const MAX_DOWNLOAD_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_CACHE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone)]
pub struct HttpSourceManager {
    cache: Arc<Mutex<CacheState>>,
    max_cache_bytes: usize,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<String, CacheEntry>,
    bytes_used: usize,
    sequence: u64,
}

struct CacheEntry {
    frames: Vec<PcmFrame>,
    bytes: usize,
    sequence: u64,
}

#[derive(Debug, Clone)]
pub struct ResolvedHttpTrack {
    pub track: Track,
    pub duration_ms: u64,
}

impl Default for HttpSourceManager {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_BYTES)
    }
}

impl HttpSourceManager {
    pub fn new(max_cache_bytes: usize) -> Self {
        Self {
            cache: Arc::new(Mutex::new(CacheState::default())),
            max_cache_bytes,
        }
    }

    /// Resolve and decode a direct HTTP(S) media URL.
    ///
    /// This initial adapter buffers the source before decoding. It is intended
    /// for bounded files, not live radio streams or large remote archives.
    pub async fn resolve(&self, identifier: &str) -> Result<ResolvedHttpTrack, AudioError> {
        let url = reqwest::Url::parse(identifier)
            .map_err(|error| AudioError::invalid(format!("invalid media URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(AudioError::invalid(
                "only HTTP and HTTPS media URLs are supported",
            ));
        }

        let host = url
            .host_str()
            .ok_or_else(|| AudioError::invalid("HTTP source URL has no host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| AudioError::invalid("HTTP source URL has no known port"))?;
        let addresses = match host.parse::<IpAddr>() {
            Ok(address) => vec![SocketAddr::new(address, port)],
            Err(_) => tokio::net::lookup_host((host, port))
                .await
                .map_err(|error| {
                    AudioError::invalid(format!("cannot resolve HTTP source host: {error}"))
                })?
                .collect::<Vec<_>>(),
        };
        if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
            return Err(AudioError::invalid(
                "HTTP source host resolves to a private or reserved network address",
            ));
        }

        // Pin validated DNS answers and disable redirects/proxies to prevent SSRF bypasses.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(45))
            .user_agent("RavaLink/0.1 native-audio")
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|error| AudioError::invalid(format!("cannot build HTTP client: {error}")))?;
        let mut response = client
            .get(url.clone())
            .send()
            .await
            .map_err(|error| AudioError::invalid(format!("HTTP source request failed: {error}")))?
            .error_for_status()
            .map_err(|error| {
                AudioError::invalid(format!("HTTP source returned an error: {error}"))
            })?;

        if response
            .content_length()
            .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
        {
            return Err(AudioError::invalid(
                "HTTP source exceeds the 128 MiB download limit",
            ));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| AudioError::invalid(format!("cannot read HTTP source: {error}")))?
        {
            if body.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
                return Err(AudioError::invalid(
                    "HTTP source exceeds the 128 MiB download limit",
                ));
            }
            body.extend_from_slice(&chunk);
        }

        let extension = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .and_then(|segment| segment.rsplit_once('.').map(|(_, extension)| extension))
            .map(str::to_ascii_lowercase);
        let probe = probe_name(extension.as_deref(), &content_type)
            .ok_or_else(|| AudioError::invalid("unsupported direct HTTP audio format"))?;
        let decode_extension = extension.clone();
        let source_bytes = body;
        let max_pcm_bytes = self.max_cache_bytes;
        let frames = tokio::task::spawn_blocking(move || {
            decode_bytes_limited(source_bytes, decode_extension.as_deref(), max_pcm_bytes)
        })
        .await
        .map_err(|error| AudioError::invalid(format!("audio decoder task failed: {error}")))??;

        let first = frames
            .first()
            .ok_or_else(|| AudioError::invalid("HTTP source contains no decoded audio"))?;
        let sample_rate = first.sample_rate;
        let channels = usize::from(first.channels);
        if sample_rate == 0 || channels == 0 {
            return Err(AudioError::invalid("HTTP source has an invalid PCM layout"));
        }
        if frames.iter().any(|frame| {
            frame.sample_rate != sample_rate || usize::from(frame.channels) != channels
        }) {
            return Err(AudioError::invalid(
                "HTTP source changes its PCM layout mid-track",
            ));
        }

        let frame_count: u64 = frames
            .iter()
            .map(|frame| (frame.samples.len() / channels) as u64)
            .sum();
        let duration_ms = frame_count.saturating_mul(1_000) / u64::from(sample_rate);
        let pcm_bytes: usize = frames
            .iter()
            .map(|frame| frame.samples.len() * std::mem::size_of::<f32>())
            .sum();
        if pcm_bytes > self.max_cache_bytes {
            return Err(AudioError::invalid(
                "decoded HTTP source exceeds the PCM cache limit",
            ));
        }

        let title = media_title(&url);
        let mut track = Track {
            encoded: String::new(),
            info: TrackInfo {
                identifier: identifier.to_owned(),
                is_seekable: true,
                author: "Unknown".into(),
                length: i64::try_from(duration_ms).unwrap_or(i64::MAX),
                is_stream: false,
                position: 0,
                title,
                uri: Some(identifier.to_owned()),
                source_name: "http".into(),
                artwork_url: None,
                isrc: None,
            },
            plugin_info: json!({}),
            user_data: json!({}),
        };
        track.encoded = encode_track_with_source_data(&track, Some(probe))
            .map_err(|error| AudioError::invalid(error.message))?;
        self.insert_cache(track.encoded.clone(), frames, pcm_bytes)?;
        Ok(ResolvedHttpTrack { track, duration_ms })
    }

    /// Take ownership of previously decoded frames for a track returned by loadtracks.
    pub fn take_frames(&self, encoded_track: &str) -> Result<Option<Vec<PcmFrame>>, AudioError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| AudioError::invalid("HTTP source cache lock is poisoned"))?;
        let Some(entry) = cache.entries.remove(encoded_track) else {
            return Ok(None);
        };
        cache.bytes_used = cache.bytes_used.saturating_sub(entry.bytes);
        Ok(Some(entry.frames))
    }

    pub fn cached_bytes(&self) -> Result<usize, AudioError> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| AudioError::invalid("HTTP source cache lock is poisoned"))?;
        Ok(cache.bytes_used)
    }

    fn insert_cache(
        &self,
        key: String,
        frames: Vec<PcmFrame>,
        bytes: usize,
    ) -> Result<(), AudioError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| AudioError::invalid("HTTP source cache lock is poisoned"))?;
        if let Some(previous) = cache.entries.remove(&key) {
            cache.bytes_used = cache.bytes_used.saturating_sub(previous.bytes);
        }
        while cache.bytes_used.saturating_add(bytes) > self.max_cache_bytes {
            let oldest = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.sequence)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            if let Some(evicted) = cache.entries.remove(&oldest) {
                cache.bytes_used = cache.bytes_used.saturating_sub(evicted.bytes);
            }
        }
        cache.sequence = cache.sequence.wrapping_add(1);
        let sequence = cache.sequence;
        cache.entries.insert(
            key,
            CacheEntry {
                frames,
                bytes,
                sequence,
            },
        );
        cache.bytes_used = cache.bytes_used.saturating_add(bytes);
        Ok(())
    }
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [a, b, c, _] = address.octets();
            !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_unspecified()
                && !address.is_broadcast()
                && !address.is_multicast()
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && b == 0)
                && a != 0
                && !(a == 198 && (b == 18 || b == 19 || b == 51))
                && !(a == 203 && b == 0 && c == 113)
                && a < 224
        }
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(mapped));
            }
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
                && !(address.segments()[0] == 0x2001 && address.segments()[1] == 0x0db8)
        }
    }
}

fn probe_name(extension: Option<&str>, content_type: &str) -> Option<&'static str> {
    match extension {
        Some("mp3") => Some("MPEG"),
        Some("wav") | Some("wave") => Some("WAV"),
        Some("flac") => Some("FLAC"),
        Some("ogg") | Some("oga") | Some("opus") => Some("OGG"),
        Some("aac") => Some("ADTS"),
        Some("m4a") | Some("mp4") => Some("MP4"),
        Some("webm") | Some("mkv") => Some("MATROSKA"),
        _ => match content_type.split(';').next().unwrap_or("").trim() {
            "audio/mpeg" => Some("MPEG"),
            "audio/wav" | "audio/wave" | "audio/x-wav" => Some("WAV"),
            "audio/flac" | "audio/x-flac" => Some("FLAC"),
            "audio/ogg" | "application/ogg" => Some("OGG"),
            "audio/aac" | "audio/aacp" => Some("ADTS"),
            "audio/mp4" | "video/mp4" => Some("MP4"),
            "audio/webm" | "video/webm" => Some("MATROSKA"),
            _ => None,
        },
    }
}

fn media_title(url: &reqwest::Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .unwrap_or("Untitled audio")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_cache_evicts_oldest_entries_to_respect_its_limit() {
        let manager = HttpSourceManager::new(16);
        let frame = PcmFrame {
            samples: vec![0.1, 0.2],
            channels: 2,
            sample_rate: 48_000,
        };
        manager
            .insert_cache("first".into(), vec![frame.clone()], 8)
            .unwrap();
        manager
            .insert_cache("second".into(), vec![frame], 12)
            .unwrap();

        assert_eq!(manager.cached_bytes().unwrap(), 12);
        assert!(manager.take_frames("first").unwrap().is_none());
        assert!(manager.take_frames("second").unwrap().is_some());
        assert_eq!(manager.cached_bytes().unwrap(), 0);
    }

    #[test]
    fn taking_pcm_frames_removes_the_cache_entry() {
        let manager = HttpSourceManager::new(64);
        let frame = PcmFrame {
            samples: vec![0.1, -0.1],
            channels: 2,
            sample_rate: 48_000,
        };
        manager
            .insert_cache("track".into(), vec![frame], 8)
            .unwrap();

        assert!(manager.take_frames("track").unwrap().is_some());
        assert!(manager.take_frames("track").unwrap().is_none());
        assert_eq!(manager.cached_bytes().unwrap(), 0);
    }

    #[test]
    fn rejects_private_and_reserved_network_addresses() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(!is_public_ip(address.parse().unwrap()), "{address}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700::1111".parse().unwrap()));
    }

    #[test]
    fn maps_common_media_extensions_to_lavaplayer_probes() {
        assert_eq!(probe_name(Some("mp3"), ""), Some("MPEG"));
        assert_eq!(probe_name(Some("wav"), ""), Some("WAV"));
        assert_eq!(probe_name(Some("flac"), ""), Some("FLAC"));
        assert_eq!(probe_name(Some("ogg"), ""), Some("OGG"));
        assert_eq!(probe_name(Some("aac"), ""), Some("ADTS"));
        assert_eq!(probe_name(Some("unknown"), ""), None);
    }

    #[test]
    fn maps_content_types_when_urls_have_no_extension() {
        assert_eq!(probe_name(None, "audio/mpeg; charset=binary"), Some("MPEG"));
        assert_eq!(probe_name(None, "audio/ogg"), Some("OGG"));
    }

    #[test]
    fn derives_a_title_from_the_url_path() {
        let url =
            reqwest::Url::parse("https://cdn.example/audio/demo-track.mp3?token=abc").unwrap();
        assert_eq!(media_title(&url), "demo-track.mp3");
    }
}
