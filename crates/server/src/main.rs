pub mod config;

use axum::{
    Json, Router,
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use ravalink_audio::{
    AudioError, AudioTrack, CachedHttpSource, DiscordVoiceConnection, HttpSourceManager,
    decode_bytes_to_channel_at,
};
use ravalink_core::{AudioPlayerStore, DiscordVoiceState, PlayerStore, SessionStore};
use ravalink_plugin_api::{PluginDescriptor, PluginRegistry, ProcessPlugin};
use ravalink_plugin_lavasearch::LavaSearchPlugin;
use ravalink_plugin_lavasrc::LavaSrcPlugin;
use ravalink_plugin_youtube::YouTubePlugin;
use ravalink_protocol::{
    ApiError, CpuStats, EncodedTracks, ExceptionSeverity, LoadResult, MemoryStats, NodeStats,
    PlayerStateUpdate, PlayerUpdateMessage, ServerInfo, SessionInfo, SessionUpdate, StatsMessage,
    Track, TrackEndEvent, TrackEndReason, TrackEvent, TrackEventBase, TrackException,
    TrackExceptionEvent, TrackStuckEvent, WebSocketMessage, decode_track, encode_track,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::System;
use tokio::{net::TcpListener, sync::watch};
use tracing::{info, warn};

type VoiceTaskMap = Arc<tokio::sync::Mutex<HashMap<(String, String), watch::Sender<bool>>>>;
#[derive(Clone)]
struct PlaybackSourceStore {
    inner: Arc<tokio::sync::Mutex<PlaybackSourceState>>,
    max_bytes: usize,
}

#[derive(Default)]
struct PlaybackSourceState {
    entries: HashMap<(String, String), CachedHttpSource>,
    bytes_used: usize,
}

impl PlaybackSourceStore {
    fn new(max_bytes: usize) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(PlaybackSourceState::default())),
            max_bytes,
        }
    }

    #[cfg(test)]
    async fn replace(
        &self,
        key: (String, String),
        source: CachedHttpSource,
    ) -> Result<Option<CachedHttpSource>, ()> {
        let mut state = self.inner.lock().await;
        let previous_bytes = state
            .entries
            .get(&key)
            .map_or(0, |previous| previous.bytes.len());
        let bytes = source.bytes.len();
        let bytes_without_previous = state.bytes_used.saturating_sub(previous_bytes);
        if bytes > self.max_bytes || bytes_without_previous.saturating_add(bytes) > self.max_bytes {
            return Err(());
        }
        let previous = state.entries.remove(&key);
        state.bytes_used = bytes_without_previous.saturating_add(bytes);
        state.entries.insert(key, source);
        Ok(previous)
    }

    #[cfg(test)]
    async fn restore(
        &self,
        key: (String, String),
        expected: &CachedHttpSource,
        previous: Option<CachedHttpSource>,
    ) {
        let mut state = self.inner.lock().await;
        let is_current = state
            .entries
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(&current.bytes, &expected.bytes));
        if !is_current {
            return;
        }
        if let Some(current) = state.entries.remove(&key) {
            state.bytes_used = state.bytes_used.saturating_sub(current.bytes.len());
        }
        if let Some(previous) = previous {
            let bytes = previous.bytes.len();
            if state.bytes_used.saturating_add(bytes) <= self.max_bytes {
                state.bytes_used = state.bytes_used.saturating_add(bytes);
                state.entries.insert(key, previous);
            }
        }
    }

    async fn get(&self, key: &(String, String)) -> Option<CachedHttpSource> {
        self.inner.lock().await.entries.get(key).cloned()
    }

    async fn remove(&self, key: &(String, String)) {
        let mut state = self.inner.lock().await;
        if let Some(source) = state.entries.remove(key) {
            state.bytes_used = state.bytes_used.saturating_sub(source.bytes.len());
        }
    }

    async fn remove_session(&self, session_id: &str) {
        let mut state = self.inner.lock().await;
        state
            .entries
            .retain(|(session, _), _| session != session_id);
        state.bytes_used = state.entries.values().fold(0_usize, |total, source| {
            total.saturating_add(source.bytes.len())
        });
    }

    async fn bytes_used(&self) -> usize {
        self.inner.lock().await.bytes_used
    }
}

#[derive(Clone)]
struct AppState {
    password: String,
    sessions: SessionStore,
    players: PlayerStore,
    audio_players: AudioPlayerStore,
    voice_tasks: VoiceTaskMap,
    http_sources: HttpSourceManager,
    playback_sources: PlaybackSourceStore,
    plugins: PluginRegistry,
    metrics_enabled: bool,
    player_update_interval: u64,
    system: Arc<tokio::sync::Mutex<System>>,
    stats_cache: Arc<tokio::sync::Mutex<Option<(Instant, NodeStats)>>>,
    started_at: Instant,
    events: tokio::sync::broadcast::Sender<(String, WebSocketMessage)>,
}

#[derive(Deserialize)]
struct LoadTracksQuery {
    identifier: String,
}

#[derive(Deserialize, Default)]
struct DecodeTrackQuery {
    #[serde(rename = "encodedTrack")]
    encoded_track: Option<String>,
    track: Option<String>,
}

#[derive(Deserialize)]
struct RoutePlannerFreeAddress {
    #[serde(rename = "address")]
    _address: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PlayerUpdateQuery {
    #[serde(default)]
    no_replace: bool,
}

fn has_http_scheme(identifier: &str) -> bool {
    identifier.split_once(':').is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    })
}

