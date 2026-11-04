//! Native YouTube source plugin.
//!
//! The default extractor uses an Invidious-compatible API. Configure one or
//! more comma-separated instance URLs with RAVALINK_YOUTUBE_API_BASE for failover.

use async_trait::async_trait;
use ravalink_plugin_api::{
    PluginDescriptor, PluginError, PluginLoadResult, PluginLoadType, RavaLinkPlugin,
};
use ravalink_protocol::{Track, TrackInfo, encode_track};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone)]
pub struct YouTubePlugin {
    client: reqwest::Client,
    api_bases: Vec<String>,
}

impl Default for YouTubePlugin {
    fn default() -> Self {
        Self::new(
            std::env::var("RAVALINK_YOUTUBE_API_BASE")
                .unwrap_or_else(|_| "https://inv.nadeko.net".to_owned()),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeIdentifier {
    pub video_id: Option<String>,
    pub playlist_id: Option<String>,
    pub start_seconds: Option<u64>,
}

impl YouTubePlugin {
    pub fn new(api_base: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("RavaLink/0.1 (native YouTube source)")
            .build()
            .expect("valid default HTTP client");
        let api_base = api_base.into();
        let api_bases = api_base
            .split(',')
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .map(|base| base.trim_end_matches('/').to_owned())
            .collect::<Vec<_>>();
        let api_bases = if api_bases.is_empty() {
            vec!["https://inv.nadeko.net".to_owned()]
        } else {
            api_bases
        };
        Self { client, api_bases }
    }

    pub fn parse_identifier(identifier: &str) -> Option<YouTubeIdentifier> {
        let trimmed = identifier.trim();
        let url = parse_url(trimmed)?;
        let mut video_id = None;

        match url.host.as_str() {
            "youtu.be" | "www.youtu.be" => {
                video_id = url
                    .path
                    .split('/')
                    .find(|part| !part.is_empty())
                    .map(str::to_owned);
            }
            "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
                if url.path == "/watch" {
                    video_id = url.query_value("v").map(str::to_owned);
                } else if url.path.starts_with("/shorts/")
                    || url.path.starts_with("/embed/")
                    || url.path.starts_with("/live/")
                {
                    video_id = url.path.split('/').nth(2).map(str::to_owned);
                }
            }
            _ => return None,
        }

        let playlist_id = url.query_value("list").map(str::to_owned);
        let start_seconds = url
            .query_value("t")
            .and_then(parse_timestamp)
            .or_else(|| url.query_value("start").and_then(parse_timestamp));

        let video_id = video_id.filter(|id| valid_video_id(id));
        if video_id.is_none() && playlist_id.is_none() {
            return None;
        }

        Some(YouTubeIdentifier {
            video_id,
            playlist_id,
            start_seconds,
        })
    }

    pub fn is_search(identifier: &str) -> bool {
        identifier.starts_with("ytsearch:") || identifier.starts_with("ytmsearch:")
    }

    pub fn search_query(identifier: &str) -> Option<&str> {
        identifier
            .strip_prefix("ytsearch:")
            .or_else(|| identifier.strip_prefix("ytmsearch:"))
            .map(str::trim)
            .filter(|query| !query.is_empty())
    }

    async fn request_json(&self, path: &str) -> Result<Value, PluginError> {
        let mut failures = Vec::new();
        for api_base in &self.api_bases {
            let url = format!("{api_base}{path}");
            let response = match self.client.get(&url).send().await {
                Ok(response) => match response.error_for_status() {
                    Ok(response) => response,
                    Err(error) => {
                        failures.push(format!("{api_base}: HTTP request failed: {error}"));
                        continue;
                    }
                },
                Err(error) => {
                    failures.push(format!("{api_base}: request failed: {error}"));
                    continue;
                }
            };
            let body = match response.text().await {
                Ok(body) => body,
                Err(error) => {
                    failures.push(format!("{api_base}: cannot read response: {error}"));
                    continue;
                }
            };
            match serde_json::from_str(&body) {
                Ok(data) => return Ok(data),
                Err(error) => failures.push(format!("{api_base}: invalid JSON response: {error}")),
            }
        }
        if failures.is_empty() {
            return Err(PluginError::new(
                "youtube",
                "no Invidious API base URL is configured",
            ));
        }
        Err(PluginError::new(
            "youtube",
            format!(
                "all configured Invidious API instances failed: {}",
                failures.join("; ")
            ),
        ))
    }

    async fn load_video(&self, video_id: &str) -> Result<Track, PluginError> {
        let data = self
            .request_json(&format!("/api/v1/videos/{video_id}"))
            .await?;
        let mut track = track_from_video(&data)?;
        let stream_url = data
            .get("adaptiveFormats")
            .and_then(Value::as_array)
            .and_then(|formats| select_audio_format(formats))
            .and_then(|format| format.get("url").and_then(Value::as_str))
            .ok_or_else(|| PluginError::new("youtube", "no playable audio format was returned"))?;
        track.plugin_info = json!({
            "streamUrl": stream_url,
            "provider": "invidious",
        });
        Ok(track)
    }

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<Track>, PluginError> {
        let result = self.search(query).await?;
        serde_json::from_value(result.data)
            .map_err(|error| PluginError::new("youtube", format!("invalid search tracks: {error}")))
    }

    pub async fn resolve_video(&self, video_id: &str) -> Result<Track, PluginError> {
        self.load_video(video_id).await
    }

    async fn search(&self, query: &str) -> Result<PluginLoadResult, PluginError> {
        let encoded_query = url_encode(query);
        let data = self
            .request_json(&format!("/api/v1/search?q={encoded_query}&type=video"))
            .await?;
        let items = data
            .as_array()
            .ok_or_else(|| PluginError::new("youtube", "search response is not an array"))?;
        let tracks = items
            .iter()
            .filter_map(|item| track_from_video(item).ok())
            .collect::<Vec<_>>();
        Ok(PluginLoadResult::new(
            PluginLoadType::Search,
            serde_json::to_value(tracks).expect("track list serializes"),
        ))
    }

    async fn load_playlist(&self, playlist_id: &str) -> Result<PluginLoadResult, PluginError> {
        let data = self
            .request_json(&format!("/api/v1/playlists/{playlist_id}"))
            .await?;
        let videos = data
            .get("videos")
            .and_then(Value::as_array)
            .ok_or_else(|| PluginError::new("youtube", "playlist response has no videos"))?;
        let tracks = videos
            .iter()
            .filter_map(|video| track_from_video(video).ok())
            .collect::<Vec<_>>();
        let name = data
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("YouTube playlist");
        Ok(PluginLoadResult::new(
            PluginLoadType::Playlist,
            json!({
                "info": {"name": name, "selectedTrack": -1},
                "tracks": tracks,
            }),
        ))
    }
}

#[async_trait]
impl RavaLinkPlugin for YouTubePlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "youtube",
            version: env!("CARGO_PKG_VERSION"),
            description: "YouTube video, playlist and search source",
            capabilities: &["video", "playlist", "search", "music-search"],
        }
    }

    fn supports_identifier(&self, identifier: &str) -> bool {
        Self::parse_identifier(identifier).is_some() || Self::is_search(identifier)
    }

    async fn load_tracks(&self, identifier: &str) -> Result<Option<PluginLoadResult>, PluginError> {
        if let Some(query) = Self::search_query(identifier) {
            return self.search(query).await.map(Some);
        }
        let parsed = Self::parse_identifier(identifier)
            .ok_or_else(|| PluginError::new("youtube", "invalid YouTube identifier"))?;
        if let Some(playlist_id) = parsed.playlist_id {
            let mut result = self.load_playlist(&playlist_id).await?;
            if let Some(video_id) = parsed.video_id {
                let tracks = result
                    .data
                    .get_mut("tracks")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| {
                        PluginError::new("youtube", "playlist response has no tracks")
                    })?;
                let selected = tracks.iter().position(|track| {
                    track
                        .get("info")
                        .and_then(|info| info.get("identifier"))
                        .and_then(Value::as_str)
                        == Some(video_id.as_str())
                });
                if let Some(index) = selected {
                    if let Some(start_seconds) = parsed.start_seconds {
                        let mut track: Track = serde_json::from_value(tracks[index].clone())
                            .map_err(|error| {
                                PluginError::new(
                                    "youtube",
                                    format!("invalid playlist track: {error}"),
                                )
                            })?;
                        track.info.position = start_seconds.saturating_mul(1_000);
                        track.encoded = encode_track(&track)
                            .map_err(|error| PluginError::new("youtube", error.message))?;
                        tracks[index] =
                            serde_json::to_value(track).expect("playlist track serializes");
                    }
                    result.data["info"]["selectedTrack"] = json!(index);
                }
            }
            return Ok(Some(result));
        }
        if let Some(video_id) = parsed.video_id {
            let mut track = self.load_video(&video_id).await?;
            if let Some(start_seconds) = parsed.start_seconds {
                track.info.position = start_seconds.saturating_mul(1_000);
                track.encoded = encode_track(&track)
                    .map_err(|error| PluginError::new("youtube", error.message))?;
            }
            return Ok(Some(PluginLoadResult::new(
                PluginLoadType::Track,
                serde_json::to_value(track).expect("track serializes"),
            )));
        }
        Ok(None)
    }
}

