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
use std::{
    collections::HashSet,
    env,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_PROVIDER_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

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
    spotify_token: Arc<Mutex<Option<(String, Instant)>>>,
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
                .user_agent(format!(
                    "RavaLink/{} (native LavaSrc adapter)",
                    env!("CARGO_PKG_VERSION")
                ))
                .build()
                .expect("valid default HTTP client"),
            spotify_token: Arc::new(Mutex::new(None)),
        }
    }
}

fn append_bounded_response_chunk(
    body: &mut Vec<u8>,
    chunk: &[u8],
    provider: &str,
) -> Result<(), PluginError> {
    if body.len().saturating_add(chunk.len()) > MAX_PROVIDER_RESPONSE_BYTES {
        return Err(PluginError::new(
            "lavasrc",
            format!("{provider} response exceeds the 8 MiB limit"),
        ));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

async fn read_json_response(
    mut response: reqwest::Response,
    provider: &str,
) -> Result<Value, PluginError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PROVIDER_RESPONSE_BYTES as u64)
    {
        return Err(PluginError::new(
            "lavasrc",
            format!("{provider} response exceeds the 8 MiB limit"),
        ));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        PluginError::new(
            "lavasrc",
            format!("{provider} response read failed: {error}"),
        )
    })? {
        append_bounded_response_chunk(&mut body, &chunk, provider)?;
    }

    serde_json::from_slice(&body).map_err(|error| {
        PluginError::new("lavasrc", format!("invalid {provider} response: {error}"))
    })
}

impl LavaSrcPlugin {
    pub fn parse_identifier(identifier: &str) -> Option<ProviderIdentifier> {
        if let Some(spotify_uri) = identifier.strip_prefix("spotify:") {
            let (content_type, id) = spotify_uri.split_once(':')?;
            if !matches!(content_type, "track" | "album" | "playlist" | "artist")
                || id.is_empty()
                || !id.bytes().all(|byte| byte.is_ascii_alphanumeric())
            {
                return None;
            }
            return Some(ProviderIdentifier {
                provider: MusicProvider::Spotify,
                content_type: content_type.to_owned(),
                id: id.to_owned(),
            });
        }

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

        let valid_id = match provider {
            MusicProvider::AppleMusic => id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
            MusicProvider::Spotify | MusicProvider::Deezer => id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        };
        if !matches!(content_type, "track" | "album" | "playlist" | "artist")
            || id.is_empty()
            || !valid_id
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
        parsed: &ProviderIdentifier,
    ) -> Result<ExternalTrack, PluginError> {
        match parsed.provider {
            MusicProvider::Spotify => self.spotify_metadata(&parsed.id).await,
            MusicProvider::AppleMusic => self.apple_music_metadata(&parsed.id).await,
            MusicProvider::Deezer => self.deezer_metadata(&parsed.id).await,
        }
    }