type PlayerFieldValidator = (&'static str, fn(&serde_json::Value) -> bool);

fn invalid_player_field(
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Option<&'static str> {
    let field_validators: [PlayerFieldValidator; 7] = [
        ("identifier", |value: &serde_json::Value| {
            value.is_null() || value.is_string()
        }),
        ("encodedTrack", |value: &serde_json::Value| {
            value.is_null() || value.is_string()
        }),
        ("track", |value: &serde_json::Value| {
            value.is_null() || value.is_object()
        }),
        ("paused", |value: &serde_json::Value| value.is_boolean()),
        ("position", |value: &serde_json::Value| {
            value.as_u64().is_some()
        }),
        ("volume", |value: &serde_json::Value| {
            value.as_u64().is_some_and(|volume| volume <= 1_000)
        }),
        ("filters", |value: &serde_json::Value| value.is_object()),
    ];
    field_validators.into_iter().find_map(|(field, valid)| {
        fields
            .get(field)
            .filter(|value| !valid(value))
            .map(|_| field)
    })
}

fn built_in_plugins(
    external: &[config::ExternalPluginConfig],
    plugin_directory: &std::path::Path,
) -> Result<PluginRegistry, String> {
    let mut plugins = PluginRegistry::default();
    plugins.register(YouTubePlugin::default());
    plugins.register(LavaSrcPlugin::default());
    plugins.register(LavaSearchPlugin::default());
    for plugin in external {
        plugins.register(
            ProcessPlugin::new(
                PluginDescriptor {
                    name: plugin.name.clone(),
                    version: plugin.version.clone(),
                    description: plugin.description.clone(),
                    capabilities: plugin.capabilities.clone(),
                },
                plugin.identifier_prefix.clone(),
                plugin.program.clone(),
                plugin.args.clone(),
                plugin.environment.clone(),
            )
            .with_config(plugin.config.clone()),
        );
    }
    let loaded = plugins
        .load_directory(plugin_directory)
        .map_err(|error| error.to_string())?;
    info!(loaded, directory = %plugin_directory.display(), "community plugins discovered");
    Ok(plugins)
}

#[tokio::main]
async fn main() {
    let config = config::Config::load().expect("failed to load RavaLink configuration");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| config.logging.level.clone().into()),
        )
        .init();

    let address = config.server.address.clone();
    let port = config.server.port;
    let password = config.server.password.clone();

    let (events, _) = tokio::sync::broadcast::channel(256);
    let state = AppState {
        password,
        sessions: SessionStore::default(),
        players: PlayerStore::default(),
        audio_players: AudioPlayerStore::new_with_limits(
            config.audio.opus_encoding_quality,
            config.audio.max_active_decoded_pcm_bytes,
        ),
        voice_tasks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        http_sources: HttpSourceManager::with_limits(
            config.audio.max_source_download_bytes,
            config.audio.max_decoded_cache_bytes,
        ),
        playback_sources: PlaybackSourceStore::new(config.audio.max_decoded_cache_bytes),
        plugins: built_in_plugins(&config.plugins, &config.plugin_directory)
            .expect("failed to load configured plugins"),
        metrics_enabled: config.metrics.prometheus_enabled,
        player_update_interval: config.server.player_update_interval,
        system: Arc::new(tokio::sync::Mutex::new(System::new_all())),
        stats_cache: Arc::new(tokio::sync::Mutex::new(None)),
        started_at: Instant::now(),
        events,
    };

    let cleanup_sessions = state.sessions.clone();
    let cleanup_players = state.players.clone();
    let cleanup_audio_players = state.audio_players.clone();
    let cleanup_playback_sources = state.playback_sources.clone();
    let cleanup_voice_tasks = state.voice_tasks.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            for session_id in cleanup_sessions.prune_expired().await {
                cleanup_session_resources(
                    &cleanup_players,
                    &cleanup_audio_players,
                    &cleanup_playback_sources,
                    &cleanup_voice_tasks,
                    &session_id,
                )
                .await;
            }
        }
    });

    let app = Router::new()
        .route("/version", get(version))
        .route("/v4/info", get(info_endpoint))
        .route("/v4/stats", get(stats))
        .route("/v4/routeplanner/status", get(routeplanner_status))
        .route(
            "/v4/routeplanner/free/address",
            post(routeplanner_free_address),
        )
        .route("/v4/routeplanner/free/all", post(routeplanner_free_all))
        .route("/v4/loadtracks", get(load_tracks))
        .route("/v4/decodetrack", get(decode_track_endpoint))
        .route("/v4/decodetracks", post(decode_tracks_endpoint))
        .route(
            "/v4/sessions/{session_id}",
            axum::routing::patch(update_session),
        )
        .route("/v4/sessions/{session_id}/players", get(get_players))
        .route(
            "/v4/sessions/{session_id}/players/{guild_id}",
            get(get_player).patch(update_player).delete(delete_player),
        )
        .route("/v4/websocket", get(websocket));

    let app = if config.metrics.prometheus_enabled {
        app.route(&config.metrics.prometheus_endpoint, get(metrics_endpoint))
    } else {
        app
    };
    let app = app.with_state(state);

    let socket = format!("{address}:{port}");
    let listener = TcpListener::bind(&socket)
        .await
        .expect("failed to bind listener");
    info!(%socket, "RavaLink native server starting");
    axum::serve(listener, app).await.expect("server failed");
}

async fn metrics_endpoint(State(state): State<AppState>) -> Response {
    if !state.metrics_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }

    let sessions = state.sessions.len().await;
    let players = state.players.len().await;
    let playing_players = state.audio_players.playing_len().await;
    let cache_bytes = state.http_sources.cached_bytes().unwrap_or_default();
    let playback_source_bytes = state.playback_sources.bytes_used().await;
    let playback_source_limit = state.playback_sources.max_bytes;
    let active_pcm_bytes = state.audio_players.active_pcm_bytes();
    let active_pcm_limit = state.audio_players.max_active_pcm_bytes();
    let uptime_seconds = state.started_at.elapsed().as_secs_f64();

    let body = format!(
        "# HELP ravalink_uptime_seconds Time since this RavaLink process started.\n# TYPE ravalink_uptime_seconds gauge\nravalink_uptime_seconds {uptime_seconds:.3}\n# HELP ravalink_sessions Number of active Lavalink sessions.\n# TYPE ravalink_sessions gauge\nravalink_sessions {sessions}\n# HELP ravalink_players Number of player states across sessions.\n# TYPE ravalink_players gauge\nravalink_players {players}\n# HELP ravalink_playing_players Number of players with a non-paused track.\n# TYPE ravalink_playing_players gauge\nravalink_playing_players {playing_players}\n# HELP ravalink_source_cache_bytes Compressed media bytes currently held by the HTTP source cache.\n# TYPE ravalink_source_cache_bytes gauge\nravalink_source_cache_bytes {cache_bytes}\n# HELP ravalink_playback_seek_cache_bytes Compressed source bytes retained for active player seeking.\n# TYPE ravalink_playback_seek_cache_bytes gauge\nravalink_playback_seek_cache_bytes {playback_source_bytes}\n# HELP ravalink_playback_seek_cache_limit_bytes Configured aggregate compressed source seek-cache limit.\n# TYPE ravalink_playback_seek_cache_limit_bytes gauge\nravalink_playback_seek_cache_limit_bytes {playback_source_limit}\n# HELP ravalink_active_decoded_pcm_bytes Decoded PCM bytes held by active native players.\n# TYPE ravalink_active_decoded_pcm_bytes gauge\nravalink_active_decoded_pcm_bytes {active_pcm_bytes}\n# HELP ravalink_active_decoded_pcm_limit_bytes Configured aggregate active-player PCM limit.\n# TYPE ravalink_active_decoded_pcm_limit_bytes gauge\nravalink_active_decoded_pcm_limit_bytes {active_pcm_limit}\n"
    );

    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

fn authorization_failure(headers: &HeaderMap, state: &AppState) -> Option<Response> {
    match headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value == state.password => None,
        Some(_) => Some((StatusCode::FORBIDDEN, "Forbidden").into_response()),
        None => Some((StatusCode::UNAUTHORIZED, "Unauthorized").into_response()),
    }
}

async fn version(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    "4.0.0-ravalink".into_response()
}

async fn info_endpoint(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    let mut source_managers = ServerInfo::default().source_managers;
    source_managers.extend([
        "youtube".to_owned(),
        "spotify".to_owned(),
        "applemusic".to_owned(),
        "deezer".to_owned(),
    ]);
    for descriptor in state.plugins.descriptors() {
        source_managers.extend(
            descriptor
                .capabilities
                .iter()
                .filter_map(|capability| capability.strip_prefix("source:"))
                .map(str::to_owned),
        );
    }
    source_managers.sort();
    source_managers.dedup();
    let info = ServerInfo {
        plugins: state.plugins.infos(),
        source_managers,
        ..ServerInfo::default()
    };
    Json(info).into_response()
}

async fn stats(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    Json(collect_stats(&state).await).into_response()
}

