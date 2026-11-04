//! Direct HTTP media source adapter with bounded compressed-source caching.

use crate::{AudioError, PcmFrame, decode_bytes_limited, decode_bytes_with};
use ravalink_protocol::{Track, TrackInfo, encode_track_with_source_data};
use serde_json::json;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

const DEFAULT_CACHE_BYTES: usize = 256 * 1024 * 1024;
const DEFAULT_DOWNLOAD_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_CONCURRENT_RESOLVES: usize = 2;
const DEFAULT_CONCURRENT_STREAMS: usize = 128;
const MAX_PCM_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct HttpSourceManager {
    cache: Arc<Mutex<CacheState>>,
    max_cache_bytes: usize,
    max_download_bytes: usize,
    resolve_slots: Arc<tokio::sync::Semaphore>,
    stream_slots: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<String, CacheEntry>,
    bytes_used: usize,
    sequence: u64,
}

struct CacheEntry {
    source: Arc<[u8]>,
    extension: Option<String>,
    max_pcm_bytes: usize,
    bytes: usize,
    sequence: u64,
}

#[derive(Debug, Clone)]
pub struct CachedHttpSource {
    pub bytes: Arc<[u8]>,
    pub extension: Option<String>,
    pub max_pcm_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct ResolvedHttpTrack {
    pub track: Track,
    pub duration_ms: u64,
}

impl Default for HttpSourceManager {
    fn default() -> Self {
        Self::with_limits(DEFAULT_DOWNLOAD_BYTES, DEFAULT_CACHE_BYTES)
    }
}

impl HttpSourceManager {
    pub fn new(max_cache_bytes: usize) -> Self {
        Self::with_limits(DEFAULT_DOWNLOAD_BYTES, max_cache_bytes)
    }

    pub fn with_limits(max_download_bytes: usize, max_cache_bytes: usize) -> Self {
        Self {
            cache: Arc::new(Mutex::new(CacheState::default())),
            max_cache_bytes,
            max_download_bytes,
            resolve_slots: Arc::new(tokio::sync::Semaphore::new(DEFAULT_CONCURRENT_RESOLVES)),
            stream_slots: Arc::new(tokio::sync::Semaphore::new(DEFAULT_CONCURRENT_STREAMS)),
        }
    }