    async fn spotify_metadata(&self, track_id: &str) -> Result<ExternalTrack, PluginError> {
        if let Ok(token) = self.spotify_access_token().await {
            let response = self
                .client
                .get(format!("https://api.spotify.com/v1/tracks/{track_id}"))
                .bearer_auth(token)
                .send()
                .await;
            if let Ok(response) = response
                && let Ok(response) = response.error_for_status()
                && let Ok(data) = read_json_response(response, "Spotify").await
            {
                let artists = data
                    .get("artists")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|artist| artist.get("name").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ");
                let artwork_url = data
                    .pointer("/album/images")
                    .and_then(Value::as_array)
                    .and_then(|images| images.first())
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                return Ok(ExternalTrack {
                    title: data
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown title")
                        .to_owned(),
                    author: if artists.is_empty() {
                        "Unknown".into()
                    } else {
                        artists
                    },
                    duration_ms: data.get("duration_ms").and_then(Value::as_u64),
                    artwork_url,
                    isrc: data
                        .pointer("/external_ids/isrc")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
        }

        // Public oEmbed remains a no-credential fallback for track metadata.
        let canonical_url = format!("https://open.spotify.com/track/{track_id}");
        let response = self
            .client
            .get("https://open.spotify.com/oembed")
            .query(&[("url", canonical_url)])
            .send()
            .await
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Spotify request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Spotify returned an error: {error}"))
            })?;
        let data = read_json_response(response, "Spotify").await?;
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
        let data = read_json_response(response, "Apple Music").await?;
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
        let data = read_json_response(response, "Deezer").await?;
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
            .iter()
            .filter_map(|candidate| {
                let title_similarity = text_similarity(&metadata.title, &candidate.info.title);
                (title_similarity >= 0.25)
                    .then_some((candidate, music_match_score(&metadata, candidate)))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(candidate, _)| candidate)
            .ok_or_else(|| {
                PluginError::new("lavasrc", "no sufficiently close YouTube match was found")
            })?;
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
        let metadata = self.resolve_metadata(parsed).await?;
        self.match_external_track(identifier, parsed, metadata)
            .await
    }

    async fn spotify_access_token(&self) -> Result<String, PluginError> {
        if let Ok(cache) = self.spotify_token.lock()
            && let Some((token, expires_at)) = cache.as_ref()
            && *expires_at > Instant::now()
        {
            return Ok(token.clone());
        }

        let client_id = env::var("RAVALINK_SPOTIFY_CLIENT_ID").map_err(|_| {
            PluginError::new(
                "lavasrc",
                "Spotify collection playback requires RAVALINK_SPOTIFY_CLIENT_ID and RAVALINK_SPOTIFY_CLIENT_SECRET",
            )
        })?;
        let client_secret = env::var("RAVALINK_SPOTIFY_CLIENT_SECRET").map_err(|_| {
            PluginError::new(
                "lavasrc",
                "Spotify collection playback requires RAVALINK_SPOTIFY_CLIENT_ID and RAVALINK_SPOTIFY_CLIENT_SECRET",
            )
        })?;
        let response = self
            .client
            .post("https://accounts.spotify.com/api/token")
            .basic_auth(client_id, Some(client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Spotify token request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Spotify authentication failed: {error}"))
            })?;
        let response = read_json_response(response, "Spotify").await?;
        let token = response
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| PluginError::new("lavasrc", "Spotify returned no access token"))?
            .to_owned();
        let expires_in = response
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3_600);
        if let Ok(mut cache) = self.spotify_token.lock() {
            *cache = Some((
                token.clone(),
                Instant::now() + Duration::from_secs(expires_in.saturating_sub(60)),
            ));
        }
        Ok(token)
    }