async fn collect_stats(state: &AppState) -> NodeStats {
    let mut cache = state.stats_cache.lock().await;
    if let Some((updated_at, cached)) = cache.as_ref()
        && updated_at.elapsed() < Duration::from_secs(5)
    {
        return cached.clone();
    }

    let mut system = state.system.lock().await;
    system.refresh_all();
    system.refresh_cpu_usage();
    let total = system.total_memory();
    let used = system.used_memory();
    let free = total.saturating_sub(used);
    let cores = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let system_load = f64::from(system.global_cpu_usage()) / 100.0;
    let current_process = sysinfo::get_current_pid()
        .ok()
        .and_then(|pid| system.process(pid));
    let process_load = current_process
        .map(|process| f64::from(process.cpu_usage()) / 100.0)
        .unwrap_or(0.0);
    drop(system);

    let stats = NodeStats {
        frame_stats: None,
        players: state.players.len().await,
        playing_players: state.audio_players.playing_len().await,
        uptime: state.started_at.elapsed().as_millis() as u64,
        memory: MemoryStats {
            free,
            used,
            allocated: total,
            reservable: free,
        },
        cpu: CpuStats {
            cores,
            system_load,
            lavalink_load: process_load,
        },
    };
    *cache = Some((Instant::now(), stats.clone()));
    stats
}

async fn routeplanner_status(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn routeplanner_free_address(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(_body): Json<RoutePlannerFreeAddress>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Can't access disabled route planner",
        "/v4/routeplanner/free/address",
    )
}

async fn routeplanner_free_all(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Can't access disabled route planner",
        "/v4/routeplanner/free/all",
    )
}

async fn load_tracks(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<LoadTracksQuery>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    match state.plugins.load_tracks(&query.identifier).await {
        Ok(Some(plugin_result)) => {
            return Json(LoadResult {
                load_type: plugin_result.load_type.as_lavalink_str(),
                data: Some(plugin_result.data),
            })
            .into_response();
        }
        Err(error) => {
            return Json(LoadResult {
                load_type: "error",
                data: Some(serde_json::json!({
                    "message": error.message,
                    "severity": "common",
                    "cause": error.message
                })),
            })
            .into_response();
        }
        Ok(None) => {}
    }

    if !has_http_scheme(&query.identifier) {
        return Json(LoadResult::empty()).into_response();
    }

    match state
        .http_sources
        .resolve_stream_track(&query.identifier)
        .await
    {
        Ok(resolved) => Json(LoadResult {
            load_type: "track",
            data: Some(serde_json::to_value(resolved.track).expect("Track serializes to JSON")),
        })
        .into_response(),
        Err(error) => {
            let message = error.message;
            Json(LoadResult {
                load_type: "error",
                data: Some(serde_json::json!({
                    "message": message,
                    "severity": "common",
                    "cause": message
                })),
            })
            .into_response()
        }
    }
}

async fn update_session(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(update): Json<SessionUpdate>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    match state
        .sessions
        .update(&session_id, update.resuming, update.timeout)
        .await
    {
        Some(session) => Json(SessionInfo {
            resuming: session.resumable,
            timeout: session.resume_timeout_seconds,
        })
        .into_response(),
        None => api_error(
            StatusCode::NOT_FOUND,
            "Session not found",
            &format!("/v4/sessions/{session_id}"),
        ),
    }
}

async fn get_players(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    if state.sessions.get(&session_id).await.is_none() {
        return api_error(
            StatusCode::NOT_FOUND,
            "Session not found",
            &format!("/v4/sessions/{session_id}/players"),
        );
    }
    Json(state.players.list(&session_id).await).into_response()
}

async fn get_player(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((session_id, guild_id)): Path<(String, String)>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    if state.sessions.get(&session_id).await.is_none() {
        return api_error(
            StatusCode::NOT_FOUND,
            "Session not found",
            &format!("/v4/sessions/{session_id}/players/{guild_id}"),
        );
    }
    match state.players.get(&session_id, &guild_id).await {
        Some(player) => Json(player).into_response(),
        None => api_error(
            StatusCode::NOT_FOUND,
            "Player not found",
            &format!("/v4/sessions/{session_id}/players/{guild_id}"),
        ),
    }
}