    /// Resolve HTTP media metadata without downloading the media body.
    ///
    /// Duration is intentionally unknown at this stage. The actual stream is
    /// opened when a player starts, allowing playback to begin before EOF.
    pub async fn resolve_stream_track(
        &self,
        identifier: &str,
    ) -> Result<ResolvedHttpTrack, AudioError> {
        let url = reqwest::Url::parse(identifier)
            .map_err(|error| AudioError::invalid(format!("invalid media URL: {error}")))?;
        let client = self.streaming_client(&url).await?;
        let response = tokio::time::timeout(
            Duration::from_secs(15),
            client
                .get(url.clone())
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send(),
        )
        .await
        .map_err(|_| AudioError::invalid("HTTP source metadata request timed out"))?
        .map_err(|error| {
            AudioError::invalid(format!("HTTP source metadata request failed: {error}"))
        })?
        .error_for_status()
        .map_err(|error| {
            AudioError::invalid(format!("HTTP source returned an error: {error}"))
        })?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let extension = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .and_then(|segment| segment.rsplit_once('.').map(|(_, extension)| extension))
            .map(str::to_ascii_lowercase);
        let probe = probe_name(extension.as_deref(), content_type)
            .ok_or_else(|| AudioError::invalid("unsupported direct HTTP audio format"))?;
        let mut track = Track {
            encoded: String::new(),
            info: TrackInfo {
                identifier: identifier.to_owned(),
                is_seekable: false,
                author: "Unknown".into(),
                length: 0,
                position: 0,
                is_stream: true,
                title: media_title(&url),
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
        Ok(ResolvedHttpTrack {
            track,
            duration_ms: 0,
        })
    }

    /// Fetch, demux and decode an HTTP response concurrently with network input.
    /// Both the compressed-byte bridge and decoded-PCM queue are bounded.
    pub async fn stream_to_channel(
        &self,
        identifier: &str,
        max_pcm_bytes: usize,
        position_ms: u64,
        sender: tokio::sync::mpsc::Sender<Result<PcmFrame, AudioError>>,
    ) -> Result<(), AudioError> {
        let permit = self
            .stream_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AudioError::invalid("HTTP stream manager is shutting down"))?;
        let url = reqwest::Url::parse(identifier)
            .map_err(|error| AudioError::invalid(format!("invalid media URL: {error}")))?;
        let client = self.streaming_client(&url).await?;
        let mut response = tokio::time::timeout(
            Duration::from_secs(15),
            client.get(url.clone()).send(),
        )
        .await
        .map_err(|_| AudioError::invalid("HTTP source response headers timed out"))?
        .map_err(|error| AudioError::invalid(format!("HTTP source request failed: {error}")))?
        .error_for_status()
        .map_err(|error| {
            AudioError::invalid(format!("HTTP source returned an error: {error}"))
        })?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let extension = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .and_then(|segment| segment.rsplit_once('.').map(|(_, extension)| extension))
            .map(str::to_ascii_lowercase);
        let probe = probe_name(extension.as_deref(), &content_type)
            .ok_or_else(|| AudioError::invalid("unsupported direct HTTP audio format"))?;
        let (compressed_sender, compressed_receiver) = tokio::sync::mpsc::channel(4);
        let producer = tokio::spawn(async move {
            let _permit = permit;
            loop {
                let chunk =
                    match tokio::time::timeout(Duration::from_secs(60), response.chunk()).await {
                        Err(_) => {
                            let message = "HTTP media stream stalled for 60 seconds".to_owned();
                            let _ = compressed_sender.send(Err(message.clone())).await;
                            return Err(AudioError::invalid(message));
                        }
                        Ok(Err(error)) => {
                            let message = format!("HTTP media stream failed: {error}");
                            let _ = compressed_sender.send(Err(message.clone())).await;
                            return Err(AudioError::invalid(message));
                        }
                        Ok(Ok(chunk)) => chunk,
                    };
                let Some(chunk) = chunk else {
                    return Ok::<(), AudioError>(());
                };
                if compressed_sender.send(Ok(chunk.to_vec())).await.is_err() {
                    return Ok(());
                }
            }
        });

        let decoded = crate::decode_stream_to_channel_at(
            compressed_receiver,
            Some(
                match probe {
                    "MPEG" => "mp3",
                    "WAV" => "wav",
                    "FLAC" => "flac",
                    "OGG" => "ogg",
                    "ADTS" => "aac",
                    "MP4" => "m4a",
                    "MATROSKA" => "webm",
                    _ => unreachable!("probe_name returns a known decoder format"),
                }
                .to_owned(),
            ),
            max_pcm_bytes,
            position_ms,
            sender,
        )
        .await;
        if decoded.is_err() {
            producer.abort();
        }
        let network_result = producer
            .await
            .map_err(|error| AudioError::invalid(format!("HTTP stream task failed: {error}")))?;
        decoded?;
        network_result
    }

    async fn streaming_client(&self, url: &reqwest::Url) -> Result<reqwest::Client, AudioError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(AudioError::invalid(
                "only HTTP and HTTPS media URLs are supported",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(AudioError::invalid(
                "HTTP source URLs must not embed credentials",
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
            Err(_) => tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host((host, port)),
            )
            .await
            .map_err(|_| AudioError::invalid("HTTP source DNS lookup timed out"))?
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
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent("RavaLink/0.1 native-audio")
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|error| AudioError::invalid(format!("cannot build HTTP client: {error}")))
    }

    async fn streaming_client(&self, url: &reqwest::Url) -> Result<reqwest::Client, AudioError> {
        let host = url
            .host_str()
            .ok_or_else(|| AudioError::invalid("HTTP source URL has no host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| AudioError::invalid("HTTP source URL has no known port"))?;
        let addresses = match host.parse::<IpAddr>() {
            Ok(address) => vec![SocketAddr::new(address, port)],
            Err(_) => tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host((host, port)),
            )
            .await
            .map_err(|_| AudioError::invalid("HTTP source DNS lookup timed out"))?
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
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent("RavaLink/0.1 native-audio")
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|error| AudioError::invalid(format!("cannot build HTTP client: {error}")))
    }

    /// Resolve and decode a direct HTTP(S) media URL.
    ///
    /// This initial adapter buffers the source before decoding. It is intended
    /// for bounded files, not live radio streams or large remote archives.
    pub async fn resolve(&self, identifier: &str) -> Result<ResolvedHttpTrack, AudioError> {
        let resolve_permit = self
            .resolve_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AudioError::invalid("HTTP source resolver is shutting down"))?;
        let url = reqwest::Url::parse(identifier)
            .map_err(|error| AudioError::invalid(format!("invalid media URL: {error}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(AudioError::invalid(
                "only HTTP and HTTPS media URLs are supported",
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(AudioError::invalid(
                "HTTP source URLs must not embed credentials",
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
            Err(_) => tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host((host, port)),
            )
            .await
            .map_err(|_| AudioError::invalid("HTTP source DNS lookup timed out"))?
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
            .is_some_and(|length| length > self.max_download_bytes as u64)
        {
            return Err(AudioError::invalid(format!(
                "HTTP source exceeds the configured {} byte download limit",
                self.max_download_bytes
            )));
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
            if body.len().saturating_add(chunk.len()) > self.max_download_bytes {
                return Err(AudioError::invalid(format!(
                    "HTTP source exceeds the configured {} byte download limit",
                    self.max_download_bytes
                )));
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
        // Reject a source that cannot fit before spending CPU on a duration scan.
        if body.len() > self.max_cache_bytes {
            return Err(AudioError::invalid(format!(
                "compressed HTTP source requires {} cache bytes, exceeding the configured {} byte limit",
                body.len(),
                self.max_cache_bytes
            )));
        }
        // Validate and measure off the async executor, discarding each PCM packet.
        let source_bytes: Arc<[u8]> = body.into();
        let decode_source = Arc::clone(&source_bytes);
        let decode_extension = extension.clone();
        let max_pcm_bytes = MAX_PCM_FRAME_BYTES;
        let (frame_count, sample_rate) = tokio::task::spawn_blocking(move || {
            // Keep the slot occupied if the async request is cancelled while
            // this non-cancellable blocking decode is still running.
            let _resolve_permit = resolve_permit;
            let mut frame_count = 0_u64;
            let mut sample_rate = None;
            let mut channels = None;
            decode_bytes_with(
                decode_source,
                decode_extension.as_deref(),
                max_pcm_bytes,
                |frame| {
                    if frame.sample_rate == 0 || frame.channels == 0 {
                        return Err(AudioError::invalid("HTTP source has an invalid PCM layout"));
                    }
                    match (sample_rate, channels) {
                        (Some(rate), Some(count))
                            if rate != frame.sample_rate || count != frame.channels =>
                        {
                            return Err(AudioError::invalid(
                                "HTTP source changes its PCM layout mid-track",
                            ));
                        }
                        _ => {
                            sample_rate = Some(frame.sample_rate);
                            channels = Some(frame.channels);
                        }
                    }
                    frame_count = frame_count
                        .saturating_add((frame.samples.len() / usize::from(frame.channels)) as u64);
                    Ok(())
                },
            )?;
            let rate = sample_rate
                .ok_or_else(|| AudioError::invalid("HTTP source contains no decoded audio"))?;
            Ok::<_, AudioError>((frame_count, rate))
        })
        .await
        .map_err(|error| AudioError::invalid(format!("audio decoder task failed: {error}")))??;
        let duration_ms = frame_count.saturating_mul(1_000) / u64::from(sample_rate);

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
        self.insert_cache(
            track.encoded.clone(),
            source_bytes,
            extension,
            max_pcm_bytes,
        )?;
        Ok(ResolvedHttpTrack { track, duration_ms })
    }

    /// Take ownership of the compressed source bytes for playback.
    pub fn take_source(&self, encoded_track: &str) -> Result<Option<CachedHttpSource>, AudioError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| AudioError::invalid("HTTP source cache lock is poisoned"))?;
        let Some(entry) = cache.entries.remove(encoded_track) else {
            return Ok(None);
        };
        cache.bytes_used = cache.bytes_used.saturating_sub(entry.bytes);
        Ok(Some(CachedHttpSource {
            bytes: entry.source,
            extension: entry.extension,
            max_pcm_bytes: entry.max_pcm_bytes,
        }))
    }

    /// Compatibility helper for callers that explicitly need a seekable decoded buffer.
    pub fn take_frames(&self, encoded_track: &str) -> Result<Option<Vec<PcmFrame>>, AudioError> {
        let Some(source) = self.take_source(encoded_track)? else {
            return Ok(None);
        };
        Ok(Some(decode_bytes_limited(
            source.bytes,
            source.extension.as_deref(),
            source.max_pcm_bytes,
        )?))
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
        source: Arc<[u8]>,
        extension: Option<String>,
        max_pcm_bytes: usize,
    ) -> Result<(), AudioError> {
        let bytes = source.len();
        if bytes > self.max_cache_bytes {
            return Err(AudioError::invalid(format!(
                "compressed HTTP source requires {bytes} cache bytes, exceeding the configured {} byte limit",
                self.max_cache_bytes
            )));
        }
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
                source,
                extension,
                max_pcm_bytes,
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
                && !(a == 192 && b == 0 && (c == 0 || c == 2))
                && !(a == 192 && b == 88 && c == 99)
                && a != 0
                && !(a == 198 && (b == 18 || b == 19))
                && !(a == 198 && b == 51 && c == 100)
                && !(a == 203 && b == 0 && c == 113)
                && a < 224
        }
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(mapped));
            }
            let segments = address.segments();
            // Only globally-routable unicast space is safe for remote media
            // fetches. Merely excluding RFC 1918/4193 and a few special
            // prefixes would still admit reserved and protocol-assignment
            // ranges such as 4000::/3 and 5f00::/16.
            let global_unicast = segments[0] & 0xe000 == 0x2000;
            let ipv4_compatible = segments[..6].iter().all(|segment| *segment == 0);
            let ipv4_translated =
                segments[..4].iter().all(|segment| *segment == 0) && segments[4] == 0xffff;
            let nat64_well_known = segments[0] == 0x0064
                && segments[1] == 0xff9b
                && segments[2..6].iter().all(|segment| *segment == 0);
            let nat64_local_use =
                segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 1;
            let discard_only =
                segments[0] == 0x0100 && segments[1..4].iter().all(|segment| *segment == 0);
            let ietf_protocol = segments[0] == 0x2001 && segments[1] <= 0x01ff;
            let documentation = (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || (segments[0] == 0x3fff && segments[1] <= 0x0fff);
            global_unicast
                && !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
                && !ipv4_compatible
                && !ipv4_translated
                && !nat64_well_known
                && !nat64_local_use
                && !discard_only
                && !ietf_protocol
                && !documentation
                && segments[0] != 0x2002
                && segments[0] & 0xffc0 != 0xfec0
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

    #[tokio::test]
    async fn progressive_stream_resolution_rejects_credentials_and_non_http_schemes() {
        let manager = HttpSourceManager::default();
        for (identifier, expected) in [
            (
                "https://user:secret@example.com/audio.mp3",
                "must not embed credentials",
            ),
            ("file:///etc/passwd", "only HTTP and HTTPS"),
        ] {
            let error = manager.resolve_stream_track(identifier).await.unwrap_err();
            assert!(
                error.message.contains(expected),
                "unexpected error for {identifier}: {}",
                error.message
            );
        }
    }

    #[tokio::test]
    async fn rejects_embedded_http_credentials_before_network_access() {
        let manager = HttpSourceManager::default();
        let error = manager
            .resolve("https://user:secret@example.com/audio.mp3")
            .await
            .unwrap_err();
        assert!(error.message.contains("must not embed credentials"));
    }

    #[test]
    fn cloned_http_source_managers_share_a_bounded_resolution_semaphore() {
        let manager = HttpSourceManager::new(64);
        let clone = manager.clone();
        assert_eq!(manager.resolve_slots.available_permits(), 2);
        assert_eq!(manager.stream_slots.available_permits(), DEFAULT_CONCURRENT_STREAMS);

        let first = manager.resolve_slots.clone().try_acquire_owned().unwrap();
        let second = clone.resolve_slots.clone().try_acquire_owned().unwrap();
        assert_eq!(manager.resolve_slots.available_permits(), 0);
        assert!(manager.resolve_slots.clone().try_acquire_owned().is_err());

        drop(first);
        assert_eq!(manager.resolve_slots.available_permits(), 1);
        drop(second);
        assert_eq!(manager.resolve_slots.available_permits(), 2);
    }

    #[test]
    fn compressed_source_cache_evicts_oldest_entries_to_respect_its_limit() {
        let manager = HttpSourceManager::new(16);
        manager
            .insert_cache(
                "first".into(),
                Arc::from(vec![1; 8]),
                Some("mp3".into()),
                64,
            )
            .unwrap();
        manager
            .insert_cache(
                "second".into(),
                Arc::from(vec![2; 12]),
                Some("mp3".into()),
                64,
            )
            .unwrap();

        assert_eq!(manager.cached_bytes().unwrap(), 12);
        assert!(manager.take_source("first").unwrap().is_none());
        assert_eq!(
            manager.take_source("second").unwrap().unwrap().bytes.len(),
            12
        );
        assert_eq!(manager.cached_bytes().unwrap(), 0);
    }

    #[test]
    fn rejects_entries_larger_than_the_entire_cache_without_evicting_or_caching_them() {
        let manager = HttpSourceManager::new(16);
        manager
            .insert_cache("kept".into(), Arc::from(vec![1; 8]), Some("mp3".into()), 64)
            .unwrap();
        let error = manager
            .insert_cache(
                "oversized".into(),
                Arc::from(vec![2; 17]),
                Some("mp3".into()),
                64,
            )
            .unwrap_err();

        assert!(
            error
                .message
                .contains("exceeding the configured 16 byte limit")
        );
        assert_eq!(manager.cached_bytes().unwrap(), 8);
        assert!(manager.take_source("oversized").unwrap().is_none());
        assert!(manager.take_source("kept").unwrap().is_some());
        assert_eq!(manager.cached_bytes().unwrap(), 0);
    }

    #[test]
    fn taking_a_compressed_source_removes_the_cache_entry() {
        let manager = HttpSourceManager::new(64);
        manager
            .insert_cache(
                "track".into(),
                Arc::from(vec![1; 8]),
                Some("mp3".into()),
                64,
            )
            .unwrap();
        let source = manager.take_source("track").unwrap().unwrap();
        let clone = source.clone();
        assert!(Arc::ptr_eq(&source.bytes, &clone.bytes));
        assert!(manager.take_source("track").unwrap().is_none());
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
            "192.0.2.1",
            "192.88.99.1",
            "198.51.100.10",
            "203.0.113.1",
            "240.0.0.1",
            "::1",
            "::a00:1",
            "::ffff:10.0.0.1",
            "::ffff:0:0:0:a00:1",
            "64:ff9b::a00:1",
            "64:ff9b:1::a00:1",
            "100::1",
            "2001:1::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "3fff::1",
            "4000::1",
            "5f00::1",
            "fec0::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(!is_public_ip(address.parse().unwrap()), "{address}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700::1111".parse().unwrap()));
        assert!(is_public_ip("2001:4860:4860::8888".parse().unwrap()));
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
