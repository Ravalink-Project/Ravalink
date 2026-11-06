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
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use ytdown::{MediaInfo, Ytdown, cookies::CookieJar};

const MAX_SEARCH_RESULTS: usize = 50;
const MAX_PLAYLIST_TRACKS: usize = 500;
const MAX_SEARCH_QUERY_BYTES: usize = 512;

#[derive(Clone)]
pub struct YouTubePlugin {
    extractor: Arc<RwLock<Arc<Ytdown>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeIdentifier {
    pub video_id: Option<String>,
    pub playlist_id: Option<String>,
    pub start_seconds: Option<u64>,
}

impl YouTubePlugin {
    /// Build the native extractor with optional browser cookies and Google OAuth.
    /// OAuth uses Google's Device Authorization Grant and refreshes tokens in-process.
    pub async fn new(
        cookies_file: Option<&Path>,
        oauth_client_id: Option<&str>,
        oauth_client_secret: Option<&str>,
        oauth_token_file: Option<&Path>,
    ) -> Result<Self, PluginError> {
        let cookies_file = cookies_file.map(Path::to_path_buf);
        let oauth = match (oauth_client_id, oauth_client_secret, oauth_token_file) {
            (Some(client_id), Some(client_secret), Some(token_file))
                if !client_id.trim().is_empty() && !client_secret.trim().is_empty() =>
            {
                let token = load_or_authorize(client_id, client_secret, token_file).await?;
                Some((
                    client_id.to_owned(),
                    client_secret.to_owned(),
                    token_file.to_path_buf(),
                    token,
                ))
            }
            (None, None, None) => None,
            _ => {
                return Err(PluginError::new(
                    "youtube",
                    "youtube.oauth_client_id and youtube.oauth_token_file must be configured together",
                ));
            }
        };

        let token = oauth
            .as_ref()
            .map(|(_, _, _, token)| token.access_token.as_str());
        let extractor = Arc::new(build_extractor(cookies_file.as_deref(), token)?);
        let plugin = Self {
            extractor: Arc::new(RwLock::new(extractor)),
        };

        if let Some((client_id, client_secret, token_file, token)) = oauth {
            let extractor_slot = plugin.extractor.clone();
            tokio::spawn(async move {
                let http = reqwest::Client::new();
                let mut current = token;
                loop {
                    let now = unix_time();
                    let refresh_at = current
                        .created_at
                        .saturating_add(current.expires_in.saturating_sub(90));
                    tokio::time::sleep(Duration::from_secs(refresh_at.saturating_sub(now).max(1)))
                        .await;
                    match refresh_access_token(
                        &http,
                        &client_id,
                        &client_secret,
                        &current.refresh_token,
                    )
                    .await
                    {
                        Ok(mut refreshed) => {
                            if refreshed.refresh_token.is_empty() {
                                refreshed.refresh_token = current.refresh_token.clone();
                            }
                            refreshed.created_at = unix_time();
                            if let Err(error) = save_token(&token_file, &refreshed) {
                                tracing::error!(%error, "failed to persist refreshed YouTube OAuth token");
                                tokio::time::sleep(Duration::from_secs(30)).await;
                                continue;
                            }
                            match build_extractor(
                                cookies_file.as_deref(),
                                Some(&refreshed.access_token),
                            ) {
                                Ok(next) => {
                                    *extractor_slot.write().await = Arc::new(next);
                                    current = refreshed;
                                    tracing::info!("YouTube OAuth access token refreshed");
                                }
                                Err(error) => {
                                    tracing::error!(message = %error.message, "failed to rebuild YouTube extractor after OAuth refresh");
                                    tokio::time::sleep(Duration::from_secs(30)).await;
                                }
                            }
                        }
                        Err(error) => {
                            tracing::error!(%error, "YouTube OAuth token refresh failed; retrying");
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                    }
                }
            });
        }
        Ok(plugin)
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
        let extractor = self.extractor.read().await.clone();
        let info = extractor
            .resolve(&url)
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

    pub async fn search_tracks(&self, query: &str) -> Result<Vec<Track>, PluginError> {
        let extractor = self.extractor.read().await.clone();
        let result = extractor
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
        while tracks.len() < MAX_SEARCH_RESULTS {
            let Some(entry) = collection.entries.next().await else {
                break;
            };
            let entry = entry.map_err(|error| PluginError::new("youtube", error.to_string()))?;
            if let Ok(track) = track_from_entry(&entry) {
                tracks.push(track);
            }
        }
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
        let extractor = self.extractor.read().await.clone();
        let result = extractor
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
            extractor: Arc::new(RwLock::new(Arc::new(extractor))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OAuthToken {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default = "default_bearer")]
    token_type: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    created_at: u64,
}
fn default_bearer() -> String {
    "Bearer".into()
}
fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn build_extractor(
    cookies_file: Option<&Path>,
    access_token: Option<&str>,
) -> Result<Ytdown, PluginError> {
    let mut builder = Ytdown::builder().user_agent(concat!("RavaLink/", env!("CARGO_PKG_VERSION")));
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
    if let Some(token) = access_token {
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|error| {
            PluginError::new(
                "youtube",
                format!("invalid OAuth access token header: {error}"),
            )
        })?;
        headers.insert(AUTHORIZATION, value);
        let client = reqwest::Client::builder()
            .user_agent(concat!("RavaLink/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .build()
            .map_err(|error| {
                PluginError::new(
                    "youtube",
                    format!("cannot build OAuth HTTP client: {error}"),
                )
            })?;
        builder = builder.client(client);
    }
    builder
        .build()
        .map_err(|error| PluginError::new("youtube", error.to_string()))
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    #[serde(default)]
    verification_url_complete: Option<String>,
    #[serde(default = "default_poll_interval")]
    interval: u64,
    expires_in: u64,
}
fn default_poll_interval() -> u64 {
    5
}

#[derive(Deserialize)]
struct OAuthErrorResponse {
    error: String,
}

async fn load_or_authorize(
    client_id: &str,
    client_secret: &str,
    path: &Path,
) -> Result<OAuthToken, PluginError> {
    if path.exists() {
        let raw = std::fs::read_to_string(path).map_err(|error| {
            PluginError::new(
                "youtube",
                format!("cannot read OAuth token file {}: {error}", path.display()),
            )
        })?;
        let token: OAuthToken = serde_json::from_str(&raw).map_err(|error| {
            PluginError::new(
                "youtube",
                format!("invalid OAuth token file {}: {error}", path.display()),
            )
        })?;
        if !token.refresh_token.is_empty() {
            if token.created_at.saturating_add(token.expires_in) > unix_time().saturating_add(90) {
                return Ok(token);
            }
            let http = reqwest::Client::new();
            let mut refreshed =
                refresh_access_token(&http, client_id, client_secret, &token.refresh_token)
                    .await
                    .map_err(|error| {
                    PluginError::new(
                        "youtube",
                        format!("cannot refresh stored OAuth token: {error}"),
                    )
                })?;
            if refreshed.refresh_token.is_empty() {
                refreshed.refresh_token = token.refresh_token;
            }
            refreshed.created_at = unix_time();
            save_token(path, &refreshed)?;
            return Ok(refreshed);
        }
    }
    authorize_device(client_id, client_secret, path).await
}

async fn authorize_device(
    client_id: &str,
    client_secret: &str,
    token_path: &Path,
) -> Result<OAuthToken, PluginError> {
    let http = reqwest::Client::new();
    let device: DeviceCodeResponse = http
        .post("https://oauth2.googleapis.com/device/code")
        .form(&[
            ("client_id", client_id),
            ("scope", "https://www.googleapis.com/auth/youtube.readonly"),
        ])
        .send()
        .await
        .map_err(|error| {
            PluginError::new("youtube", format!("OAuth device request failed: {error}"))
        })?
        .error_for_status()
        .map_err(|error| {
            PluginError::new("youtube", format!("OAuth device request rejected: {error}"))
        })?
        .json()
        .await
        .map_err(|error| {
            PluginError::new("youtube", format!("invalid OAuth device response: {error}"))
        })?;
    eprintln!(
        "
[RavaLink YouTube OAuth] Open {} and enter code {}{}
",
        device.verification_url,
        device.user_code,
        device
            .verification_url_complete
            .as_deref()
            .map(|url| format!(" (or visit {url})"))
            .unwrap_or_default()
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = device.interval.max(5);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(PluginError::new(
                "youtube",
                "Google OAuth device code expired; restart RavaLink to retry",
            ));
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let response = http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("device_code", device.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|error| {
                PluginError::new("youtube", format!("OAuth token polling failed: {error}"))
            })?;
        if response.status().is_success() {
            let mut token: OAuthToken = response.json().await.map_err(|error| {
                PluginError::new("youtube", format!("invalid OAuth token response: {error}"))
            })?;
            token.created_at = unix_time();
            save_token(token_path, &token)?;
            return Ok(token);
        }
        let error: OAuthErrorResponse = response.json().await.unwrap_or(OAuthErrorResponse {
            error: "unknown_error".into(),
        });
        match error.error.as_str() {
            "authorization_pending" => continue,
            "slow_down" => {
                interval = interval.saturating_add(5);
            }
            "access_denied" => {
                return Err(PluginError::new(
                    "youtube",
                    "Google OAuth authorization was denied",
                ));
            }
            "expired_token" => {
                return Err(PluginError::new(
                    "youtube",
                    "Google OAuth device code expired; restart RavaLink to retry",
                ));
            }
            other => {
                return Err(PluginError::new(
                    "youtube",
                    format!("Google OAuth failed: {other}"),
                ));
            }
        }
    }
}

async fn refresh_access_token(
    http: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<OAuthToken, String> {
    let response = http
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Google returned {status}: {body}"));
    }
    response.json().await.map_err(|error| error.to_string())
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
    std::fs::rename(&temporary, path).map_err(|error| {
        PluginError::new(
            "youtube",
            format!("cannot install OAuth token file: {error}"),
        )
    })?;
    Ok(())
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
    fn parses_google_oauth_device_code_response_defaults() {
        let response: DeviceCodeResponse = serde_json::from_str(
            r#"{
                "device_code": "device",
                "user_code": "ABCD-EFGH",
                "verification_url": "https://www.google.com/device",
                "expires_in": 1800
            }"#,
        )
        .unwrap();

        assert_eq!(response.device_code, "device");
        assert_eq!(response.user_code, "ABCD-EFGH");
        assert_eq!(response.verification_url, "https://www.google.com/device");
        assert_eq!(response.interval, 5);
        assert_eq!(response.verification_url_complete, None);
    }

    #[test]
    fn oauth_token_roundtrips_and_accepts_google_response_fields() {
        let token: OAuthToken = serde_json::from_str(
            r#"{
                "access_token": "access",
                "refresh_token": "refresh",
                "token_type": "Bearer",
                "expires_in": 3600
            }"#,
        )
        .unwrap();

        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token, "refresh");
        assert_eq!(token.expires_in, 3600);
        assert_eq!(token.created_at, 0);

        let serialized = serde_json::to_value(&token).unwrap();
        assert_eq!(serialized["access_token"], "access");
        assert_eq!(serialized["refresh_token"], "refresh");
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