async fn delete_player(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((session_id, _guild_id)): Path<(String, String)>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    if state.sessions.get(&session_id).await.is_none() {
        return api_error(
            StatusCode::NOT_FOUND,
            "Session not found",
            &format!("/v4/sessions/{session_id}"),
        );
    }
    if let Some(encoded) = state
        .players
        .get(&session_id, &_guild_id)
        .await
        .and_then(|player| player["track"]["encoded"].as_str().map(str::to_owned))
    {
        publish_track_end(
            &state,
            &session_id,
            &_guild_id,
            encoded,
            TrackEndReason::Cleanup,
        );
    }
    shutdown_voice_task(&state, &session_id, &_guild_id).await;
    state
        .playback_sources
        .remove(&(session_id.clone(), _guild_id.clone()))
        .await;
    state.audio_players.remove(&session_id, &_guild_id).await;
    state.players.remove(&session_id, &_guild_id).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn update_player(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((session_id, guild_id)): Path<(String, String)>,
    Query(query): Query<PlayerUpdateQuery>,
    Json(update): Json<serde_json::Value>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    let path = format!("/v4/sessions/{session_id}/players/{guild_id}");
    if state.sessions.get(&session_id).await.is_none() {
        return api_error(StatusCode::NOT_FOUND, "Session not found", &path);
    }
    let Some(fields) = update.as_object() else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Player update must be a JSON object",
            &path,
        );
    };

    if let Some(field) = invalid_player_field(fields) {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("Invalid value for player field '{field}'"),
            &path,
        );
    }

    let has_identifier = fields
        .get("identifier")
        .is_some_and(|value| !value.is_null());
    let has_encoded_track = fields
        .get("encodedTrack")
        .is_some_and(|value| !value.is_null());
    let has_track_object = fields.get("track").is_some_and(|track| !track.is_null());
    let has_track_request = has_identifier || has_encoded_track || has_track_object;
    let encoded_track_is_null = fields
        .get("encodedTrack")
        .is_some_and(serde_json::Value::is_null);
    if fields.contains_key("track")
        && (fields.contains_key("identifier") || fields.contains_key("encodedTrack"))
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Cannot specify both track and encodedTrack/identifier",
            &path,
        );
    }
    if fields.contains_key("identifier") && fields.contains_key("encodedTrack") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Cannot specify both encodedTrack and identifier",
            &path,
        );
    }

    let previous_encoded_track = state
        .players
        .get(&session_id, &guild_id)
        .await
        .and_then(|player| player["track"]["encoded"].as_str().map(str::to_owned));

    let no_replace_active = query.no_replace
        && has_track_request
        && state
            .players
            .get(&session_id, &guild_id)
            .await
            .is_some_and(|player| !player["track"].is_null());

    let voice_update = if let Some(voice) = fields.get("voice") {
        if voice.is_null() {
            Some(None)
        } else {
            let Some(voice) = voice.as_object() else {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "Voice state must be an object or null",
                    &path,
                );
            };
            let (Some(token), Some(endpoint), Some(session_id), Some(channel_id)) = (
                voice.get("token").and_then(serde_json::Value::as_str),
                voice.get("endpoint").and_then(serde_json::Value::as_str),
                voice.get("sessionId").and_then(serde_json::Value::as_str),
                voice.get("channelId").and_then(serde_json::Value::as_str),
            ) else {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "Voice state requires token, endpoint, sessionId and channelId strings",
                    &path,
                );
            };
            if token.is_empty()
                || endpoint.is_empty()
                || session_id.is_empty()
                || channel_id.is_empty()
            {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "Voice state fields cannot be empty",
                    &path,
                );
            }
            Some(Some(DiscordVoiceState {
                token: token.to_owned(),
                endpoint: endpoint.to_owned(),
                session_id: session_id.to_owned(),
                channel_id: channel_id.to_owned(),
            }))
        }
    } else {
        None
    };
    if let Some(end_time) = fields.get("endTime").filter(|value| !value.is_null())
        && end_time.as_u64().is_none_or(|value| value == 0)
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "End time must be greater than 0",
            &path,
        );
    }

    let fields = fields.clone();
    let mut normalized_update = update;
    if no_replace_active {
        let object = normalized_update
            .as_object_mut()
            .expect("validated JSON object");
        object.remove("track");
        object.remove("encodedTrack");
        object.remove("identifier");
    }
    if has_track_request && !no_replace_active {
        let (track, stream_url) = match resolve_player_track(&state, &fields).await {
            Ok(resolved) => resolved,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error, &path),
        };
        let audio_track = AudioTrack {
            identifier: track.info.identifier.clone(),
            title: track.info.title.clone(),
            author: track.info.author.clone(),
            duration_ms: u64::try_from(track.info.length).ok(),
            is_stream: track.info.is_stream,
        };
        let initial_position_ms = fields
            .get("position")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(track.info.position);
        let (pcm_sender, pcm_receiver) = tokio::sync::mpsc::channel(8);
        if let Err(error) = state
            .audio_players
            .load_streamed_track_at(
                &session_id,
                &guild_id,
                audio_track,
                initial_position_ms,
                pcm_receiver,
            )
            .await
        {
            return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
        }
        state
            .playback_sources
            .remove(&(session_id.clone(), guild_id.clone()))
            .await;
        let http_sources = state.http_sources.clone();
        tokio::spawn(async move {
            if let Err(error) = http_sources
                .stream_to_channel(
                    &stream_url,
                    4 * 1024 * 1024,
                    initial_position_ms,
                    pcm_sender,
                )
                .await
            {
                tracing::warn!(%error, "progressive HTTP audio stream stopped");
            }
        });
        if let Some(previous) = previous_encoded_track.as_ref() {
            publish_track_end(
                &state,
                &session_id,
                &guild_id,
                previous.clone(),
                TrackEndReason::Replaced,
            );
        }
        publish_track_start(&state, &session_id, &guild_id, track.encoded.clone());
        let object = normalized_update
            .as_object_mut()
            .expect("validated JSON object");
        object.remove("identifier");
        object.remove("encodedTrack");
        object.insert(
            "track".to_owned(),
            serde_json::to_value(track).expect("Track serializes to JSON"),
        );
    } else if !no_replace_active
        && (fields.get("track").is_some_and(serde_json::Value::is_null) || encoded_track_is_null)
    {
        if let Some(previous) = previous_encoded_track.as_ref() {
            publish_track_end(
                &state,
                &session_id,
                &guild_id,
                previous.clone(),
                TrackEndReason::Stopped,
            );
        }
        state.audio_players.stop(&session_id, &guild_id).await;
        state
            .playback_sources
            .remove(&(session_id.clone(), guild_id.clone()))
            .await;
        let object = normalized_update
            .as_object_mut()
            .expect("validated JSON object");
        if encoded_track_is_null {
            object.remove("encodedTrack");
            object.insert("track".to_owned(), serde_json::Value::Null);
        }
    }

    if fields.contains_key("endTime") {
        let end_time = fields.get("endTime").and_then(serde_json::Value::as_u64);
        if let Err(error) = state
            .audio_players
            .set_end_time(&session_id, &guild_id, end_time)
            .await
        {
            return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
        }
    }

    if let Some(volume) = fields.get("volume") {
        let Some(volume) = volume.as_u64().and_then(|value| u16::try_from(value).ok()) else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "Volume must be an integer from 0 to 1000",
                &path,
            );
        };
        if let Err(error) = state
            .audio_players
            .set_volume(&session_id, &guild_id, volume)
            .await
        {
            return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
        }
    }
    if let Some(filters) = fields.get("filters") {
        let filters = match serde_json::from_value::<ravalink_protocol::Filters>(filters.clone()) {
            Ok(filters) => filters,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string(), &path),
        };
        if matches!(
            filters.timescale,
            ravalink_protocol::Omissible::Present(Some(_))
        ) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "The timescale filter is not implemented by the native audio engine",
                &path,
            );
        }
        if !filters.plugin_filters.is_empty() {
            return api_error(
                StatusCode::BAD_REQUEST,
                "Plugin-specific filters are not supported by this node",
                &path,
            );
        }
        state
            .audio_players
            .set_filters(&session_id, &guild_id, filters)
            .await;
    }
    if let Some(paused) = fields.get("paused").and_then(serde_json::Value::as_bool)
        && state
            .audio_players
            .snapshot(&session_id, &guild_id)
            .await
            .is_some_and(|snapshot| snapshot.track.is_some())
        && let Err(error) = state
            .audio_players
            .pause(&session_id, &guild_id, paused)
            .await
    {
        return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
    }
    if let Some(position) = fields
        .get("position")
        .and_then(serde_json::Value::as_u64)
        .filter(|_| !has_track_request)
        && state
            .audio_players
            .snapshot(&session_id, &guild_id)
            .await
            .is_some_and(|snapshot| snapshot.track.is_some())
        && let Err(error) = seek_player_source(&state, &session_id, &guild_id, position).await
    {
        return api_error(StatusCode::BAD_REQUEST, &error, &path);
    }

    if let Some(voice_update) = voice_update {
        let previous = state
            .audio_players
            .voice_state(&session_id, &guild_id)
            .await;
        state
            .audio_players
            .set_voice_state(&session_id, &guild_id, voice_update.clone())
            .await;
        if previous != voice_update {
            shutdown_voice_task(&state, &session_id, &guild_id).await;
            if let Some(voice) = voice_update {
                let (shutdown, shutdown_rx) = watch::channel(false);
                state
                    .voice_tasks
                    .lock()
                    .await
                    .insert((session_id.clone(), guild_id.clone()), shutdown);
                let task_state = state.clone();
                let task_session_id = session_id.clone();
                let task_guild_id = guild_id.clone();
                tokio::spawn(async move {
                    run_voice_task(
                        task_state,
                        task_session_id,
                        task_guild_id,
                        voice,
                        shutdown_rx,
                    )
                    .await;
                });
            }
        }
    }

    Json(
        state
            .players
            .update(&session_id, &guild_id, normalized_update)
            .await,
    )
    .into_response()
}

async fn shutdown_voice_task(state: &AppState, session_id: &str, guild_id: &str) {
    if let Some(shutdown) = state
        .voice_tasks
        .lock()
        .await
        .remove(&(session_id.to_owned(), guild_id.to_owned()))
    {
        let _ = shutdown.send(true);
    }
}

