//! Native LavaSrc-compatible provider resolver.
//!
//! Spotify and Apple Music metadata are resolved through their public embed/catalog
//! endpoints; Deezer uses its public API. Playable audio is matched against YouTube,
//! so the actual audio path remains within RavaLink's native source pipeline.

use async_trait::async_trait;
use ravalink_plugin_api::{
    PluginDescriptor, PluginError, PluginLoadResult, PluginLoadType, RavaLinkPlugin,
};
use ravalink_plugin_youtube::YouTubePlugin;
use ravalink_protocol::{Track, encode_track};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicProvider {
    Spotify,
    AppleMusic,
    Deezer,
}

impl MusicProvider {
    fn source_name(self) -> &'static str {
        match self {
            Self::Spotify => "spotify",
            Self::AppleMusic => "applemusic",
            Self::Deezer => "deezer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIdentifier {
    pub provider: MusicProvider,
    pub content_type: String,
    pub id: String,
}

#[derive(Clone)]
pub struct LavaSrcPlugin {
    youtube: YouTubePlugin,
    client: reqwest::Client,
}

#[derive(Debug, Clone)]
struct ExternalTrack {
    title: String,
    author: String,
    duration_ms: Option<u64>,
    artwork_url: Option<String>,
    isrc: Option<String>,
}

impl Default for LavaSrcPlugin {
    fn default() -> Self {
        Self {
            youtube: YouTubePlugin::default(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("RavaLink/0.1 (native LavaSrc adapter)")
                .build()
                .expect("valid default HTTP client"),
        }
    }
}

impl LavaSrcPlugin {
    pub fn parse_identifier(identifier: &str) -> Option<ProviderIdentifier> {
        let url = split_url(identifier)?;
        let segments: Vec<&str> = url
            .path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        let provider = match url.host.as_str() {
            "open.spotify.com" | "spotify.com" | "www.spotify.com" => MusicProvider::Spotify,
            "music.apple.com" | "itunes.apple.com" => MusicProvider::AppleMusic,
            "www.deezer.com" | "deezer.com" => MusicProvider::Deezer,
            _ => return None,
        };

        let (content_type, id) = match provider {
            MusicProvider::Spotify => {
                let kind_index = segments
                    .iter()
                    .position(|part| matches!(*part, "track" | "album" | "playlist" | "artist"))?;
                (*segments.get(kind_index)?, *segments.get(kind_index + 1)?)
            }
            MusicProvider::AppleMusic => {
                let kind = *segments.get(1)?;
                if let Some(track_id) = url.query_value("i") {
                    ("track", track_id)
                } else {
                    let normalized_kind = if kind == "song" { "track" } else { kind };
                    (normalized_kind, segments.last()?.trim_start_matches("id"))
                }
            }
            MusicProvider::Deezer => {
                let kind_index = segments
                    .iter()
                    .position(|part| matches!(*part, "track" | "album" | "playlist" | "artist"))?;
                (*segments.get(kind_index)?, *segments.get(kind_index + 1)?)
            }
        };

        if !matches!(content_type, "track" | "album" | "playlist" | "artist")
            || id.is_empty()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return None;
        }

        Some(ProviderIdentifier {
            provider,
            content_type: content_type.to_owned(),
            id: id.to_owned(),
        })
    }

    async fn resolve_metadata(
        &self,
        identifier: &str,
        parsed: &ProviderIdentifier,
    ) -> Result<ExternalTrack, PluginError> {
        match parsed.provider {
            MusicProvider::Spotify => self.spotify_metadata(identifier).await,
            MusicProvider::AppleMusic => self.apple_music_metadata(&parsed.id).await,
            MusicProvider::Deezer => self.deezer_metadata(&parsed.id).await,
        }
    }