    async fn spotify_collection(
        &self,
        parsed: &ProviderIdentifier,
    ) -> Result<PluginLoadResult, PluginError> {
        let token = self.spotify_access_token().await?;
        let (endpoint, tracks_path, fallback_name) = match parsed.content_type.as_str() {
            "album" => (
                format!("https://api.spotify.com/v1/albums/{}?market=US", parsed.id),
                "/tracks/items",
                "Spotify album",
            ),
            "playlist" => (
                format!(
                    "https://api.spotify.com/v1/playlists/{}?market=US",
                    parsed.id
                ),
                "/tracks/items",
                "Spotify playlist",
            ),
            "artist" => (
                format!(
                    "https://api.spotify.com/v1/artists/{}/top-tracks?market=US",
                    parsed.id
                ),
                "/tracks",
                "Spotify artist top tracks",
            ),
            _ => {
                return Err(PluginError::new(
                    "lavasrc",
                    "unsupported Spotify collection",
                ));
            }
        };
        let response = self
            .client
            .get(endpoint)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasrc",
                    format!("Spotify collection request failed: {error}"),
                )
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Spotify returned an error: {error}"))
            })?;
        let data = read_json_response(response, "Spotify").await?;
        let items = data
            .pointer(tracks_path)
            .and_then(Value::as_array)
            .ok_or_else(|| PluginError::new("lavasrc", "Spotify collection contains no tracks"))?;
        let name = data
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(fallback_name)
            .to_owned();
        let mut tracks = Vec::new();
        for item in items.iter().take(15) {
            let item = item.get("track").unwrap_or(item);
            if item.is_null() {
                continue;
            }
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            let metadata = ExternalTrack {
                title: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown title")
                    .to_owned(),
                author: item
                    .get("artists")
                    .and_then(Value::as_array)
                    .and_then(|artists| artists.first())
                    .and_then(|artist| artist.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown")
                    .to_owned(),
                duration_ms: item.get("duration_ms").and_then(Value::as_u64),
                artwork_url: item
                    .get("album")
                    .and_then(|album| album.get("images"))
                    .and_then(Value::as_array)
                    .and_then(|images| images.first())
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                isrc: item
                    .get("external_ids")
                    .and_then(|ids| ids.get("isrc"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            let track_identifier = ProviderIdentifier {
                provider: MusicProvider::Spotify,
                content_type: "track".to_owned(),
                id: id.to_owned(),
            };
            let uri = item
                .pointer("/external_urls/spotify")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("https://open.spotify.com/track/{id}"));
            if let Ok(track) = self
                .match_external_track(&uri, &track_identifier, metadata)
                .await
            {
                tracks.push(track);
            }
        }
        Ok(PluginLoadResult::new(
            PluginLoadType::Playlist,
            json!({"info": {"name": name, "selectedTrack": -1}, "tracks": tracks}),
        ))
    }

    async fn apple_music_playlist(
        &self,
        parsed: &ProviderIdentifier,
    ) -> Result<PluginLoadResult, PluginError> {
        let token = env::var("RAVALINK_APPLE_MUSIC_TOKEN").map_err(|_| {
            PluginError::new(
                "lavasrc",
                "Apple Music playlist resolution requires RAVALINK_APPLE_MUSIC_TOKEN",
            )
        })?;
        let response = self
            .client
            .get(format!(
                "https://api.music.apple.com/v1/catalog/us/playlists/{}",
                parsed.id
            ))
            .bearer_auth(token)
            .query(&[("include", "tracks")])
            .send()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasrc",
                    format!("Apple Music playlist request failed: {error}"),
                )
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Apple Music returned an error: {error}"))
            })?;
        let data = read_json_response(response, "Apple Music").await?;
        let playlist = data
            .pointer("/data/0")
            .ok_or_else(|| PluginError::new("lavasrc", "Apple Music playlist was not found"))?;
        let name = playlist
            .pointer("/attributes/name")
            .and_then(Value::as_str)
            .unwrap_or("Apple Music playlist")
            .to_owned();
        let items = playlist
            .pointer("/relationships/tracks/data")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                PluginError::new("lavasrc", "Apple Music playlist contains no track list")
            })?;
        let mut tracks = Vec::new();
        for item in items.iter().take(50) {
            let Some(attributes) = item.get("attributes") else {
                continue;
            };
            if item.get("type").and_then(Value::as_str) != Some("songs") {
                continue;
            }
            let id = attributes
                .pointer("/playParams/id")
                .and_then(Value::as_str)
                .or_else(|| item.get("id").and_then(Value::as_str));
            let Some(id) = id else {
                continue;
            };
            let artwork_url = attributes
                .pointer("/artwork/url")
                .and_then(Value::as_str)
                .map(|url| url.replace("{w}", "1000").replace("{h}", "1000"));
            let metadata = ExternalTrack {
                title: attributes
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown title")
                    .to_owned(),
                author: attributes
                    .get("artistName")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown")
                    .to_owned(),
                duration_ms: attributes.get("durationInMillis").and_then(Value::as_u64),
                artwork_url,
                isrc: attributes
                    .get("isrc")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            let track_identifier = ProviderIdentifier {
                provider: MusicProvider::AppleMusic,
                content_type: "track".to_owned(),
                id: id.to_owned(),
            };
            let uri = attributes
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("https://music.apple.com/us/song/{id}"));
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

    async fn apple_music_collection(
        &self,
        parsed: &ProviderIdentifier,
    ) -> Result<PluginLoadResult, PluginError> {
        if parsed.content_type == "playlist" {
            return self.apple_music_playlist(parsed).await;
        }
        let response = self
            .client
            .get("https://itunes.apple.com/lookup")
            .query(&[
                ("id", parsed.id.as_str()),
                ("entity", "song"),
                ("limit", "25"),
            ])
            .send()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasrc",
                    format!("Apple Music collection request failed: {error}"),
                )
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasrc", format!("Apple Music returned an error: {error}"))
            })?;
        let data = read_json_response(response, "Apple Music").await?;
        let items = data
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                PluginError::new("lavasrc", "Apple Music collection contains no results")
            })?;
        let name = items
            .iter()
            .find_map(|item| {
                item.get("collectionName")
                    .or_else(|| item.get("artistName"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("Apple Music collection")
            .to_owned();
        let mut tracks = Vec::new();
        for item in items
            .iter()
            .filter(|item| item.get("wrapperType").and_then(Value::as_str) == Some("track"))
            .take(15)
        {
            let Some(id) = item.get("trackId").and_then(Value::as_u64) else {
                continue;
            };
            let metadata = ExternalTrack {
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
            };
            let track_identifier = ProviderIdentifier {
                provider: MusicProvider::AppleMusic,
                content_type: "track".to_owned(),
                id: id.to_string(),
            };
            let uri = item
                .get("trackViewUrl")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("https://music.apple.com/us/song/{id}"));
            if let Ok(track) = self
                .match_external_track(&uri, &track_identifier, metadata)
                .await
            {
                tracks.push(track);
            }
        }
        Ok(PluginLoadResult::new(
            PluginLoadType::Playlist,
            json!({"info": {"name": name, "selectedTrack": -1}, "tracks": tracks}),
        ))
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
        let response = self
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
            })?;
        let data = read_json_response(response, "Deezer").await?;
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

fn text_similarity(left: &str, right: &str) -> f32 {
    let tokens = |value: &str| {
        value
            .chars()
            .map(|character| {
                if character.is_alphanumeric() {
                    character.to_lowercase().collect::<String>()
                } else {
                    " ".to_owned()
                }
            })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_owned)
            .collect::<HashSet<_>>()
    };
    let left = tokens(left);
    let right = tokens(right);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(&right).count() as f32;
    let union = left.union(&right).count() as f32;
    intersection / union
}