async fn run_voice_task(
    state: AppState,
    session_id: String,
    guild_id: String,
    voice: DiscordVoiceState,
    mut shutdown: watch::Receiver<bool>,
) {
    let Some(session) = state.sessions.get(&session_id).await else {
        return;
    };
    let Ok(guild_id_number) = guild_id.parse::<u64>() else {
        warn!(%guild_id, "Cannot connect voice: invalid guild ID");
        return;
    };
    let Ok(channel_id) = voice.channel_id.parse::<u64>() else {
        warn!(channel_id = %voice.channel_id, "Cannot connect voice: invalid channel ID");
        return;
    };

    let mut retry_delay = Duration::from_secs(1);
    loop {
        if *shutdown.borrow() {
            break;
        }
        let connection_result = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
                continue;
            }
            result = tokio::time::timeout(
                Duration::from_secs(15),
                DiscordVoiceConnection::connect(
                    guild_id_number,
                    session.user_id,
                    channel_id,
                    &voice.session_id,
                    &voice.token,
                    &voice.endpoint,
                ),
            ) => result,
        };
        let mut connection = match connection_result {
            Ok(Ok(connection)) => {
                retry_delay = Duration::from_secs(1);
                connection
            }
            Ok(Err(error)) => {
                warn!(%error, %guild_id, ?retry_delay, "Discord voice connection failed; retrying");
                if wait_for_voice_retry(&mut shutdown, retry_delay).await {
                    break;
                }
                retry_delay = retry_delay.saturating_mul(2).min(Duration::from_secs(30));
                continue;
            }
            Err(_) => {
                warn!(%guild_id, "Discord voice handshake timed out; retrying");
                if wait_for_voice_retry(&mut shutdown, retry_delay).await {
                    break;
                }
                retry_delay = retry_delay.saturating_mul(2).min(Duration::from_secs(30));
                continue;
            }
        };

        let should_retry = run_voice_audio_loop(
            &state,
            &session_id,
            &guild_id,
            &mut connection,
            &mut shutdown,
        )
        .await;
        drop(connection);
        state
            .players
            .set_runtime_state(&session_id, &guild_id, 0, false, -1)
            .await;
        if !should_retry || wait_for_voice_retry(&mut shutdown, retry_delay).await {
            break;
        }
        retry_delay = retry_delay.saturating_mul(2).min(Duration::from_secs(30));
    }
}

async fn run_voice_audio_loop(
    state: &AppState,
    session_id: &str,
    guild_id: &str,
    connection: &mut DiscordVoiceConnection,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    let mut interval = tokio::time::interval(Duration::from_millis(20));
    interval.tick().await;
    let mut speaking = false;
    let mut last_position_ms = None;
    let mut stalled_since: Option<Instant> = None;
    let mut stuck_reported = false;
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = interval.tick() => {
                if !connection.is_connected() {
                    warn!(%guild_id, "Discord voice gateway disconnected");
                    break;
                }
                let snapshot = state.audio_players.snapshot(session_id, guild_id).await;
                let position_ms = snapshot.as_ref().map_or(0, |player| player.position_ms);
                state
                    .players
                    .set_runtime_state(session_id, guild_id, position_ms, true, -1)
                    .await;
                let mut should_speak = snapshot
                    .as_ref()
                    .is_some_and(|player| player.track.is_some() && !player.paused);
                if should_speak {
                    should_speak = connection.dave.lock().await.can_send_audio();
                }
                if should_speak != speaking {
                    if speaking
                        && !should_speak
                        && let Err(error) = send_voice_silence(connection).await
                    {
                        warn!(%error, %guild_id, "Failed to send Discord voice silence frames");
                    }
                    if let Err(error) = connection.set_speaking(should_speak) {
                        warn!(%error, %guild_id, "Failed to update Discord speaking state");
                        break;
                    }
                    speaking = should_speak;
                }
                if !should_speak {
                    last_position_ms = None;
                    stalled_since = None;
                    stuck_reported = false;
                }
                if should_speak {
                    if let Some(player) = snapshot.as_ref() {
                        if last_position_ms == Some(player.position_ms) {
                            let stalled = stalled_since.get_or_insert_with(Instant::now);
                            if !stuck_reported
                                && stalled.elapsed() >= Duration::from_secs(10)
                                && let Some(encoded) = state
                                    .players
                                    .get(session_id, guild_id)
                                    .await
                                    .and_then(|player| player["track"]["encoded"].as_str().map(str::to_owned))
                            {
                                publish_track_stuck(state, session_id, guild_id, encoded, 10_000);
                                stuck_reported = true;
                            }
                        } else {
                            last_position_ms = Some(player.position_ms);
                            stalled_since = Some(Instant::now());
                            stuck_reported = false;
                        }
                    }
                    let packet = {
                        let mut dave = connection.dave.lock().await;
                        state
                            .audio_players
                            .next_dave_encrypted_voice_packet(
                                session_id,
                                guild_id,
                                &mut connection.pipeline,
                                dave.session_mut(),
                            )
                            .await
                    };
                    match packet {
                        Ok(Some(packet)) => {
                            if let Err(error) = connection.transport.send_voice_packet(&packet).await {
                                warn!(%error, %guild_id, "Failed to send Discord voice packet");
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let (cause, log_message) = if error.message.starts_with("audio decode failed:") {
                                ("AudioDecodeError", "Failed to decode audio track")
                            } else {
                                ("AudioEncodingError", "Failed to encode Discord voice packet")
                            };
                            warn!(%error, %guild_id, "{log_message}");
                            if let Some(encoded) = state
                                .players
                                .get(session_id, guild_id)
                                .await
                                .and_then(|player| player["track"]["encoded"].as_str().map(str::to_owned))
                            {
                                publish_track_exception(
                                    state,
                                    session_id,
                                    guild_id,
                                    encoded,
                                    &error.message,
                                    cause,
                                );
                            }
                            state.audio_players.stop(session_id, guild_id).await;
                            state.playback_sources.remove(&(
                                session_id.to_owned(),
                                guild_id.to_owned(),
                            )).await;
                            state
                                .players
                                .update(session_id, guild_id, serde_json::json!({
                                    "track": null,
                                    "paused": false
                                }))
                                .await;
                        }
                    }

                    if state
                        .audio_players
                        .take_completed_track(session_id, guild_id)
                        .await
                        .is_some()
                    {
                        state.playback_sources.remove(&(
                            session_id.to_owned(),
                            guild_id.to_owned(),
                        )).await;
                        if let Some(encoded) = state
                            .players
                            .get(session_id, guild_id)
                            .await
                            .and_then(|player| player["track"]["encoded"].as_str().map(str::to_owned))
                        {
                            publish_track_end(
                                state,
                                session_id,
                                guild_id,
                                encoded,
                                TrackEndReason::Finished,
                            );
                        }
                        state
                            .players
                            .update(session_id, guild_id, serde_json::json!({
                                "track": null,
                                "paused": false
                            }))
                            .await;
                    }
                }
            }
        }
    }
    if speaking {
        let _ = send_voice_silence(connection).await;
        let _ = connection.set_speaking(false);
    }
    !*shutdown.borrow()
}

