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
use ravalink_core::{AudioPlayerStore, PlayerStore, SessionStore};
use ravalink_protocol::{
    ApiError, CpuStats, EncodedTracks, LoadResult, MemoryStats, NodeStats, ServerInfo, SessionInfo,
    SessionUpdate, decode_track,
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

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PlayerUpdateQuery {
    #[serde(default)]
    no_replace: bool,
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
        audio_players: AudioPlayerStore::default(),
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
        .route("/v4/websocket", get(websocket))
        .with_state(state);

    let socket = format!("{address}:{port}");
    let listener = TcpListener::bind(&socket)
        .await
        .expect("failed to bind listener");
    info!(%socket, "RavaLink native server starting");
    axum::serve(listener, app).await.expect("server failed");
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

async fn load_tracks(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<LoadTracksQuery>,
) -> Response {
    if let Some(response) = authorization_failure(&headers, &state) {
        return response;
    }
    // The route and response contract are present from the first milestone.
    // Real source resolution is added with native source adapters; do not invent
    // a track or claim that an unresolved identifier has been loaded.
    let _identifier = query.identifier;
    Json(LoadResult::empty()).into_response()
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

    if query.no_replace
        && has_track_request
        && let Some(player) = state.players.get(&session_id, &guild_id).await
        && !player["track"].is_null()
    {
        return Json(player).into_response();
    }

    if has_track_request || fields.contains_key("voice") {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "Track resolution and Discord voice transport are not implemented yet",
            &path,
        );
    }
    if let Some(end_time) = fields.get("endTime").filter(|value| !value.is_null())
        && end_time.as_u64().is_none_or(|value| value == 0)
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "End time must be greater than 0",
            &path,
        );
    }

    let mut normalized_update = update;
    if encoded_track_is_null {
        let object = normalized_update
            .as_object_mut()
            .expect("validated JSON object");
        object.remove("encodedTrack");
        object.insert("track".to_owned(), serde_json::Value::Null);
    }
    if fields.get("track").is_some_and(serde_json::Value::is_null) || encoded_track_is_null {
        state.audio_players.stop(&session_id, &guild_id).await;
    }
    if let Some(volume) = fields.get("volume") {
        let Some(volume) = volume.as_u64().and_then(|value| u16::try_from(value).ok()) else {
            return api_error(StatusCode::BAD_REQUEST, "Volume must be an integer from 0 to 1000", &path);
        };
        if let Err(error) = state.audio_players.set_volume(&session_id, &guild_id, volume).await {
            return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
        }
    }
    if let Some(filters) = fields.get("filters") {
        let filters = match serde_json::from_value::<ravalink_protocol::Filters>(filters.clone()) {
            Ok(filters) => filters,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string(), &path),
        };
        state.audio_players.set_filters(&session_id, &guild_id, filters).await;
    }
    if let Some(paused) = fields.get("paused").and_then(serde_json::Value::as_bool) {
        if state.audio_players.snapshot(&session_id, &guild_id).await
            .is_some_and(|snapshot| snapshot.track.is_some())
        {
            if let Err(error) = state.audio_players.pause(&session_id, &guild_id, paused).await {
                return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
            }
        }
    }
    if let Some(position) = fields.get("position").and_then(serde_json::Value::as_u64) {
        if state.audio_players.snapshot(&session_id, &guild_id).await
            .is_some_and(|snapshot| snapshot.track.is_some())
        {
            if let Err(error) = state.audio_players.seek(&session_id, &guild_id, position).await {
                return api_error(StatusCode::BAD_REQUEST, &error.message, &path);
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