fn music_match_score(metadata: &ExternalTrack, candidate: &Track) -> f32 {
    let title = text_similarity(&metadata.title, &candidate.info.title);
    let artist = text_similarity(&metadata.author, &candidate.info.author);
    let duration = match (
        metadata.duration_ms,
        u64::try_from(candidate.info.length).ok(),
    ) {
        (Some(expected), Some(actual)) if expected > 0 && actual > 0 => {
            let difference = expected.abs_diff(actual) as f32;
            (1.0 - difference / expected.max(actual) as f32).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    title * 5.0 + artist * 3.0 + duration * 2.0
}

#[async_trait]
impl RavaLinkPlugin for LavaSrcPlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "lavasrc".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Spotify, Apple Music and Deezer source integration".into(),
            capabilities: vec![
                "spotify".into(),
                "apple-music".into(),
                "deezer".into(),
                "track".into(),
                "album".into(),
                "artist".into(),
                "playlist".into(),
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
        match parsed.provider {
            MusicProvider::Deezer => self.resolve_deezer_collection(&parsed).await.map(Some),
            MusicProvider::Spotify => self.spotify_collection(&parsed).await.map(Some),
            MusicProvider::AppleMusic => self.apple_music_collection(&parsed).await.map(Some),
        }
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

    async fn mock_youtube_api(
        search_result: serde_json::Value,
        video_result: serde_json::Value,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for body in [search_result.to_string(), video_result.to_string()] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 4_096];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn matches_external_catalog_track_to_playable_youtube_audio() {
        let search_result = json!([{
            "videoId": "dQw4w9WgXcQ",
            "title": "Example Track",
            "author": "Example Artist",
            "lengthSeconds": 200
        }]);
        let video_result = json!({
            "videoId": "dQw4w9WgXcQ",
            "title": "Example Track",
            "author": "Example Artist",
            "lengthSeconds": 200,
            "adaptiveFormats": [{
                "type": "audio/webm; codecs=opus",
                "bitrate": 160000,
                "url": "https://media.example/audio.webm"
            }]
        });
        let api_base = mock_youtube_api(search_result, video_result).await;
        let plugin = LavaSrcPlugin {
            youtube: YouTubePlugin::new(api_base),
            client: reqwest::Client::new(),
            spotify_token: Arc::new(Mutex::new(None)),
        };
        let parsed = ProviderIdentifier {
            provider: MusicProvider::Spotify,
            content_type: "track".into(),
            id: "spotify-track-id".into(),
        };
        let identifier = "https://open.spotify.com/track/spotify-track-id";

        let matched = plugin
            .match_external_track(
                identifier,
                &parsed,
                ExternalTrack {
                    title: "Example Track".into(),
                    author: "Example Artist".into(),
                    duration_ms: Some(200_000),
                    artwork_url: Some("https://images.example/cover.jpg".into()),
                    isrc: Some("USRC12345678".into()),
                },
            )
            .await
            .unwrap();

        assert_eq!(matched.info.identifier, "spotify-track-id");
        assert_eq!(matched.info.title, "Example Track");
        assert_eq!(matched.info.author, "Example Artist");
        assert_eq!(matched.info.source_name, "spotify");
        assert_eq!(matched.info.uri.as_deref(), Some(identifier));
        assert_eq!(matched.info.length, 200_000);
        assert_eq!(
            matched.info.artwork_url.as_deref(),
            Some("https://images.example/cover.jpg")
        );
        assert_eq!(matched.info.isrc.as_deref(), Some("USRC12345678"));
        assert_eq!(
            matched.plugin_info["streamUrl"],
            "https://media.example/audio.webm"
        );
        assert_eq!(matched.plugin_info["matchedSource"], "youtube");
        assert!(!matched.encoded.is_empty());
    }

    #[tokio::test]
    async fn rejects_external_track_when_youtube_search_has_no_matches() {
        let api_base = mock_youtube_api(json!([]), json!({})).await;
        let plugin = LavaSrcPlugin {
            youtube: YouTubePlugin::new(api_base),
            client: reqwest::Client::new(),
            spotify_token: Arc::new(Mutex::new(None)),
        };
        let parsed = ProviderIdentifier {
            provider: MusicProvider::Deezer,
            content_type: "track".into(),
            id: "123456789".into(),
        };

        let error = plugin
            .match_external_track(
                "https://www.deezer.com/track/123456789",
                &parsed,
                ExternalTrack {
                    title: "Missing Track".into(),
                    author: "Missing Artist".into(),
                    duration_ms: Some(180_000),
                    artwork_url: None,
                    isrc: None,
                },
            )
            .await
            .unwrap_err();

        assert!(
            error
                .message
                .contains("no sufficiently close YouTube match")
        );
    }

    #[test]
    fn provider_response_limit_accepts_exact_limit_and_rejects_overflow() {
        let mut body = vec![0; MAX_PROVIDER_RESPONSE_BYTES - 1];
        append_bounded_response_chunk(&mut body, &[1], "test").unwrap();
        assert_eq!(body.len(), MAX_PROVIDER_RESPONSE_BYTES);
        assert!(append_bounded_response_chunk(&mut body, &[2], "test").is_err());
        assert_eq!(body.len(), MAX_PROVIDER_RESPONSE_BYTES);
    }

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
    fn parses_spotify_uris_as_well_as_web_urls() {
        let parsed =
            LavaSrcPlugin::parse_identifier("spotify:track:4uLU6hMCjMI75M1A2tKUQC").unwrap();
        assert_eq!(parsed.provider, MusicProvider::Spotify);
        assert_eq!(parsed.content_type, "track");
        assert_eq!(parsed.id, "4uLU6hMCjMI75M1A2tKUQC");
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

        let apple_playlist = LavaSrcPlugin::parse_identifier(
            "https://music.apple.com/us/playlist/road-trip/pl.u-123456789",
        )
        .unwrap();
        assert_eq!(apple_playlist.provider, MusicProvider::AppleMusic);
        assert_eq!(apple_playlist.content_type, "playlist");
        assert_eq!(apple_playlist.id, "pl.u-123456789");

        let deezer =
            LavaSrcPlugin::parse_identifier("https://www.deezer.com/track/123456").unwrap();
        assert_eq!(deezer.provider, MusicProvider::Deezer);
        assert_eq!(deezer.id, "123456");
    }

    #[test]
    fn music_match_scoring_prefers_title_and_artist_agreement() {
        let metadata = ExternalTrack {
            title: "Midnight City".into(),
            author: "M83".into(),
            duration_ms: Some(240_000),
            artwork_url: None,
            isrc: None,
        };
        let exact = Track {
            encoded: String::new(),
            info: ravalink_protocol::TrackInfo {
                identifier: "exact".into(),
                is_seekable: true,
                author: "M83".into(),
                length: 240_000,
                is_stream: false,
                position: 0,
                title: "Midnight City".into(),
                uri: None,
                source_name: "youtube".into(),
                artwork_url: None,
                isrc: None,
            },
            plugin_info: json!({}),
            user_data: json!({}),
        };
        let wrong_artist = Track {
            info: ravalink_protocol::TrackInfo {
                author: "Other Artist".into(),
                ..exact.info.clone()
            },
            ..exact.clone()
        };
        assert!(music_match_score(&metadata, &exact) > music_match_score(&metadata, &wrong_artist));
        assert!(text_similarity("Midnight City", "Midnight City (Official Video)") > 0.2);
    }

    #[test]
    fn rejects_unknown_hosts_and_content_types() {
        assert!(LavaSrcPlugin::parse_identifier("https://example.com/track/123").is_none());
        assert!(LavaSrcPlugin::parse_identifier("https://open.spotify.com/show/123").is_none());
    }
}