async fn send_voice_silence(connection: &mut DiscordVoiceConnection) -> Result<(), AudioError> {
    const SILENCE_FRAME: [u8; 3] = [0xF8, 0xFF, 0xFE];
    for _ in 0..5 {
        let packet = {
            let mut dave = connection.dave.lock().await;
            connection
                .pipeline
                .packetize_opus_with_dave(&SILENCE_FRAME, dave.session_mut())?
        };
        connection.transport.send_voice_packet(&packet).await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

fn publish_track_start(state: &AppState, session_id: &str, guild_id: &str, encoded_track: String) {
    let track = match decode_track(&encoded_track) {
        Ok(track) => track,
        Err(error) => {
            warn!(%error, %guild_id, "cannot publish TrackStartEvent with an invalid encoded track");
            return;
        }
    };
    let _ = state.events.send((
        session_id.to_owned(),
        WebSocketMessage::Event(Box::new(TrackEvent::TrackStart(TrackEventBase {
            guild_id: guild_id.to_owned(),
            track,
        }))),
    ));
}

fn publish_track_end(
    state: &AppState,
    session_id: &str,
    guild_id: &str,
    encoded_track: String,
    reason: TrackEndReason,
) {
    let track = match decode_track(&encoded_track) {
        Ok(track) => track,
        Err(error) => {
            warn!(%error, %guild_id, "cannot publish TrackEndEvent with an invalid encoded track");
            return;
        }
    };
    let _ = state.events.send((
        session_id.to_owned(),
        WebSocketMessage::Event(Box::new(TrackEvent::TrackEnd(TrackEndEvent {
            guild_id: guild_id.to_owned(),
            track,
            reason,
        }))),
    ));
}

fn publish_track_exception(
    state: &AppState,
    session_id: &str,
    guild_id: &str,
    encoded_track: String,
    message: &str,
    cause: &str,
) {
    let track = match decode_track(&encoded_track) {
        Ok(track) => track,
        Err(error) => {
            warn!(%error, %guild_id, "cannot publish TrackExceptionEvent with an invalid encoded track");
            return;
        }
    };
    let _ = state.events.send((
        session_id.to_owned(),
        WebSocketMessage::Event(Box::new(TrackEvent::TrackException(TrackExceptionEvent {
            guild_id: guild_id.to_owned(),
            track,
            exception: TrackException {
                message: Some(message.to_owned()),
                severity: ExceptionSeverity::Fault,
                cause: cause.to_owned(),
            },
        }))),
    ));
}

fn publish_track_stuck(
    state: &AppState,
    session_id: &str,
    guild_id: &str,
    encoded_track: String,
    threshold_ms: u64,
) {
    let track = match decode_track(&encoded_track) {
        Ok(track) => track,
        Err(error) => {
            warn!(%error, %guild_id, "cannot publish TrackStuckEvent with an invalid encoded track");
            return;
        }
    };
    let _ = state.events.send((
        session_id.to_owned(),
        WebSocketMessage::Event(Box::new(TrackEvent::TrackStuck(TrackStuckEvent {
            guild_id: guild_id.to_owned(),
            track,
            threshold_ms,
        }))),
    ));
}

async fn wait_for_voice_retry(shutdown: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => *shutdown.borrow(),
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
    }
}

async fn seek_player_source(
    state: &AppState,
    session_id: &str,
    guild_id: &str,
    position_ms: u64,
) -> Result<(), String> {
    let snapshot = state
        .audio_players
        .snapshot(session_id, guild_id)
        .await
        .ok_or_else(|| "player does not exist".to_owned())?;
    let track = snapshot
        .track
        .ok_or_else(|| "cannot seek a player without an active track".to_owned())?;
    if track.is_stream {
        return Err("live streams are not seekable".to_owned());
    }
    if track
        .duration_ms
        .is_some_and(|duration| position_ms > duration)
    {
        return Err("seek position exceeds track duration".to_owned());
    }
    let key = (session_id.to_owned(), guild_id.to_owned());
    let source = state.playback_sources.get(&key).await;
    let Some(source) = source else {
        return state
            .audio_players
            .seek(session_id, guild_id, position_ms)
            .await
            .map_err(|error| error.message);
    };
    let (sender, receiver) = tokio::sync::mpsc::channel(8);
    state
        .audio_players
        .restart_streamed_source(session_id, guild_id, position_ms, receiver)
        .await
        .map_err(|error| error.message)?;
    tokio::spawn(async move {
        let _ = decode_bytes_to_channel_at(
            source.bytes,
            source.extension,
            source.max_pcm_bytes,
            position_ms,
            sender,
        )
        .await;
    });
    Ok(())
}

async fn resolve_player_track(
    state: &AppState,
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Result<(Track, String), String> {
    if let Some(identifier) = fields.get("identifier").and_then(serde_json::Value::as_str) {
        if state.plugins.find(identifier).is_some() {
            return resolve_plugin_audio_track(state, identifier).await;
        }
        let resolved = state
            .http_sources
            .resolve_stream_track(identifier)
            .await
            .map_err(|error| error.message)?;
        return Ok((resolved.track, identifier.to_owned()));
    }

    let encoded = fields
        .get("encodedTrack")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            fields
                .get("track")
                .and_then(|track| track.get("encoded"))
                .and_then(serde_json::Value::as_str)
        })
        .ok_or_else(|| "track update must contain an encoded track".to_owned())?;
    let mut track = decode_track(encoded).map_err(|error| error.message)?;
    if let Some(user_data) = fields.get("track").and_then(|value| value.get("userData")) {
        track.user_data = user_data.clone();
    }

    if matches!(
        track.info.source_name.as_str(),
        "youtube" | "spotify" | "applemusic" | "deezer"
    ) {
        let position = track.info.position;
        let uri = track
            .info
            .uri
            .clone()
            .ok_or_else(|| "plugin track has no source URI".to_owned())?;
        let (mut plugin_track, stream_url) = resolve_plugin_audio_track(state, &uri).await?;
        plugin_track.info.position = position;
        plugin_track.encoded = encode_track(&plugin_track).map_err(|error| error.message)?;
        return Ok((plugin_track, stream_url));
    }
    if track.info.source_name != "http" {
        return Err(format!(
            "source manager '{}' is not available on this node",
            track.info.source_name
        ));
    }
    let uri = track
        .info
        .uri
        .clone()
        .ok_or_else(|| "encoded HTTP track has no source URI".to_owned())?;
    let resolved = state
        .http_sources
        .resolve_stream_track(&uri)
        .await
        .map_err(|error| error.message)?;
    let user_data = track.user_data;
    track = resolved.track;
    track.user_data = user_data;
    track.encoded = encode_track(&track).map_err(|error| error.message)?;
    Ok((track, uri))
}

async fn resolve_plugin_audio_track(
    state: &AppState,
    identifier: &str,
) -> Result<(Track, String), String> {
    let result = state
        .plugins
        .load_tracks(identifier)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "no native plugin can resolve this identifier".to_owned())?;
    if result.load_type != ravalink_plugin_api::PluginLoadType::Track {
        return Err("a playable track identifier is required".to_owned());
    }
    let mut track: Track = serde_json::from_value(result.data)
        .map_err(|error| format!("plugin returned an invalid track: {error}"))?;
    let stream_url = track
        .plugin_info
        .get("streamUrl")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "native plugin returned no playable audio URL".to_owned())?
        .to_owned();
    track.info.is_stream = true;
    track.info.is_seekable = false;
    track.encoded = encode_track(&track).map_err(|error| error.message)?;
    Ok((track, stream_url))
}

async fn decode_track_endpoint(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<DecodeTrackQuery>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    let Some(encoded) = query.encoded_track.or(query.track) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "No track to decode provided",
            "/v4/decodetrack",
        );
    };
    match decode_track(&encoded) {
        Ok(track) => Json(track).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error.message, "/v4/decodetrack"),
    }
}

async fn decode_tracks_endpoint(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(encoded): Json<EncodedTracks>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    if encoded.0.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "No tracks to decode provided",
            "/v4/decodetracks",
        );
    }
    let mut tracks = Vec::with_capacity(encoded.0.len());
    for value in encoded.0 {
        match decode_track(&value) {
            Ok(track) => tracks.push(track),
            Err(error) => {
                return api_error(StatusCode::BAD_REQUEST, &error.message, "/v4/decodetracks");
            }
        }
    }
    Json(ravalink_protocol::Tracks(tracks)).into_response()
}