    async fn spotify_metadata(&self, identifier: &str) -> Result<ExternalTrack, PluginError> {
        let response = self
            .client
            .get("https://open.spotify.com/oembed")
            .query(&[("url", identifier)])
            .send()
            .await
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Spotify request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Spotify returned an error: {error}"))
            })?;
        let data: Value = response.json().await.map_err(|error| {
            PluginError::new("lavasrc", format!("invalid Spotify response: {error}"))
        })?;
        Ok(ExternalTrack {
            title: data
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Unknown title")
                .to_owned(),
            author: data
                .get("author_name")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_owned(),
            duration_ms: None,
            artwork_url: data
                .get("thumbnail_url")
                .and_then(Value::as_str)
                .map(str::to_owned),
            isrc: None,
        })
    }

    async fn apple_music_metadata(&self, track_id: &str) -> Result<ExternalTrack, PluginError> {
        let response = self
            .client
            .get("https://itunes.apple.com/lookup")
            .query(&[("id", track_id)])
            .send()
            .await
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Apple Music request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Apple Music returned an error: {error}"))
            })?;
        let data: Value = response.json().await.map_err(|error| {
            PluginError::new("lavasrc", format!("invalid Apple Music response: {error}"))
        })?;
        let item = data
            .get("results")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("wrapperType").and_then(Value::as_str) == Some("track"))
            })
            .ok_or_else(|| PluginError::new("lavasrc", "Apple Music did not return a track"))?;
        Ok(ExternalTrack {
            title: item
                .get("trackName")
                .and_then(Value::as_str)
                .unwrap_or("Unknown title")
                .to_owned(),
            author: item
                .get("artistName")
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_owned(),
            duration_ms: item.get("trackTimeMillis").and_then(Value::as_u64),
            artwork_url: item
                .get("artworkUrl100")
                .and_then(Value::as_str)
                .map(str::to_owned),
            isrc: item.get("isrc").and_then(Value::as_str).map(str::to_owned),
        })
    }

    async fn deezer_metadata(&self, track_id: &str) -> Result<ExternalTrack, PluginError> {
        let response = self
            .client
            .get(format!("https://api.deezer.com/track/{track_id}"))
            .send()
            .await
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Deezer request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Deezer returned an error: {error}"))
            })?;
        let data: Value = response.json().await.map_err(|error| {
            PluginError::new("lavasrc", format!("invalid Deezer response: {error}"))
        })?;
        if data.get("error").is_some() {
            return Err(PluginError::new(
                "lavasrc",
                "Deezer could not resolve this track",
            ));
        }
        let duration_ms = data
            .get("duration")
            .and_then(Value::as_u64)
            .map(|seconds| seconds.saturating_mul(1_000));
        Ok(ExternalTrack {
            title: data
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Unknown title")
                .to_owned(),
            author: data
                .get("artist")
                .and_then(|artist| artist.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("Unknown")
                .to_owned(),
            duration_ms,
            artwork_url: data
                .get("album")
                .and_then(|album| album.get("cover_xl"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            isrc: data.get("isrc").and_then(Value::as_str).map(str::to_owned),
        })
    }

    async fn match_external_track(
        &self,
        identifier: &str,
        parsed: &ProviderIdentifier,
        metadata: ExternalTrack,
    ) -> Result<Track, PluginError> {
        let query = format!("{} {}", metadata.title, metadata.author);
        let candidates = self.youtube.search_tracks(&query).await?;
        let candidate = candidates
            .first()
            .ok_or_else(|| PluginError::new("lavasrc", "no matching YouTube track was found"))?;
        let mut matched = self
            .youtube
            .resolve_video(&candidate.info.identifier)
            .await?;
        matched.info.identifier = parsed.id.clone();
        matched.info.title = metadata.title;
        matched.info.author = metadata.author;
        matched.info.source_name = parsed.provider.source_name().to_owned();
        matched.info.uri = Some(identifier.to_owned());
        if let Some(duration) = metadata.duration_ms {
            matched.info.length = i64::try_from(duration).unwrap_or(i64::MAX);
        }
        if metadata.artwork_url.is_some() {
            matched.info.artwork_url = metadata.artwork_url;
        }
        matched.info.isrc = metadata.isrc;
        matched.encoded =
            encode_track(&matched).map_err(|error| PluginError::new("lavasrc", error.message))?;
        matched.plugin_info["matchedSource"] = json!("youtube");
        Ok(matched)
    }

    async fn resolve_track(
        &self,
        identifier: &str,
        parsed: &ProviderIdentifier,
    ) -> Result<Track, PluginError> {
        let metadata = self.resolve_metadata(identifier, parsed).await?;
        self.match_external_track(identifier, parsed, metadata)
            .await
    }

    async fn resolve_deezer_collection(
        &self,
        parsed: &ProviderIdentifier,
    ) -> Result<PluginLoadResult, PluginError> {
        let endpoint = match parsed.content_type.as_str() {
            "album" => format!("https://api.deezer.com/album/{}", parsed.id),
            "playlist" => format!("https://api.deezer.com/playlist/{}", parsed.id),
            "artist" => format!("https://api.deezer.com/artist/{}/top?limit=15", parsed.id),
            _ => {
                return Err(PluginError::new(
                    "lavasrc",
                    "unsupported Deezer collection type",
                ));
            }
        };
        let data: Value = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasrc",
                    format!("Deezer collection request failed: {error}"),
                )
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Deezer returned an error: {error}"))
            })?
            .json()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasrc",
                    format!("invalid Deezer collection response: {error}"),
                )
            })?;
        if data.get("error").is_some() {
            return Err(PluginError::new(
                "lavasrc",
                "Deezer could not resolve this collection",
            ));
        }
        let items = if parsed.content_type == "artist" {
            data.get("data").and_then(Value::as_array)
        } else {
            data.get("tracks")
                .and_then(|tracks| tracks.get("data"))
                .and_then(Value::as_array)
        }
        .ok_or_else(|| PluginError::new("lavasrc", "Deezer collection contains no track list"))?;
        let name = data
            .get("title")
            .or_else(|| data.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Deezer collection")
            .to_owned();

        let mut tracks = Vec::new();
        for item in items.iter().take(15) {
            let id = item
                .get("id")
                .and_then(Value::as_u64)
                .map(|value| value.to_string())
                .or_else(|| item.get("id").and_then(Value::as_str).map(str::to_owned));
            let Some(id) = id else {
                continue;
            };
            let metadata = ExternalTrack {
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown title")
                    .to_owned(),
                author: item
                    .get("artist")
                    .and_then(|artist| artist.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown")
                    .to_owned(),
                duration_ms: item
                    .get("duration")
                    .and_then(Value::as_u64)
                    .map(|seconds| seconds.saturating_mul(1_000)),
                artwork_url: item
                    .get("album")
                    .and_then(|album| album.get("cover_xl"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                isrc: item.get("isrc").and_then(Value::as_str).map(str::to_owned),
            };
            let track_identifier = ProviderIdentifier {
                provider: MusicProvider::Deezer,
                content_type: "track".to_owned(),
                id: id.clone(),
            };
            let uri = format!("https://www.deezer.com/track/{id}");
            if let Ok(track) = self
                .match_external_track(&uri, &track_identifier, metadata)
                .await
            {
                tracks.push(track);
            }
        }

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
impl RavaLinkPlugin for LavaSrcPlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "lavasrc",
            version: env!("CARGO_PKG_VERSION"),
            description: "Spotify, Apple Music and Deezer source integration",
            capabilities: &[
                "spotify",
                "apple-music",
                "deezer",
                "track",
                "album",
                "artist",
                "playlist",
            ],
        }
    }

    fn supports_identifier(&self, identifier: &str) -> bool {
        Self::parse_identifier(identifier).is_some()
    }

    async fn load_tracks(&self, identifier: &str) -> Result<Option<PluginLoadResult>, PluginError> {
        let Some(parsed) = Self::parse_identifier(identifier) else {
            return Ok(None);
        };
        if parsed.content_type == "track" {
            let track = self.resolve_track(identifier, &parsed).await?;
            return Ok(Some(PluginLoadResult::new(
                PluginLoadType::Track,
                serde_json::to_value(track).expect("track serializes"),
            )));
        }
        if parsed.provider == MusicProvider::Deezer {
            return self.resolve_deezer_collection(&parsed).await.map(Some);
        }
        Err(PluginError::new(
            "lavasrc",
            format!(
                "{} collection resolution is not available for this provider yet",
                parsed.content_type
            ),
        ))
    }
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

fn split_url(value: &str) -> Option<ParsedUrl<'_>> {
    let (_, rest) = value.trim().split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end]
        .split('@')
        .next_back()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let suffix = &rest[end..];
    let path_end = suffix.find(['?', '#']).unwrap_or(suffix.len());
    let query = suffix
        .find('?')
        .map(|index| suffix[index + 1..].split('#').next().unwrap_or(""))
        .unwrap_or("");
    Some(ParsedUrl {
        host,
        path: &suffix[..path_end],
        query,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spotify_track_album_and_playlist_urls() {
        let track = LavaSrcPlugin::parse_identifier(
            "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=abc",
        )
        .unwrap();
        assert_eq!(track.provider, MusicProvider::Spotify);
        assert_eq!(track.content_type, "track");
        assert_eq!(track.id, "4uLU6hMCjMI75M1A2tKUQC");

        let international =
            LavaSrcPlugin::parse_identifier("https://open.spotify.com/intl-fr/album/abc123")
                .unwrap();
        assert_eq!(international.content_type, "album");
        assert_eq!(international.id, "abc123");

        let playlist =
            LavaSrcPlugin::parse_identifier("https://open.spotify.com/playlist/abc123").unwrap();
        assert_eq!(playlist.content_type, "playlist");
    }

    #[test]
    fn parses_apple_music_and_deezer_links() {
        let apple = LavaSrcPlugin::parse_identifier(
            "https://music.apple.com/fr/album/test/123456789?i=987654321",
        )
        .unwrap();
        assert_eq!(apple.provider, MusicProvider::AppleMusic);
        assert_eq!(apple.content_type, "track");
        assert_eq!(apple.id, "987654321");

        let deezer =
            LavaSrcPlugin::parse_identifier("https://www.deezer.com/track/123456").unwrap();
        assert_eq!(deezer.provider, MusicProvider::Deezer);
        assert_eq!(deezer.id, "123456");
    }

    #[test]
    fn rejects_unknown_hosts_and_content_types() {
        assert!(LavaSrcPlugin::parse_identifier("https://example.com/track/123").is_none());
        assert!(LavaSrcPlugin::parse_identifier("https://open.spotify.com/show/123").is_none());
    }
}