fn select_audio_format(formats: &[Value]) -> Option<&Value> {
    formats
        .iter()
        .filter(|format| {
            format
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|mime| mime.starts_with("audio/"))
        })
        .max_by_key(|format| {
            let mime = format
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let prefer_aac = mime.starts_with("audio/mp4") || mime.contains("mp4a");
            let bitrate = format
                .get("bitrate")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            (prefer_aac, bitrate)
        })
}

fn track_from_video(video: &Value) -> Result<Track, PluginError> {
    let video_id = video
        .get("videoId")
        .and_then(Value::as_str)
        .ok_or_else(|| PluginError::new("youtube", "video result has no videoId"))?;
    let title = video
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Unknown title");
    let author = video
        .get("author")
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let length = video
        .get("lengthSeconds")
        .and_then(Value::as_i64)
        .unwrap_or_default()
        .saturating_mul(1_000);
    let artwork_url = video
        .get("videoThumbnails")
        .and_then(Value::as_array)
        .and_then(|thumbnails| thumbnails.last())
        .and_then(|thumbnail| thumbnail.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut track = Track {
        encoded: String::new(),
        info: TrackInfo {
            identifier: video_id.to_owned(),
            is_seekable: true,
            author: author.to_owned(),
            length,
            is_stream: false,
            position: 0,
            title: title.to_owned(),
            uri: Some(format!("https://www.youtube.com/watch?v={video_id}")),
            source_name: "youtube".to_owned(),
            artwork_url,
            isrc: None,
        },
        plugin_info: json!({}),
        user_data: json!({}),
    };
    track.encoded =
        encode_track(&track).map_err(|error| PluginError::new("youtube", error.message))?;
    Ok(track)
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }

    let mut total = 0_u64;
    let mut digits = String::new();
    let mut saw_unit = false;
    for character in value.chars() {
        if character.is_ascii_digit() {
            digits.push(character);
            continue;
        }
        let multiplier = match character {
            'h' => 3_600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        let amount = digits.parse::<u64>().ok()?;
        total = total.checked_add(amount.checked_mul(multiplier)?)?;
        digits.clear();
        saw_unit = true;
    }
    if !digits.is_empty() {
        let amount = digits.parse::<u64>().ok()?;
        if !saw_unit {
            return Some(amount);
        }
        total = total.checked_add(amount)?;
    }
    saw_unit.then_some(total)
}

fn valid_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

struct ParsedUrl<'a> {
    host: String,
    path: &'a str,
    query: &'a str,
}

