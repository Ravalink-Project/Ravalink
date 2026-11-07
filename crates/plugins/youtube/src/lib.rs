//! Native YouTube source plugin.
//!
//! Extraction is performed in-process through YouTube InnerTube. No public
//! Invidious/Piped instance, external downloader process, Node.js or Python
//! runtime is required. YouTube itself remains the upstream media provider.

use async_trait::async_trait;
use futures_util::StreamExt;
use ravalink_plugin_api::{
    PluginDescriptor, PluginError, PluginLoadResult, PluginLoadType, RavaLinkPlugin,
};
use ravalink_protocol::{Track, TrackInfo, encode_track};
use reqwest::header::{ORIGIN, REFERER, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use ytdown::{MediaInfo, Ytdown, cookies::CookieJar};

const YOUTUBE_OAUTH_CLIENT_ID: &str =
    "861556708454-d6dlm3lh05idd8npek18k6be8ba3oc68.apps.googleusercontent.com";
const YOUTUBE_OAUTH_CLIENT_SECRET: &str = "SboVhoG9s0rNafixCSGGKXAT";
const YOUTUBE_TV_USER_AGENT: &str = "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version";
const YOUTUBE_TV_REFERER: &str = "https://www.youtube.com/tv";
const YOUTUBE_OAUTH_DEVICE_URL: &str = "https://www.youtube.com/o/oauth2/device/code";
const YOUTUBE_OAUTH_TOKEN_URL: &str = "https://www.youtube.com/o/oauth2/token";
const YOUTUBE_OAUTH_SCOPE: &str =
    "http://gdata.youtube.com https://www.googleapis.com/auth/youtube";
const DEFAULT_OAUTH_TOKEN_FILE: &str = "data/youtube-oauth.json";
const MAX_SEARCH_RESULTS: usize = 50;
const MAX_PLAYLIST_TRACKS: usize = 500;
const MAX_SEARCH_QUERY_BYTES: usize = 512;

#[derive(Clone)]
pub struct YouTubePlugin {
    /// Unauthenticated extractor for search, playlists and ordinary playback.
    /// OAuth is never attached to these requests.
    extractor: Arc<Ytdown>,
    /// OAuth-enabled extractor used only as a playback fallback. Its bearer
    /// token is isolated from public search and playlist requests.
    oauth_extractor: Arc<RwLock<Option<Arc<Ytdown>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeIdentifier {
    pub video_id: Option<String>,
    pub playlist_id: Option<String>,
    pub start_seconds: Option<u64>,
}

impl YouTubePlugin {
    /// Build the native YouTube source. OAuth follows Lavalink's YouTube TV
    /// device flow and is isolated to a dedicated playback-fallback extractor.
    pub async fn new(
        cookies_file: Option<&Path>,
        oauth_enabled: bool,
        oauth_refresh_token: Option<&str>,
        oauth_token_file: Option<&Path>,
        skip_oauth_initialization: bool,
    ) -> Result<Self, PluginError> {
        let cookies_file = cookies_file.map(Path::to_path_buf);
        let extractor = Arc::new(build_extractor(cookies_file.as_deref(), None)?);
        let oauth_extractor = Arc::new(RwLock::new(None));

        if oauth_enabled {
            let token_file = oauth_token_file
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_OAUTH_TOKEN_FILE));
            let token = load_stored_token(oauth_refresh_token, &token_file).await?;
            if let Some(token) = token {
                install_oauth_extractor(
                    oauth_extractor.clone(),
                    cookies_file.clone(),
                    token_file.clone(),
                    token,
                )
                .await?;
            } else if skip_oauth_initialization {
                tracing::warn!(
                    "YouTube OAuth initialization skipped; no refresh token is available"
                );
            } else {
                // Match Lavalink's behavior: start the device flow in the
                // background so the node can finish starting without waiting
                // for an interactive Google sign-in.
                let slot = oauth_extractor.clone();
                let cookies = cookies_file.clone();
                tokio::spawn(async move {
                    match authorize_device().await {
                        Ok(token) => {
                            if let Err(error) = save_token(&token_file, &token) {
                                tracing::error!(%error, "failed to persist initial YouTube OAuth token");
                                return;
                            }
                            match build_extractor(cookies.as_deref(), Some(&token)) {
                                Ok(extractor) => {
                                    *slot.write().await = Some(Arc::new(extractor));
                                    spawn_oauth_refresh_loop(
                                        slot.clone(),
                                        cookies,
                                        token_file,
                                        token,
                                    );
                                    tracing::info!("YouTube TV OAuth authorization completed");
                                }
                                Err(error) => {
                                    tracing::error!(message = %error.message, "failed to build OAuth YouTube extractor after authorization");
                                }
                            }
                        }
                        Err(error) => {
                            tracing::error!(message = %error.message, "YouTube TV OAuth device flow failed");
                        }
                    }
                });
            }
        }

        Ok(Self {
            extractor,
            oauth_extractor,
        })
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

        let playlist_id = url
            .query_value("list")
            .filter(|id| valid_playlist_id(id))
            .map(str::to_owned);
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
            .filter(|query| !query.is_empty() && query.len() <= MAX_SEARCH_QUERY_BYTES)
    }

    pub async fn resolve_video(&self, video_id: &str) -> Result<Track, PluginError> {
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        // When OAuth is enabled and a token is available, use it for video
        // resolution as Lavalink does. Fall back to anonymous extraction if the
        // authenticated request fails. Search and playlist calls stay public.
        let authenticated = self.oauth_extractor.read().await.clone();
        if let Some(authenticated) = authenticated {
            match resolve_video_with_extractor(&authenticated, &url).await {
                Ok(track) => return Ok(track),
                Err(oauth_error) => {
                    return resolve_video_with_extractor(&self.extractor, &url)
                        .await
                        .map_err(|anonymous_error| {
                            PluginError::new(
                                "youtube",
                                format!(
                                    "TV OAuth extraction failed ({oauth_error}); anonymous fallback failed ({anonymous_error})"
                                ),
                            )
                        });
                }
            }
        }

        resolve_video_with_extractor(&self.extractor, &url).await
    }

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<Track>, PluginError> {
        let result = self
            .extractor
            .resolve(&format!("ytsearch:{query}"))
            .await
            .map_err(|error| PluginError::new("youtube", error.to_string()))?;
        let MediaInfo::Collection(mut collection) = result else {
            return Err(PluginError::new(
                "youtube",
                "YouTube search did not return a result collection",
            ));
        };

        let mut tracks = Vec::with_capacity(MAX_SEARCH_RESULTS);
        let mut entries_seen = 0usize;
        let mut entries_rejected = 0usize;
        while tracks.len() < MAX_SEARCH_RESULTS {
            let Some(entry) = collection.entries.next().await else {
                break;
            };
            let entry = entry.map_err(|error| PluginError::new("youtube", error.to_string()))?;
            entries_seen += 1;
            match track_from_entry(&entry) {
                Ok(track) => tracks.push(track),
                Err(error) => {
                    entries_rejected += 1;
                    tracing::debug!(
                        video_id = %entry.id,
                        error = %error.message,
                        "skipping unsupported YouTube search entry"
                    );
                }
            }
        }
        tracing::debug!(
            query,
            entries_seen,
            entries_rejected,
            tracks_returned = tracks.len(),
            "YouTube search extraction completed"
        );
        Ok(tracks)
    }

    async fn search(&self, query: &str) -> Result<PluginLoadResult, PluginError> {
        let tracks = self.search_tracks(query).await?;
        Ok(PluginLoadResult::new(
            PluginLoadType::Search,
            serde_json::to_value(tracks).expect("track list serializes"),
        ))
    }

    async fn load_playlist(&self, playlist_id: &str) -> Result<PluginLoadResult, PluginError> {
        let url = format!("https://www.youtube.com/playlist?list={playlist_id}");
        let result = self
            .extractor
            .resolve(&url)
            .await
            .map_err(|error| PluginError::new("youtube", error.to_string()))?;
        let MediaInfo::Collection(mut collection) = result else {
            return Err(PluginError::new(
                "youtube",
                "YouTube playlist did not return a result collection",
            ));
        };

        let name = collection
            .title
            .unwrap_or_else(|| "YouTube playlist".to_owned());
        let mut tracks = Vec::with_capacity(MAX_PLAYLIST_TRACKS);
        while tracks.len() < MAX_PLAYLIST_TRACKS {
            let Some(entry) = collection.entries.next().await else {
                break;
            };
            let entry = entry.map_err(|error| PluginError::new("youtube", error.to_string()))?;
            if let Ok(track) = track_from_entry(&entry) {
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
impl RavaLinkPlugin for YouTubePlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "youtube".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Native YouTube InnerTube video, playlist and search source".into(),
            capabilities: vec![
                "video".into(),
                "playlist".into(),
                "search".into(),
                "music-search".into(),
            ],
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

        // A watch URL with a playlist parameter is handled as a playlist so
        // Lavalink's selectedTrack semantics remain intact.
        if let Some(playlist_id) = parsed.playlist_id {
            let mut result = self.load_playlist(&playlist_id).await?;
            if let Some(video_id) = parsed.video_id {
                let tracks = result
                    .data
                    .get_mut("tracks")
                    .and_then(serde_json::Value::as_array_mut)
                    .ok_or_else(|| {
                        PluginError::new("youtube", "playlist response has no tracks")
                    })?;
                let selected = tracks.iter().position(|track| {
                    track
                        .get("info")
                        .and_then(|info| info.get("identifier"))
                        .and_then(serde_json::Value::as_str)
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
            let mut track = self.resolve_video(&video_id).await?;
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

async fn resolve_video_with_extractor(extractor: &Ytdown, url: &str) -> Result<Track, PluginError> {
    let info = extractor
        .resolve(url)
        .await
        .map_err(|error| PluginError::new("youtube", error.to_string()))?;
    let MediaInfo::Single(video) = info else {
        return Err(PluginError::new(
            "youtube",
            "YouTube returned a collection where a single video was expected",
        ));
    };

    let format = video
        .formats()
        .best_audio()
        .or_else(|_| video.formats().best_progressive())
        .map_err(|error| {
            PluginError::new(
                "youtube",
                format!("no playable audio format was returned: {error}"),
            )
        })?;
    if format.url.is_empty() {
        return Err(PluginError::new(
            "youtube",
            "YouTube returned an empty audio stream URL",
        ));
    }

    let mut track = track_from_video(&video)?;
    track.plugin_info = json!({
        "streamUrl": format.url,
        "provider": "youtube-innertube",
        "format": format.itag,
        "mimeType": format.mime_type,
        "httpHeaders": format.http_headers,
    });
    track.encoded =
        encode_track(&track).map_err(|error| PluginError::new("youtube", error.message))?;
    Ok(track)
}

fn track_from_video(video: &ytdown::VideoInfo) -> Result<Track, PluginError> {
    let artwork_url = video
        .thumbnails
        .last()
        .map(|thumbnail| thumbnail.url.clone());
    let length = video
        .duration
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default();
    let mut track = Track {
        encoded: String::new(),
        info: TrackInfo {
            identifier: video.id.clone(),
            is_seekable: !video.is_live,
            author: video
                .uploader
                .clone()
                .unwrap_or_else(|| "Unknown".to_owned()),
            length,
            is_stream: video.is_live,
            position: 0,
            title: video.title.clone(),
            uri: Some(video.webpage_url.clone()),
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

fn track_from_entry(entry: &ytdown::Entry) -> Result<Track, PluginError> {
    track_from_entry_fields(
        &entry.id,
        entry.title.as_deref().unwrap_or("Unknown title"),
        &entry.url,
        entry.duration,
        entry
            .thumbnails
            .last()
            .map(|thumbnail| thumbnail.url.clone()),
    )
}

fn track_from_entry_fields(
    id: &str,
    title: &str,
    url: &str,
    duration: Option<std::time::Duration>,
    artwork_url: Option<String>,
) -> Result<Track, PluginError> {
    if !valid_video_id(id) {
        return Err(PluginError::new(
            "youtube",
            "search result has an invalid video ID",
        ));
    }
    let length = duration
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default();
    let mut track = Track {
        encoded: String::new(),
        info: TrackInfo {
            identifier: id.to_owned(),
            is_seekable: true,
            author: "Unknown".to_owned(),
            length,
            is_stream: false,
            position: 0,
            title: title.to_owned(),
            uri: Some(url.to_owned()),
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

impl Default for YouTubePlugin {
    fn default() -> Self {
        let extractor = build_extractor(None, None)
            .expect("the native YouTube extractor must build with default settings");
        Self {
            extractor: Arc::new(extractor),
            oauth_extractor: Arc::new(RwLock::new(None)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OAuthToken {
    access_token: String,
    #[serde(default = "default_oauth_token_type")]
    token_type: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    #[serde(default = "default_oauth_poll_interval")]
    interval: u64,
    expires_in: u64,
}

fn default_oauth_poll_interval() -> u64 {
    5
}

fn default_oauth_token_lifetime() -> u64 {
    300
}

fn default_oauth_token_type() -> String {
    "Bearer".to_owned()
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OAuthResponse {
    Token {
        access_token: String,
        #[serde(default = "default_oauth_token_type")]
        token_type: String,
        #[serde(default)]
        refresh_token: String,
        #[serde(default = "default_oauth_token_lifetime")]
        expires_in: u64,
    },
    Error {
        error: String,
        #[serde(default)]
        error_description: String,
    },
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn youtube_tv_http() -> Result<reqwest::Client, PluginError> {
    reqwest::Client::builder()
        .user_agent(YOUTUBE_TV_USER_AGENT)
        .build()
        .map_err(|error| {
            PluginError::new(
                "youtube",
                format!("cannot build OAuth HTTP client: {error}"),
            )
        })
}

fn oauth_request_headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header(USER_AGENT, YOUTUBE_TV_USER_AGENT)
        .header(ORIGIN, "https://www.youtube.com")
        .header(REFERER, YOUTUBE_TV_REFERER)
}

async fn load_stored_token(
    configured_refresh_token: Option<&str>,
    token_path: &Path,
) -> Result<Option<OAuthToken>, PluginError> {
    let refresh_token = if let Some(refresh_token) =
        configured_refresh_token.filter(|value| !value.trim().is_empty())
    {
        Some(refresh_token.to_owned())
    } else if token_path.exists() {
        let raw = std::fs::read_to_string(token_path).map_err(|error| {
            PluginError::new(
                "youtube",
                format!(
                    "cannot read OAuth token file {}: {error}",
                    token_path.display()
                ),
            )
        })?;
        let stored: OAuthToken = serde_json::from_str(&raw).map_err(|error| {
            PluginError::new(
                "youtube",
                format!("invalid OAuth token file {}: {error}", token_path.display()),
            )
        })?;
        (!stored.refresh_token.is_empty()).then_some(stored.refresh_token)
    } else {
        None
    };

    let Some(refresh_token) = refresh_token else {
        return Ok(None);
    };

    let mut token = refresh_access_token(&refresh_token).await?;
    if token.refresh_token.is_empty() {
        token.refresh_token = refresh_token;
    }
    token.expires_at = unix_time().saturating_add(token.expires_in);
    save_token(token_path, &token)?;
    Ok(Some(token))
}

async fn install_oauth_extractor(
    slot: Arc<RwLock<Option<Arc<Ytdown>>>>,
    cookies: Option<PathBuf>,
    token_file: PathBuf,
    token: OAuthToken,
) -> Result<(), PluginError> {
    let extractor = build_extractor(cookies.as_deref(), Some(&token))?;
    *slot.write().await = Some(Arc::new(extractor));
    spawn_oauth_refresh_loop(slot, cookies, token_file, token);
    Ok(())
}

fn spawn_oauth_refresh_loop(
    slot: Arc<RwLock<Option<Arc<Ytdown>>>>,
    cookies: Option<PathBuf>,
    token_file: PathBuf,
    mut current: OAuthToken,
) {
    tokio::spawn(async move {
        loop {
            let refresh_in = current
                .expires_at
                .saturating_sub(unix_time())
                .saturating_sub(90)
                .max(1);
            tokio::time::sleep(Duration::from_secs(refresh_in)).await;

            match refresh_access_token(&current.refresh_token).await {
                Ok(mut refreshed) => {
                    if refreshed.refresh_token.is_empty() {
                        refreshed.refresh_token = current.refresh_token.clone();
                    }
                    refreshed.expires_at = unix_time().saturating_add(refreshed.expires_in);
                    if let Err(error) = save_token(&token_file, &refreshed) {
                        tracing::error!(%error, "failed to persist refreshed YouTube OAuth token");
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        continue;
                    }
                    match build_extractor(cookies.as_deref(), Some(&refreshed)) {
                        Ok(next) => {
                            *slot.write().await = Some(Arc::new(next));
                            current = refreshed;
                            tracing::info!("YouTube TV OAuth access token refreshed");
                        }
                        Err(error) => {
                            tracing::error!(message = %error.message, "failed to rebuild OAuth YouTube extractor");
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "YouTube TV OAuth refresh failed; retrying");
                    if unix_time() >= current.expires_at {
                        // Do not keep sending an expired Bearer token. The
                        // anonymous extractor remains available until refresh
                        // succeeds and installs a new authenticated client.
                        *slot.write().await = None;
                    }
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
        }
    });
}

async fn authorize_device() -> Result<OAuthToken, PluginError> {
    let http = youtube_tv_http()?;
    let device_id = uuid::Uuid::new_v4().simple().to_string();
    let device: DeviceCodeResponse = oauth_request_headers(http.post(YOUTUBE_OAUTH_DEVICE_URL))
        .json(&serde_json::json!({
            "client_id": YOUTUBE_OAUTH_CLIENT_ID,
            "scope": YOUTUBE_OAUTH_SCOPE,
            "device_id": device_id,
            "device_model": "ytlr::"
        }))
        .send()
        .await
        .map_err(|error| {
            PluginError::new(
                "youtube",
                format!("YouTube OAuth device request failed: {error}"),
            )
        })?
        .error_for_status()
        .map_err(|error| {
            PluginError::new(
                "youtube",
                format!("YouTube OAuth device request rejected: {error}"),
            )
        })?
        .json()
        .await
        .map_err(|error| {
            PluginError::new(
                "youtube",
                format!("invalid YouTube OAuth device response: {error}"),
            )
        })?;

    tracing::warn!("==================================================");
    tracing::warn!("YouTube OAuth: use a secondary/burner account, not your primary account.");
    tracing::info!(
        "To authorize YouTube playback, open {} and enter code {}",
        device.verification_url,
        device.user_code
    );
    tracing::warn!("==================================================");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = device.interval.max(1);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(PluginError::new(
                "youtube",
                "YouTube OAuth device code expired; restart RavaLink to retry",
            ));
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let response = match oauth_request_headers(http.post(YOUTUBE_OAUTH_TOKEN_URL))
            .json(&serde_json::json!({
                "client_id": YOUTUBE_OAUTH_CLIENT_ID,
                "client_secret": YOUTUBE_OAUTH_CLIENT_SECRET,
                "code": device.device_code,
                "grant_type": "http://oauth.net/grant_type/device/1.0"
            }))
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "YouTube OAuth token poll failed; retrying");
                continue;
            }
        };

        let payload: OAuthResponse = match response.json().await {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(%error, "invalid YouTube OAuth token poll response; retrying");
                continue;
            }
        };
        match payload {
            OAuthResponse::Token {
                access_token,
                token_type,
                refresh_token,
                expires_in,
            } => {
                if refresh_token.is_empty() {
                    return Err(PluginError::new(
                        "youtube",
                        "YouTube OAuth did not return a refresh token",
                    ));
                }
                return Ok(OAuthToken {
                    access_token,
                    token_type,
                    refresh_token,
                    expires_in,
                    expires_at: unix_time().saturating_add(expires_in),
                });
            }
            OAuthResponse::Error {
                error,
                error_description,
            } => match error.as_str() {
                "authorization_pending" => continue,
                "slow_down" => interval = interval.saturating_add(5),
                "access_denied" => {
                    return Err(PluginError::new(
                        "youtube",
                        "YouTube OAuth authorization was denied",
                    ));
                }
                "expired_token" => {
                    return Err(PluginError::new(
                        "youtube",
                        "YouTube OAuth device code expired; restart RavaLink to retry",
                    ));
                }
                other => {
                    return Err(PluginError::new(
                        "youtube",
                        format!("YouTube OAuth failed: {other}: {error_description}"),
                    ));
                }
            },
        }
    }
}

async fn refresh_access_token(refresh_token: &str) -> Result<OAuthToken, PluginError> {
    let http = youtube_tv_http()?;
    let response = oauth_request_headers(http.post(YOUTUBE_OAUTH_TOKEN_URL))
        .json(&serde_json::json!({
            "client_id": YOUTUBE_OAUTH_CLIENT_ID,
            "client_secret": YOUTUBE_OAUTH_CLIENT_SECRET,
            "refresh_token": refresh_token,
            "grant_type": "refresh_token"
        }))
        .send()
        .await
        .map_err(|error| {
            PluginError::new(
                "youtube",
                format!("YouTube OAuth refresh request failed: {error}"),
            )
        })?;
    let status = response.status();
    let payload: OAuthResponse = response.json().await.map_err(|error| {
        PluginError::new(
            "youtube",
            format!("invalid YouTube OAuth refresh response ({status}): {error}"),
        )
    })?;
    match payload {
        OAuthResponse::Token {
            access_token,
            token_type,
            refresh_token: next_refresh,
            expires_in,
        } => Ok(OAuthToken {
            access_token,
            token_type,
            refresh_token: if next_refresh.is_empty() {
                refresh_token.to_owned()
            } else {
                next_refresh
            },
            expires_in,
            expires_at: unix_time().saturating_add(expires_in),
        }),
        OAuthResponse::Error {
            error,
            error_description,
        } => Err(PluginError::new(
            "youtube",
            format!("YouTube OAuth refresh rejected ({status}): {error}: {error_description}"),
        )),
    }
}

fn save_token(path: &Path, token: &OAuthToken) -> Result<(), PluginError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            PluginError::new(
                "youtube",
                format!("cannot create OAuth token directory: {error}"),
            )
        })?;
    }
    let data = serde_json::to_vec_pretty(token).map_err(|error| {
        PluginError::new("youtube", format!("cannot serialize OAuth token: {error}"))
    })?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, data).map_err(|error| {
        PluginError::new("youtube", format!("cannot write OAuth token file: {error}"))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| {
                PluginError::new(
                    "youtube",
                    format!("cannot secure OAuth token file: {error}"),
                )
            },
        )?;
    }
    // Windows does not replace an existing destination during rename.
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| {
            PluginError::new(
                "youtube",
                format!("cannot replace existing OAuth token file: {error}"),
            )
        })?;
    }
    std::fs::rename(&temporary, path).map_err(|error| {
        PluginError::new(
            "youtube",
            format!("cannot install OAuth token file: {error}"),
        )
    })?;
    Ok(())
}

fn build_extractor(
    cookies_file: Option<&Path>,
    oauth_token: Option<&OAuthToken>,
) -> Result<Ytdown, PluginError> {
    let token = oauth_token.map(|token| format!("{} {}", token.token_type, token.access_token));
    let mut builder = Ytdown::builder()
        .user_agent("RavaLink")
        .youtube_oauth_token(token);
    if let Some(path) = cookies_file {
        let contents = std::fs::read_to_string(path).map_err(|error| {
            PluginError::new(
                "youtube",
                format!(
                    "cannot read YouTube cookies file {}: {error}",
                    path.display()
                ),
            )
        })?;
        let jar = CookieJar::parse_netscape(&contents).map_err(|error| {
            PluginError::new("youtube", format!("invalid YouTube cookies file: {error}"))
        })?;
        builder = builder.cookies(jar);
    }
    builder
        .build()
        .map_err(|error| PluginError::new("youtube", error.to_string()))
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

fn valid_playlist_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lavalink_youtube_tv_device_code_response() {
        let response: DeviceCodeResponse = serde_json::from_str(
            r#"{
                "device_code": "device-code",
                "user_code": "ABCD-EFGH",
                "verification_url": "https://www.google.com/device",
                "expires_in": 1800
            }"#,
        )
        .unwrap();

        assert_eq!(response.device_code, "device-code");
        assert_eq!(response.user_code, "ABCD-EFGH");
        assert_eq!(response.verification_url, "https://www.google.com/device");
        assert_eq!(response.interval, 5);
    }

    #[test]
    fn parses_youtube_oauth_refresh_response_without_rotating_refresh_token() {
        let response: OAuthResponse = serde_json::from_str(
            r#"{
                "access_token": "new-access-token",
                "expires_in": 3600,
                "token_type": "Bearer"
            }"#,
        )
        .unwrap();

        match response {
            OAuthResponse::Token {
                access_token,
                token_type,
                refresh_token,
                expires_in,
            } => {
                assert_eq!(access_token, "new-access-token");
                assert_eq!(token_type, "Bearer");
                assert!(refresh_token.is_empty());
                assert_eq!(expires_in, 3600);
            }
            OAuthResponse::Error { .. } => panic!("expected an OAuth token response"),
        }
    }

    #[test]
    fn parses_youtube_oauth_pending_authorization_response() {
        let response: OAuthResponse =
            serde_json::from_str(r#"{"error":"authorization_pending"}"#).unwrap();
        assert!(matches!(
            response,
            OAuthResponse::Error { error, .. } if error == "authorization_pending"
        ));
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
        let oversized = format!("ytsearch:{}", "x".repeat(MAX_SEARCH_QUERY_BYTES + 1));
        assert!(YouTubePlugin::search_query(&oversized).is_none());
    }

    #[test]
    fn creates_lavalink_track_metadata_from_video_entry() {
        let track = track_from_entry_fields(
            "dQw4w9WgXcQ",
            "Example",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            Some(std::time::Duration::from_secs(210)),
            None,
        )
        .unwrap();
        assert_eq!(track.info.source_name, "youtube");
        assert_eq!(track.info.length, 210_000);
        assert!(!track.encoded.is_empty());
    }
}
