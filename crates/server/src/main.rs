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
use ravalink_audio::{AudioTrack, HttpSourceManager, PcmFrame};
use ravalink_core::{AudioPlayerStore, DiscordVoiceState, PlayerStore, SessionStore};
use ravalink_protocol::{
    ApiError, CpuStats, EncodedTracks, LoadResult, MemoryStats, NodeStats, ServerInfo, SessionInfo,
    SessionUpdate, Track, decode_track,
};
use serde::Deserialize;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::System;
use tokio::net::TcpListener;
use tracing::{info, warn};

#[derive(Clone)]
struct AppState {
    password: String,
    sessions: SessionStore,
    players: PlayerStore,
    audio_players: AudioPlayerStore,
    http_sources: HttpSourceManager,
    metrics_enabled: bool,
    started_at: Instant,
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

    let state = AppState {
        password,
        sessions: SessionStore::default(),
        players: PlayerStore::default(),
        audio_players: AudioPlayerStore::new(config.audio.opus_encoding_quality),
        http_sources: HttpSourceManager::with_limits(
            config.audio.max_source_download_bytes,
            config.audio.max_decoded_cache_bytes,
        ),
        metrics_enabled: config.metrics.prometheus_enabled,
        started_at: Instant::now(),
    };

    let cleanup_sessions = state.sessions.clone();
    let cleanup_players = state.players.clone();
    let cleanup_audio_players = state.audio_players.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            for session_id in cleanup_sessions.prune_expired().await {
                cleanup_players.remove_session(&session_id).await;
                cleanup_audio_players.remove_session(&session_id).await;
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
    let uptime_seconds = state.started_at.elapsed().as_secs_f64();

    let body = format!(
        "# HELP ravalink_uptime_seconds Time since this RavaLink process started.\n# TYPE ravalink_uptime_seconds gauge\nravalink_uptime_seconds {uptime_seconds:.3}\n# HELP ravalink_sessions Number of active Lavalink sessions.\n# TYPE ravalink_sessions gauge\nravalink_sessions {sessions}\n# HELP ravalink_players Number of player states across sessions.\n# TYPE ravalink_players gauge\nravalink_players {players}\n# HELP ravalink_playing_players Number of players with a non-paused track.\n# TYPE ravalink_playing_players gauge\nravalink_playing_players {playing_players}\n# HELP ravalink_decoded_cache_bytes Decoded PCM bytes currently held by the HTTP source cache.\n# TYPE ravalink_decoded_cache_bytes gauge\nravalink_decoded_cache_bytes {cache_bytes}\n"
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
    Json(ServerInfo::default()).into_response()
}

async fn stats(headers: HeaderMap, State(state): State<AppState>) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }

    let mut system = System::new_all();
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

    let value = NodeStats {
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
    Json(value).into_response()
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
    if !query.identifier.starts_with("http://") && !query.identifier.starts_with("https://") {
        return Json(LoadResult::empty()).into_response();
    }

    match state.http_sources.resolve(&query.identifier).await {
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
            let (Some(token), Some(endpoint), Some(session_id)) = (
                voice.get("token").and_then(serde_json::Value::as_str),
                voice.get("endpoint").and_then(serde_json::Value::as_str),
                voice.get("sessionId").and_then(serde_json::Value::as_str),
            ) else {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "Voice state requires token, endpoint and sessionId strings",
                    &path,
                );
            };
            if token.is_empty() || endpoint.is_empty() || session_id.is_empty() {
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
        let (track, frames) = match resolve_player_track(&state, &fields).await {
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
        if let Err(error) = state
            .audio_players
            .load_decoded_track(&session_id, &guild_id, audio_track, frames)
            .await
        {
            return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
        }
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
        state.audio_players.stop(&session_id, &guild_id).await;
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
    if let Some(position) = fields.get("position").and_then(serde_json::Value::as_u64)
        && state
            .audio_players
            .snapshot(&session_id, &guild_id)
            .await
            .is_some_and(|snapshot| snapshot.track.is_some())
        && let Err(error) = state
            .audio_players
            .seek(&session_id, &guild_id, position)
            .await
    {
        return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
    }

    if let Some(voice_update) = voice_update {
        state
            .audio_players
            .set_voice_state(&session_id, &guild_id, voice_update)
            .await;
    }

    Json(
        state
            .players
            .update(&session_id, &guild_id, normalized_update)
            .await,
    )
    .into_response()
}

async fn resolve_player_track(
    state: &AppState,
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Result<(Track, Vec<PcmFrame>), String> {
    if let Some(identifier) = fields.get("identifier").and_then(serde_json::Value::as_str) {
        let resolved = state
            .http_sources
            .resolve(identifier)
            .await
            .map_err(|error| error.message)?;
        let frames = state
            .http_sources
            .take_frames(&resolved.track.encoded)
            .map_err(|error| error.message)?
            .ok_or_else(|| "decoded HTTP audio is no longer cached".to_owned())?;
        return Ok((resolved.track, frames));
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

    let frames = state
        .http_sources
        .take_frames(&track.encoded)
        .map_err(|error| error.message)?;
    if let Some(frames) = frames {
        return Ok((track, frames));
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
        .resolve(&uri)
        .await
        .map_err(|error| error.message)?;
    let frames = state
        .http_sources
        .take_frames(&resolved.track.encoded)
        .map_err(|error| error.message)?
        .ok_or_else(|| "decoded HTTP audio is no longer cached".to_owned())?;
    track = resolved.track;
    Ok((track, frames))
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
        state.sessions.remove(&session_id).await;
        state.players.remove_session(&session_id).await;
        state.audio_players.remove_session(&session_id).await;
        return;
    }

    while let Some(message) = socket.recv().await {
        match message {
            Ok(Message::Ping(payload)) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(Message::Text(_)) => {
                warn!("Ignoring client WebSocket text message; Lavalink v4 commands use REST");
            }
            _ => {}
        }
    }

    state.sessions.disconnect(&session_id).await;
    if state.sessions.get(&session_id).await.is_none() {
        state.players.remove_session(&session_id).await;
        state.audio_players.remove_session(&session_id).await;
    }
    info!(%session_id, "WebSocket disconnected");
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
    use axum::http::HeaderValue;

    fn state() -> AppState {
        AppState {
            password: "test-password".into(),
            sessions: SessionStore::default(),
            players: PlayerStore::default(),
            audio_players: AudioPlayerStore::default(),
            http_sources: HttpSourceManager::default(),
            metrics_enabled: false,
            started_at: Instant::now(),
        }
    }

    fn authorized_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("test-password"));
        headers
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