impl ParsedUrl<'_> {
    fn query_value(&self, key: &str) -> Option<&str> {
        self.query.split('&').find_map(|part| {
            let (name, value) = part.split_once('=')?;
            (name == key).then_some(value)
        })
    }
}

fn parse_url(value: &str) -> Option<ParsedUrl<'_>> {
    let (_, rest) = value.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let suffix = &rest[authority_end..];
    let path_end = suffix.find(['?', '#']).unwrap_or(suffix.len());
    let path = if path_end == 0 {
        "/"
    } else {
        &suffix[..path_end]
    };
    let query = suffix
        .find('?')
        .map(|index| suffix[index + 1..].split('#').next().unwrap_or(""))
        .unwrap_or("");
    Some(ParsedUrl { host, path, query })
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_comma_separated_invidious_failover_list() {
        let plugin = YouTubePlugin::new(
            "https://first.example/, https://second.example, ,https://third.example/",
        );
        assert_eq!(
            plugin.api_bases,
            vec![
                "https://first.example".to_owned(),
                "https://second.example".to_owned(),
                "https://third.example".to_owned(),
            ]
        );
    }

    async fn one_shot_http_response(status: &str, body: &str) -> String {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let body = body.to_owned();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn request_json_fails_over_to_the_next_invidious_instance() {
        let unavailable = one_shot_http_response("503 Service Unavailable", "").await;
        let healthy = one_shot_http_response("200 OK", r#"{"ok":true}"#).await;
        let plugin = YouTubePlugin::new(format!("{unavailable},{healthy}"));

        let response = plugin.request_json("/api/v1/health").await.unwrap();
        assert_eq!(response["ok"], true);
    }

    #[test]
    fn parses_standard_short_and_shorts_urls() {
        assert_eq!(
            YouTubePlugin::parse_identifier("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
                .unwrap()
                .video_id
                .as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            YouTubePlugin::parse_identifier("https://youtu.be/dQw4w9WgXcQ?t=42")
                .unwrap()
                .start_seconds,
            Some(42)
        );
        assert_eq!(
            YouTubePlugin::parse_identifier("https://youtube.com/shorts/dQw4w9WgXcQ")
                .unwrap()
                .video_id
                .as_deref(),
            Some("dQw4w9WgXcQ")
        );
    }

    #[test]
    fn preserves_playlist_identifiers_and_search_prefixes() {
        let parsed = YouTubePlugin::parse_identifier(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL123",
        )
        .unwrap();
        assert_eq!(parsed.playlist_id.as_deref(), Some("PL123"));

        let playlist =
            YouTubePlugin::parse_identifier("https://www.youtube.com/playlist?list=PL123").unwrap();
        assert_eq!(playlist.video_id, None);
        assert_eq!(playlist.playlist_id.as_deref(), Some("PL123"));

        assert_eq!(
            YouTubePlugin::search_query("ytsearch:lofi hip hop"),
            Some("lofi hip hop")
        );
        assert!(YouTubePlugin::is_search("ytmsearch:ambient"));
    }

    #[test]
    fn parses_numeric_and_unit_based_video_timestamps() {
        assert_eq!(parse_timestamp("90"), Some(90));
        assert_eq!(parse_timestamp("90s"), Some(90));
        assert_eq!(parse_timestamp("1m30s"), Some(90));
        assert_eq!(parse_timestamp("1h2m3s"), Some(3_723));
        assert_eq!(parse_timestamp("not-a-time"), None);
        assert_eq!(
            YouTubePlugin::parse_identifier("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1m30s")
                .unwrap()
                .start_seconds,
            Some(90)
        );
    }

    #[test]
    fn rejects_non_youtube_and_malformed_video_ids() {
        assert!(YouTubePlugin::parse_identifier("https://example.com/watch?v=123").is_none());
        assert!(YouTubePlugin::parse_identifier("https://youtube.com/watch?v=short").is_none());
    }

    #[test]
    fn prefers_aac_audio_when_multiple_codecs_are_available() {
        let formats = vec![
            json!({"type": "audio/webm; codecs=opus", "bitrate": 160_000, "url": "https://cdn.test/opus"}),
            json!({"type": "audio/mp4; codecs=mp4a.40.2", "bitrate": 96_000, "url": "https://cdn.test/aac"}),
        ];
        assert_eq!(
            select_audio_format(&formats)
                .and_then(|format| format.get("url"))
                .and_then(Value::as_str),
            Some("https://cdn.test/aac")
        );
    }

    #[test]
    fn creates_lavalink_track_metadata_from_video_json() {
        let track = track_from_video(&json!({
            "videoId": "dQw4w9WgXcQ",
            "title": "Example",
            "author": "Artist",
            "lengthSeconds": 210
        }))
        .unwrap();
        assert_eq!(track.info.source_name, "youtube");
        assert_eq!(track.info.length, 210_000);
        assert!(!track.encoded.is_empty());
    }
}