async fn websocket(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }

    let user_id = match headers
        .get("user-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(value) => value,
        None => {
            return (StatusCode::BAD_REQUEST, "Missing or invalid User-Id header").into_response();
        }
    };
    let client_name = headers
        .get("client-name")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let requested_session = headers
        .get("session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    ws.on_upgrade(move |socket| {
        handle_socket(socket, state, user_id, client_name, requested_session)
    })
}

async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    user_id: u64,
    client_name: Option<String>,
    requested_session: Option<String>,
) {
    let resumed_session = match requested_session {
        Some(id) => state.sessions.resume(&id, user_id).await,
        None => None,
    };
    let (session_id, resumed) = if let Some(session) = resumed_session {
        (session.id, true)
    } else {
        let session = state.sessions.create(user_id, client_name).await;
        (session.id, false)
    };

    let mut event_rx = state.events.subscribe();
    let ready = serde_json::json!({
        "op": "ready",
        "resumed": resumed,
        "sessionId": session_id,
    });
    if socket
        .send(Message::Text(ready.to_string().into()))
        .await
        .is_err()
    {
        state.sessions.disconnect(&session_id).await;
        if state.sessions.get(&session_id).await.is_none() {
            cleanup_session_resources(
                &state.players,
                &state.audio_players,
                &state.playback_sources,
                &state.voice_tasks,
                &session_id,
            )
            .await;
        }
        return;
    }

    let mut stats_interval = tokio::time::interval(Duration::from_secs(60));
    stats_interval.tick().await;
    let mut player_update_interval =
        tokio::time::interval(Duration::from_secs(state.player_update_interval));
    player_update_interval.tick().await;
    'socket: loop {
        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Ok((event_session, message)) if event_session == session_id => {
                        let payload = serde_json::to_string(&message)
                            .expect("WebSocket event serializes to JSON");
                        if socket.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(Message::Text(_))) => {
                        warn!("Ignoring client WebSocket text message; Lavalink v4 commands use REST");
                    }
                    _ => {}
                }
            }
            _ = stats_interval.tick() => {
                let node_stats = collect_stats(&state).await;
                let message = WebSocketMessage::Stats(StatsMessage {
                    players: node_stats.players,
                    playing_players: node_stats.playing_players,
                    uptime: node_stats.uptime,
                    memory: node_stats.memory,
                    cpu: node_stats.cpu,
                    frame_stats: node_stats.frame_stats,
                });
                let payload = serde_json::to_string(&message)
                    .expect("WebSocket stats message serializes to JSON");
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    break;
                }
            }
            _ = player_update_interval.tick() => {
                let players = state.players.list(&session_id).await;
                let time = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                for player in players {
                    let Some(guild_id) = player["guildId"].as_str() else {
                        continue;
                    };
                    let snapshot = state.audio_players.snapshot(&session_id, guild_id).await;
                    let runtime = state.players.get(&session_id, guild_id).await;
                    let connected = runtime
                        .as_ref()
                        .and_then(|player| player["state"]["connected"].as_bool())
                        .unwrap_or(false);
                    let ping = runtime
                        .as_ref()
                        .and_then(|player| player["state"]["ping"].as_i64())
                        .unwrap_or(-1);
                    let message = WebSocketMessage::PlayerUpdate(PlayerUpdateMessage {
                        guild_id: guild_id.to_owned(),
                        state: PlayerStateUpdate {
                            time,
                            position: snapshot.as_ref().map_or(0, |player| player.position_ms),
                            connected,
                            ping: Some(ping),
                        },
                    });
                    let payload = serde_json::to_string(&message)
                        .expect("WebSocket player update serializes to JSON");
                    if socket.send(Message::Text(payload.into())).await.is_err() {
                        break 'socket;
                    }
                }
            }
        }
    }

    state.sessions.disconnect(&session_id).await;
    if state.sessions.get(&session_id).await.is_none() {
        cleanup_session_resources(
            &state.players,
            &state.audio_players,
            &state.playback_sources,
            &state.voice_tasks,
            &session_id,
        )
        .await;
    }
    info!(%session_id, "WebSocket disconnected");
}

async fn cleanup_session_resources(
    players: &PlayerStore,
    audio_players: &AudioPlayerStore,
    playback_sources: &PlaybackSourceStore,
    voice_tasks: &VoiceTaskMap,
    session_id: &str,
) {
    players.remove_session(session_id).await;
    audio_players.remove_session(session_id).await;
    playback_sources.remove_session(session_id).await;
    let mut tasks = voice_tasks.lock().await;
    let keys = tasks
        .keys()
        .filter(|(session, _)| session == session_id)
        .cloned()
        .collect::<Vec<_>>();
    for key in keys {
        if let Some(shutdown) = tasks.remove(&key) {
            let _ = shutdown.send(true);
        }
    }
}

fn api_error(status: StatusCode, message: &str, path: &str) -> Response {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    (
        status,
        Json(ApiError {
            timestamp,
            status: status.as_u16(),
            error: status.canonical_reason().unwrap_or("Error").to_owned(),
            message: message.to_owned(),
            path: path.to_owned(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod server_api_tests {
    use super::*;
    use axum::{body::to_bytes, http::HeaderValue};

    fn state() -> AppState {
        AppState {
            password: "test-password".into(),
            sessions: SessionStore::default(),
            players: PlayerStore::default(),
            audio_players: AudioPlayerStore::default(),
            voice_tasks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            http_sources: HttpSourceManager::default(),
            playback_sources: PlaybackSourceStore::new(256 * 1024 * 1024),
            plugins: built_in_plugins(&[], std::path::Path::new("plugins")).unwrap(),
            metrics_enabled: false,
            player_update_interval: 5,
            system: Arc::new(tokio::sync::Mutex::new(System::new_all())),
            stats_cache: Arc::new(tokio::sync::Mutex::new(None)),
            started_at: Instant::now(),
            events: tokio::sync::broadcast::channel(64).0,
        }
    }

    fn authorized_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("test-password"));
        headers
    }

    #[test]
    fn http_source_scheme_detection_is_case_insensitive_and_exact() {
        for identifier in [
            "http://cdn.example/audio.mp3",
            "https://cdn.example/audio.mp3",
            "HTTP://cdn.example/audio.mp3",
            "HtTpS://cdn.example/audio.mp3",
        ] {
            assert!(
                has_http_scheme(identifier),
                "expected HTTP scheme: {identifier}"
            );
        }
        for identifier in [
            "ftp://cdn.example/audio.mp3",
            "httpx://cdn.example/audio.mp3",
            "httpsomething://cdn.example/audio.mp3",
            "youtube:track-id",
            "not a URL",
        ] {
            assert!(
                !has_http_scheme(identifier),
                "unexpected HTTP scheme: {identifier}"
            );
        }
    }

    #[tokio::test]
    async fn version_endpoint_enforces_authentication_and_reports_native_version() {
        let unauthorized = version(HeaderMap::new(), State(state())).await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let mut invalid_headers = HeaderMap::new();
        invalid_headers.insert("authorization", HeaderValue::from_static("wrong-password"));
        let forbidden = version(invalid_headers, State(state())).await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        let response = version(authorized_headers(), State(state())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"4.0.0-ravalink");
    }

    #[tokio::test]
    async fn info_endpoint_advertises_builtin_sources_and_plugin_metadata() {
        let response = info_endpoint(authorized_headers(), State(state())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let info: serde_json::Value = serde_json::from_slice(&body).unwrap();

        for source in ["youtube", "spotify", "applemusic", "deezer"] {
            assert!(
                info["sourceManagers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value.as_str() == Some(source)),
                "missing source manager {source}"
            );
        }
        assert!(info["plugins"].is_array());
    }

    #[tokio::test]
    async fn update_player_rejects_filters_without_native_implementations() {
        let state = state();
        let session = state.sessions.create(123, None).await;
        for filters in [
            serde_json::json!({"timescale": {"speed": 1.2}}),
            serde_json::json!({"communityFilter": {"amount": 0.5}}),
        ] {
            let response = update_player(
                authorized_headers(),
                State(state.clone()),
                Path((session.id.clone(), "guild".into())),
                Query(PlayerUpdateQuery::default()),
                Json(serde_json::json!({"filters": filters})),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn missing_player_uses_lavalink_api_error_shape() {
        let response = get_player(
            authorized_headers(),
            State(state()),
            Path(("missing-session".into(), "guild".into())),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(error["status"], StatusCode::NOT_FOUND.as_u16());
        assert_eq!(error["error"], "Not Found");
        assert_eq!(error["message"], "Session not found");
        assert_eq!(error["path"], "/v4/sessions/missing-session/players/guild");
        assert!(error["timestamp"].as_u64().is_some());
    }

    #[tokio::test]
    async fn playback_source_store_enforces_aggregate_memory_limit() {
        let store = PlaybackSourceStore::new(16);
        let source = |size: usize| CachedHttpSource {
            bytes: vec![0; size].into(),
            extension: Some("wav".into()),
            max_pcm_bytes: 64,
        };
        let first = ("session".to_owned(), "guild-a".to_owned());
        let second = ("session".to_owned(), "guild-b".to_owned());

        assert!(
            store
                .replace(first.clone(), source(8))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(store.bytes_used().await, 8);
        assert!(store.replace(second.clone(), source(9)).await.is_err());
        assert_eq!(store.bytes_used().await, 8);
        let replacement = source(12);
        let previous = store
            .replace(first.clone(), replacement.clone())
            .await
            .unwrap();
        assert_eq!(store.bytes_used().await, 12);
        store.restore(first.clone(), &replacement, previous).await;
        assert_eq!(store.bytes_used().await, 8);
        assert!(store.replace(second, source(9)).await.is_err());
        store.remove(&first).await;
        assert_eq!(store.bytes_used().await, 0);
    }

    #[test]
    fn player_update_validation_rejects_invalid_field_types_and_ranges() {
        let valid = serde_json::json!({
            "identifier": "https://cdn.example/audio.mp3",
            "encodedTrack": null,
            "track": null,
            "paused": false,
            "position": 0,
            "volume": 1000,
            "filters": {}
        });
        assert_eq!(invalid_player_field(valid.as_object().unwrap()), None);

        for (field, value) in [
            ("identifier", serde_json::json!(42)),
            ("encodedTrack", serde_json::json!(false)),
            ("track", serde_json::json!("not-an-object")),
            ("paused", serde_json::json!("true")),
            ("position", serde_json::json!(-1)),
            ("volume", serde_json::json!(1001)),
            ("filters", serde_json::json!([])),
        ] {
            let invalid =
                serde_json::Value::Object([(field.to_owned(), value)].into_iter().collect());
            assert_eq!(
                invalid_player_field(invalid.as_object().unwrap()),
                Some(field)
            );
        }
    }

    #[tokio::test]
    async fn session_cleanup_stops_and_removes_all_voice_tasks() {
        let state = state();
        let (shutdown, mut receiver) = watch::channel(false);
        state
            .voice_tasks
            .lock()
            .await
            .insert(("session".into(), "guild".into()), shutdown);
        state
            .players
            .update("session", "guild", serde_json::json!({}))
            .await;
        assert!(
            state
                .playback_sources
                .replace(
                    ("session".into(), "guild".into()),
                    CachedHttpSource {
                        bytes: Arc::from(vec![1, 2, 3]),
                        extension: Some("wav".into()),
                        max_pcm_bytes: 64,
                    },
                )
                .await
                .unwrap()
                .is_none()
        );
        state
            .audio_players
            .set_voice_state(
                "session",
                "guild",
                Some(DiscordVoiceState {
                    token: "token".into(),
                    endpoint: "voice.example".into(),
                    session_id: "discord-session".into(),
                    channel_id: "channel".into(),
                }),
            )
            .await;

        cleanup_session_resources(
            &state.players,
            &state.audio_players,
            &state.playback_sources,
            &state.voice_tasks,
            "session",
        )
        .await;

        assert!(receiver.changed().await.is_ok());
        assert!(*receiver.borrow());
        assert!(state.players.is_empty().await);
        assert!(state.audio_players.is_empty().await);
        assert_eq!(state.playback_sources.bytes_used().await, 0);
        assert!(
            state
                .audio_players
                .voice_state("session", "guild")
                .await
                .is_none()
        );
        assert!(state.voice_tasks.lock().await.is_empty());
    }

    #[test]
    fn track_start_and_end_events_include_full_lavalink_track_objects() {
        let state = state();
        let mut receiver = state.events.subscribe();
        let track = Track {
            encoded: String::new(),
            info: ravalink_protocol::TrackInfo {
                identifier: "test-id".into(),
                is_seekable: true,
                author: "test artist".into(),
                length: 1_000,
                is_stream: false,
                position: 0,
                title: "test track".into(),
                uri: Some("https://www.youtube.com/watch?v=test-id".into()),
                source_name: "youtube".into(),
                artwork_url: None,
                isrc: None,
            },
            plugin_info: serde_json::json!({}),
            user_data: serde_json::json!({}),
        };
        let encoded = encode_track(&track).unwrap();
        publish_track_start(&state, "session", "guild", encoded.clone());
        let (_, started) = receiver.try_recv().unwrap();
        let started = serde_json::to_value(started).unwrap();
        assert_eq!(started["op"], "event");
        assert_eq!(started["type"], "TrackStartEvent");
        assert_eq!(started["guildId"], "guild");
        assert_eq!(started["track"]["encoded"], encoded);
        assert_eq!(started["track"]["info"]["title"], "test track");

        publish_track_end(
            &state,
            "session",
            "guild",
            encoded.clone(),
            TrackEndReason::Finished,
        );
        let (_, ended) = receiver.try_recv().unwrap();
        let ended = serde_json::to_value(ended).unwrap();
        assert_eq!(ended["type"], "TrackEndEvent");
        assert_eq!(ended["track"]["encoded"], encoded);
        assert_eq!(ended["reason"], "finished");

        publish_track_stuck(&state, "session", "guild", encoded.clone(), 10_000);
        let (_, stuck) = receiver.try_recv().unwrap();
        let stuck = serde_json::to_value(stuck).unwrap();
        assert_eq!(stuck["type"], "TrackStuckEvent");
        assert_eq!(stuck["track"]["encoded"], encoded);
        assert_eq!(stuck["thresholdMs"], 10_000);
    }

    #[tokio::test]
    async fn routeplanner_status_returns_no_content_when_disabled() {
        let response = routeplanner_status(authorized_headers(), State(state())).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn routeplanner_free_operations_report_disabled_planner() {
        let address = routeplanner_free_address(
            authorized_headers(),
            State(state()),
            Json(RoutePlannerFreeAddress {
                _address: "203.0.113.1".into(),
            }),
        )
        .await;
        let all = routeplanner_free_all(authorized_headers(), State(state())).await;
        assert_eq!(address.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(all.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
