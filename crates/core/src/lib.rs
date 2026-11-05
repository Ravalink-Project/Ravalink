//! Runtime state shared by the protocol and audio layers.

use ravalink_audio::{
    AudioError, AudioPipeline, AudioTrack, DaveVoiceSession, DiscordUdpTransport, NativePlayer,
    PcmFrame, PlayerSnapshot, RtpPacketizer, VoicePacketPipeline,
};
use ravalink_protocol::Filters;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub user_id: u64,
    pub client_name: Option<String>,
    pub resumable: bool,
    pub resume_timeout_seconds: u64,
    pub created_at: Instant,
    pub disconnected_at: Option<Instant>,
}

impl Session {
    /// A disconnected session can only be resumed before its configured deadline.
    pub fn can_resume(&self, now: Instant) -> bool {
        self.resumable
            && self.disconnected_at.is_some_and(|disconnected| {
                now.saturating_duration_since(disconnected)
                    < Duration::from_secs(self.resume_timeout_seconds)
            })
    }
}

#[derive(Clone, Default)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionStore {
    pub async fn create(&self, user_id: u64, client_name: Option<String>) -> Session {
        let id = Uuid::new_v4().simple().to_string()[..16].to_owned();
        let session = Session {
            id: id.clone(),
            user_id,
            client_name,
            resumable: false,
            resume_timeout_seconds: 60,
            created_at: Instant::now(),
            disconnected_at: None,
        };
        self.sessions.write().await.insert(id, session.clone());
        session
    }

    pub async fn get(&self, id: &str) -> Option<Session> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Checks whether a session can be resumed without consuming its resume state.
    pub async fn can_resume(&self, id: &str, user_id: u64) -> bool {
        self.sessions
            .read()
            .await
            .get(id)
            .is_some_and(|session| session.user_id == user_id && session.can_resume(Instant::now()))
    }

    /// Finds a resumable session and atomically marks it as connected again.
    pub async fn resume(&self, id: &str, user_id: u64) -> Option<Session> {
        let now = Instant::now();
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id)?;
        if session.user_id != user_id || !session.can_resume(now) {
            return None;
        }
        session.disconnected_at = None;
        Some(session.clone())
    }

    pub async fn update(
        &self,
        id: &str,
        resumable: Option<bool>,
        timeout: Option<u64>,
    ) -> Option<Session> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id)?;
        if let Some(value) = resumable {
            session.resumable = value;
            if !value {
                session.disconnected_at = None;
            }
        }
        if let Some(value) = timeout {
            session.resume_timeout_seconds = value;
        }
        Some(session.clone())
    }

    /// Records a clean or unexpected disconnect. Non-resumable sessions are removed.
    pub async fn disconnect(&self, id: &str) {
        let mut sessions = self.sessions.write().await;
        match sessions.get_mut(id) {
            Some(session) if session.resumable => {
                session.disconnected_at = Some(Instant::now());
            }
            Some(_) => {
                sessions.remove(id);
            }
            None => {}
        }
    }

    /// Removes sessions whose resume deadline has elapsed.
    pub async fn prune_expired(&self) -> Vec<String> {
        let now = Instant::now();
        let mut sessions = self.sessions.write().await;
        let expired: Vec<String> = sessions
            .iter()
            .filter(|(_, session)| session.disconnected_at.is_some() && !session.can_resume(now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            sessions.remove(id);
        }
        expired
    }

    pub async fn remove(&self, id: &str) {
        self.sessions.write().await.remove(id);
    }

    pub async fn len(&self) -> usize {
        self.sessions.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.sessions.read().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_can_be_created_and_updated() {
        let store = SessionStore::default();
        let session = store.create(123, Some("test-client".into())).await;
        assert_eq!(store.len().await, 1);

        let updated = store
            .update(&session.id, Some(true), Some(120))
            .await
            .unwrap();
        assert!(updated.resumable);
        assert_eq!(updated.resume_timeout_seconds, 120);
    }

    #[tokio::test]
    async fn resumable_session_is_marked_disconnected_and_resumed() {
        let store = SessionStore::default();
        let session = store.create(123, None).await;
        store
            .update(&session.id, Some(true), Some(120))
            .await
            .unwrap();
        store.disconnect(&session.id).await;

        let resumed = store.resume(&session.id, 123).await.unwrap();
        assert!(resumed.disconnected_at.is_none());
    }

    #[tokio::test]
    async fn can_resume_checks_user_and_deadline_without_consuming_session() {
        let store = SessionStore::default();
        let session = store.create(123, None).await;
        store
            .update(&session.id, Some(true), Some(120))
            .await
            .unwrap();
        store.disconnect(&session.id).await;

        assert!(store.can_resume(&session.id, 123).await);
        assert!(!store.can_resume(&session.id, 999).await);
        assert!(store.can_resume(&session.id, 123).await);
        assert!(store.resume(&session.id, 123).await.is_some());
        assert!(!store.can_resume(&session.id, 123).await);
    }

    #[tokio::test]
    async fn wrong_user_cannot_resume_session() {
        let store = SessionStore::default();
        let session = store.create(123, None).await;
        store
            .update(&session.id, Some(true), Some(120))
            .await
            .unwrap();
        store.disconnect(&session.id).await;

        assert!(store.resume(&session.id, 999).await.is_none());
    }

    #[tokio::test]
    async fn expired_resumable_sessions_are_pruned() {
        let store = SessionStore::default();
        let session = store.create(123, None).await;
        store
            .update(&session.id, Some(true), Some(0))
            .await
            .unwrap();
        store.disconnect(&session.id).await;

        assert_eq!(store.prune_expired().await, vec![session.id.clone()]);
        assert!(store.get(&session.id).await.is_none());
    }

    #[tokio::test]
    async fn non_resumable_disconnect_removes_session() {
        let store = SessionStore::default();
        let session = store.create(123, None).await;
        store.disconnect(&session.id).await;
        assert!(store.get(&session.id).await.is_none());
    }
}

/// In-memory player state, scoped to a Lavalink session and Discord guild.
#[derive(Clone, Default)]
pub struct PlayerStore {
    players: Arc<RwLock<HashMap<(String, String), Value>>>,
}

impl PlayerStore {
    /// Applies a Lavalink v4 player update and returns the complete player state.
    pub async fn update(&self, session_id: &str, guild_id: &str, update: Value) -> Value {
        let key = (session_id.to_owned(), guild_id.to_owned());
        let mut players = self.players.write().await;
        let player = players.entry(key).or_insert_with(|| {
            json!({
                "guildId": guild_id,
                "track": null,
                "volume": 100,
                "paused": false,
                "state": {
                    "time": 0,
                    "position": 0,
                    "connected": false,
                    "ping": -1
                },
                "voice": {
                    "token": "",
                    "endpoint": "",
                    "sessionId": "",
                    "channelId": null
                },
                "filters": {}
            })
        });

        if let Some(fields) = update.as_object() {
            for (name, value) in fields {
                match name.as_str() {
                    "guildId" | "state" | "endTime" => continue,
                    "track" => player["track"] = value.clone(),
                    "userData" => {
                        if player["track"].is_object() {
                            player["track"]["userData"] = value.clone();
                        }
                    }
                    "position" => player["state"]["position"] = value.clone(),
                    "filters" if value.is_object() => player["filters"] = value.clone(),
                    _ => player[name] = value.clone(),
                }
            }
        }
        player.clone()
    }

    pub async fn get(&self, session_id: &str, guild_id: &str) -> Option<Value> {
        self.players
            .read()
            .await
            .get(&(session_id.to_owned(), guild_id.to_owned()))
            .cloned()
    }

    pub async fn list(&self, session_id: &str) -> Vec<Value> {
        self.players
            .read()
            .await
            .iter()
            .filter(|((session, _), _)| session == session_id)
            .map(|(_, player)| player.clone())
            .collect()
    }

    pub async fn remove(&self, session_id: &str, guild_id: &str) -> bool {
        self.players
            .write()
            .await
            .remove(&(session_id.to_owned(), guild_id.to_owned()))
            .is_some()
    }

    pub async fn remove_session(&self, session_id: &str) -> usize {
        let mut players = self.players.write().await;
        let before = players.len();
        players.retain(|(session, _), _| session != session_id);
        before - players.len()
    }

    pub async fn len(&self) -> usize {
        self.players.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.players.read().await.is_empty()
    }

    /// Update server-owned playback state without accepting client-supplied state fields.
    pub async fn set_runtime_state(
        &self,
        session_id: &str,
        guild_id: &str,
        position_ms: u64,
        connected: bool,
        ping_ms: i64,
    ) {
        let key = (session_id.to_owned(), guild_id.to_owned());
        let mut players = self.players.write().await;
        if let Some(player) = players.get_mut(&key) {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or_default();
            player["state"]["time"] = json!(now_ms);
            player["state"]["position"] = json!(position_ms);
            player["state"]["connected"] = json!(connected);
            player["state"]["ping"] = json!(ping_ms);
        }
    }

    pub async fn playing_len(&self) -> usize {
        self.players
            .read()
            .await
            .values()
            .filter(|player| player["track"].is_object() && player["paused"] != true)
            .count()
    }
}

#[cfg(test)]
mod player_store_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn player_updates_are_scoped_to_session_and_guild() {
        let store = PlayerStore::default();
        let player = store
            .update(
                "session-a",
                "guild-a",
                json!({"volume": 75, "paused": true}),
            )
            .await;
        assert_eq!(player["volume"], 75);
        assert!(player["paused"].as_bool().unwrap());
        assert_eq!(player["guildId"], "guild-a");
        assert!(store.get("session-b", "guild-a").await.is_none());
    }

    #[tokio::test]
    async fn position_updates_the_player_state_object() {
        let store = PlayerStore::default();
        let player = store.update("s", "g", json!({"position": 1250})).await;
        assert_eq!(player["state"]["position"], 1250);
        assert!(player.get("position").is_none());
    }

    #[tokio::test]
    async fn user_data_update_changes_metadata_without_replacing_track() {
        let store = PlayerStore::default();
        store
            .update(
                "s",
                "g",
                json!({"track": {"encoded": "abc", "userData": {"old": true}}}),
            )
            .await;

        store
            .update("s", "g", json!({"userData": {"new": true}}))
            .await;

        let player = store.get("s", "g").await.unwrap();
        assert_eq!(player["track"]["encoded"], "abc");
        assert_eq!(player["track"]["userData"], json!({"new": true}));
    }

    #[tokio::test]
    async fn explicit_null_track_clears_track() {
        let store = PlayerStore::default();
        store
            .update("s", "g", json!({"track": {"encoded": "abc"}}))
            .await;
        let player = store.update("s", "g", json!({"track": null})).await;
        assert!(player["track"].is_null());
    }

    #[tokio::test]
    async fn runtime_state_updates_only_server_owned_state_fields() {
        let store = PlayerStore::default();
        store.update("s", "g", json!({"volume": 80})).await;
        store.set_runtime_state("s", "g", 1500, true, -1).await;
        let player = store.get("s", "g").await.unwrap();
        assert_eq!(player["state"]["position"], 1500);
        assert_eq!(player["state"]["connected"], true);
        assert_eq!(player["state"]["ping"], -1);
        assert_eq!(player["volume"], 80);
    }

    #[tokio::test]
    async fn session_cleanup_removes_only_its_players() {
        let store = PlayerStore::default();
        store.update("a", "1", json!({})).await;
        store.update("b", "2", json!({})).await;
        assert_eq!(store.remove_session("a").await, 1);
        assert_eq!(store.len().await, 1);
    }
}

type PlayerKey = (String, String);
type SharedNativePlayer = Arc<tokio::sync::Mutex<NativePlayer>>;

/// Native audio players keyed by Lavalink session and Discord guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordVoiceState {
    pub token: String,
    pub endpoint: String,
    pub session_id: String,
    pub channel_id: String,
}

#[derive(Default)]
struct DecodedPcmBudget {
    used_bytes: usize,
    player_bytes: HashMap<PlayerKey, usize>,
}

#[derive(Clone)]
pub struct AudioPlayerStore {
    players: Arc<RwLock<HashMap<PlayerKey, SharedNativePlayer>>>,
    opus_complexity: u8,
    voice_states: Arc<RwLock<HashMap<PlayerKey, DiscordVoiceState>>>,
    pcm_budget: Arc<Mutex<DecodedPcmBudget>>,
    max_decoded_pcm_bytes: usize,
}

impl Default for AudioPlayerStore {
    fn default() -> Self {
        Self::new(10)
    }
}

impl AudioPlayerStore {
    pub fn new(opus_complexity: u8) -> Self {
        Self::new_with_limits(opus_complexity, 256 * 1024 * 1024)
    }

    pub fn new_with_limits(opus_complexity: u8, max_decoded_pcm_bytes: usize) -> Self {
        Self {
            players: Arc::new(RwLock::new(HashMap::new())),
            opus_complexity: opus_complexity.min(10),
            voice_states: Arc::new(RwLock::new(HashMap::new())),
            pcm_budget: Arc::new(Mutex::new(DecodedPcmBudget::default())),
            max_decoded_pcm_bytes,
        }
    }

    pub fn active_pcm_bytes(&self) -> usize {
        self.pcm_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .used_bytes
    }

    pub fn max_active_pcm_bytes(&self) -> usize {
        self.max_decoded_pcm_bytes
    }

    fn release_pcm_budget(&self, key: &PlayerKey) {
        let mut budget = self
            .pcm_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(bytes) = budget.player_bytes.remove(key) {
            budget.used_bytes = budget.used_bytes.saturating_sub(bytes);
        }
    }
    async fn get_or_create(&self, session_id: &str, guild_id: &str) -> SharedNativePlayer {
        let key = (session_id.to_owned(), guild_id.to_owned());
        let mut players = self.players.write().await;
        players
            .entry(key)
            .or_insert_with(|| {
                Arc::new(tokio::sync::Mutex::new(
                    NativePlayer::new_with_opus_complexity(
                        Filters::default(),
                        self.opus_complexity,
                    ),
                ))
            })
            .clone()
    }

    pub async fn start(
        &self,
        session_id: &str,
        guild_id: &str,
        track: AudioTrack,
    ) -> Result<(), AudioError> {
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        player.start(track)?;
        drop(player);
        self.release_pcm_budget(&(session_id.to_owned(), guild_id.to_owned()));
        Ok(())
    }

    pub async fn pause(
        &self,
        session_id: &str,
        guild_id: &str,
        paused: bool,
    ) -> Result<(), AudioError> {
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        player.pause(paused)
    }

    pub async fn seek(
        &self,
        session_id: &str,
        guild_id: &str,
        position_ms: u64,
    ) -> Result<(), AudioError> {
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        player.seek(position_ms)
    }

    pub async fn stop(&self, session_id: &str, guild_id: &str) {
        if let Some(player) = self.get(session_id, guild_id).await {
            let _ = player.lock().await.stop();
        }
        self.release_pcm_budget(&(session_id.to_owned(), guild_id.to_owned()));
    }

    pub async fn set_end_time(
        &self,
        session_id: &str,
        guild_id: &str,
        end_time_ms: Option<u64>,
    ) -> Result<(), AudioError> {
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        player.set_end_time(end_time_ms)
    }

    pub async fn set_volume(
        &self,
        session_id: &str,
        guild_id: &str,
        volume: u16,
    ) -> Result<(), AudioError> {
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        player.set_volume(volume)
    }

    pub async fn set_filters(&self, session_id: &str, guild_id: &str, filters: Filters) {
        let player = self.get_or_create(session_id, guild_id).await;
        player.lock().await.set_filters(filters);
    }

    pub async fn load_decoded_track(
        &self,
        session_id: &str,
        guild_id: &str,
        track: AudioTrack,
        frames: Vec<PcmFrame>,
    ) -> Result<(), AudioError> {
        let key = (session_id.to_owned(), guild_id.to_owned());
        let new_bytes = frames.iter().fold(0_usize, |total, frame| {
            total.saturating_add(
                frame
                    .samples
                    .len()
                    .saturating_mul(std::mem::size_of::<f32>()),
            )
        });
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        let mut budget = self
            .pcm_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_bytes = budget.player_bytes.get(&key).copied().unwrap_or_default();
        let next_total = budget
            .used_bytes
            .saturating_sub(previous_bytes)
            .saturating_add(new_bytes);
        if next_total > self.max_decoded_pcm_bytes {
            return Err(AudioError {
                message: format!(
                    "decoded PCM budget exceeded: {next_total} bytes requested, limit is {} bytes",
                    self.max_decoded_pcm_bytes
                ),
            });
        }
        player.load_decoded_track(track, frames)?;
        budget.used_bytes = next_total;
        if new_bytes == 0 {
            budget.player_bytes.remove(&key);
        } else {
            budget.player_bytes.insert(key, new_bytes);
        }
        Ok(())
    }

    /// Replace the active track with a bounded PCM stream.
    pub async fn load_streamed_track(
        &self,
        session_id: &str,
        guild_id: &str,
        track: AudioTrack,
        receiver: tokio::sync::mpsc::Receiver<Result<PcmFrame, AudioError>>,
    ) -> Result<(), AudioError> {
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        player.load_streamed_track(track, receiver)?;
        drop(player);
        self.release_pcm_budget(&(session_id.to_owned(), guild_id.to_owned()));
        Ok(())
    }

    /// Attach a bounded source stream starting at the requested position.
    pub async fn load_streamed_track_at(
        &self,
        session_id: &str,
        guild_id: &str,
        track: AudioTrack,
        position_ms: u64,
        receiver: tokio::sync::mpsc::Receiver<Result<PcmFrame, AudioError>>,
    ) -> Result<(), AudioError> {
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        player.load_streamed_track_at(track, position_ms, receiver)?;
        drop(player);
        self.release_pcm_budget(&(session_id.to_owned(), guild_id.to_owned()));
        Ok(())
    }

    /// Restart a buffered source at a new position without resetting player controls.
    pub async fn restart_streamed_source(
        &self,
        session_id: &str,
        guild_id: &str,
        position_ms: u64,
        receiver: tokio::sync::mpsc::Receiver<Result<PcmFrame, AudioError>>,
    ) -> Result<(), AudioError> {
        let player = self.get_or_create(session_id, guild_id).await;
        let mut player = player.lock().await;
        player.restart_streamed_source(position_ms, receiver)
    }

    pub async fn next_encrypted_voice_packet(
        &self,
        session_id: &str,
        guild_id: &str,
        pipeline: &mut VoicePacketPipeline,
    ) -> Result<Option<Vec<u8>>, AudioError> {
        let Some(player) = self.get(session_id, guild_id).await else {
            return Ok(None);
        };
        let mut player = player.lock().await;
        player.next_encrypted_voice_packet(pipeline)
    }

    /// Produce a DAVE-protected Opus frame and encrypt its RTP transport packet.
    pub async fn next_dave_encrypted_voice_packet(
        &self,
        session_id: &str,
        guild_id: &str,
        pipeline: &mut VoicePacketPipeline,
        dave: &mut DaveVoiceSession,
    ) -> Result<Option<Vec<u8>>, AudioError> {
        let Some(player) = self.get(session_id, guild_id).await else {
            return Ok(None);
        };
        let mut player = player.lock().await;
        player.next_dave_encrypted_voice_packet(pipeline, dave)
    }

    /// Produce, encrypt and send the next available Opus voice packet.
    ///
    /// The caller owns the negotiated packet pipeline and drives this method at
    /// Discord's 20 ms voice cadence. No player lock is held during UDP I/O.
    pub async fn send_next_voice_packet(
        &self,
        session_id: &str,
        guild_id: &str,
        pipeline: &mut VoicePacketPipeline,
        transport: &DiscordUdpTransport,
    ) -> Result<bool, AudioError> {
        let Some(packet) = self
            .next_encrypted_voice_packet(session_id, guild_id, pipeline)
            .await?
        else {
            return Ok(false);
        };
        transport.send_voice_packet(&packet).await?;
        Ok(true)
    }

    /// Run a player's encrypted UDP voice output at Discord's 20 ms cadence.
    ///
    /// The task ends when the track finishes, the player is removed, or the
    /// caller signals shutdown. Paused players remain attached to the sender
    /// and resume without rebuilding the negotiated voice pipeline.
    pub async fn run_voice_output(
        &self,
        session_id: String,
        guild_id: String,
        mut pipeline: VoicePacketPipeline,
        transport: DiscordUdpTransport,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), AudioError> {
        let mut interval = tokio::time::interval(Duration::from_millis(20));
        interval.tick().await;

        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
                _ = interval.tick() => {
                    let Some(snapshot) = self.snapshot(&session_id, &guild_id).await else {
                        return Ok(());
                    };
                    if snapshot.track.is_none() {
                        return Ok(());
                    }
                    if snapshot.paused {
                        continue;
                    }

                    let sent = self
                        .send_next_voice_packet(
                            &session_id,
                            &guild_id,
                            &mut pipeline,
                            &transport,
                        )
                        .await?;
                    if !sent {
                        let snapshot = self.snapshot(&session_id, &guild_id).await;
                        if snapshot.is_some_and(|player| player.track.is_some() && player.paused) {
                            continue;
                        }
                        return Ok(());
                    }
                }
            }
        }
    }

    pub async fn next_rtp_packet(
        &self,
        session_id: &str,
        guild_id: &str,
        packetizer: &mut RtpPacketizer,
    ) -> Result<Option<Vec<u8>>, AudioError> {
        let Some(player) = self.get(session_id, guild_id).await else {
            return Ok(None);
        };
        let mut player = player.lock().await;
        player.next_rtp_packet(packetizer)
    }

    pub async fn next_opus_packet(
        &self,
        session_id: &str,
        guild_id: &str,
    ) -> Result<Option<Vec<u8>>, AudioError> {
        let Some(player) = self.get(session_id, guild_id).await else {
            return Ok(None);
        };
        let mut player = player.lock().await;
        player.next_opus_packet()
    }

    pub async fn next_voice_frame(
        &self,
        session_id: &str,
        guild_id: &str,
    ) -> Result<Option<PcmFrame>, AudioError> {
        let Some(player) = self.get(session_id, guild_id).await else {
            return Ok(None);
        };
        let mut player = player.lock().await;
        player.next_voice_frame()
    }

    pub async fn next_frame(
        &self,
        session_id: &str,
        guild_id: &str,
    ) -> Result<Option<PcmFrame>, AudioError> {
        let Some(player) = self.get(session_id, guild_id).await else {
            return Ok(None);
        };
        let mut player = player.lock().await;
        player.next_frame()
    }

    pub async fn process_frame(
        &self,
        session_id: &str,
        guild_id: &str,
        frame: PcmFrame,
    ) -> Result<Option<PcmFrame>, AudioError> {
        let Some(player) = self.get(session_id, guild_id).await else {
            return Ok(None);
        };
        let mut player = player.lock().await;
        player.process_frame(frame)
    }

    pub async fn take_completed_track(
        &self,
        session_id: &str,
        guild_id: &str,
    ) -> Option<AudioTrack> {
        let player = self.get(session_id, guild_id).await?;
        let completed = player.lock().await.take_completed_track();
        if completed.is_some() {
            self.release_pcm_budget(&(session_id.to_owned(), guild_id.to_owned()));
        }
        completed
    }

    pub async fn snapshot(&self, session_id: &str, guild_id: &str) -> Option<PlayerSnapshot> {
        let player = self.get(session_id, guild_id).await?;
        let player = player.lock().await;
        Some(player.snapshot())
    }

    pub async fn get(&self, session_id: &str, guild_id: &str) -> Option<SharedNativePlayer> {
        self.players
            .read()
            .await
            .get(&(session_id.to_owned(), guild_id.to_owned()))
            .cloned()
    }

    pub async fn set_voice_state(
        &self,
        session_id: &str,
        guild_id: &str,
        voice_state: Option<DiscordVoiceState>,
    ) {
        let key = (session_id.to_owned(), guild_id.to_owned());
        let mut states = self.voice_states.write().await;
        if let Some(voice_state) = voice_state {
            states.insert(key, voice_state);
        } else {
            states.remove(&key);
        }
    }

    pub async fn voice_state(&self, session_id: &str, guild_id: &str) -> Option<DiscordVoiceState> {
        self.voice_states
            .read()
            .await
            .get(&(session_id.to_owned(), guild_id.to_owned()))
            .cloned()
    }

    pub async fn remove(&self, session_id: &str, guild_id: &str) -> bool {
        let key = (session_id.to_owned(), guild_id.to_owned());
        let removed_player = self.players.write().await.remove(&key).is_some();
        let removed_voice = self.voice_states.write().await.remove(&key).is_some();
        self.release_pcm_budget(&key);
        removed_player || removed_voice
    }
    pub async fn remove_session(&self, session_id: &str) -> usize {
        let mut players = self.players.write().await;
        let before = players.len();
        players.retain(|(session, _), _| session != session_id);
        let removed = before - players.len();
        self.voice_states
            .write()
            .await
            .retain(|(session, _), _| session != session_id);
        let mut budget = self
            .pcm_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys = budget
            .player_bytes
            .keys()
            .filter(|(session, _)| session == session_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(bytes) = budget.player_bytes.remove(&key) {
                budget.used_bytes = budget.used_bytes.saturating_sub(bytes);
            }
        }
        removed
    }

    pub async fn len(&self) -> usize {
        self.players.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.players.read().await.is_empty()
    }

    pub async fn playing_len(&self) -> usize {
        let players: Vec<_> = self.players.read().await.values().cloned().collect();
        let mut count = 0;
        for player in players {
            let snapshot = player.lock().await.snapshot();
            if snapshot.track.is_some() && !snapshot.paused {
                count += 1;
            }
        }
        count
    }
}

#[cfg(test)]
mod audio_player_store_tests {
    use super::*;

    #[tokio::test]
    async fn voice_state_is_stored_and_removed_per_player() {
        let store = AudioPlayerStore::default();
        let voice = DiscordVoiceState {
            token: "voice-token".into(),
            endpoint: "voice.example:443".into(),
            session_id: "discord-session".into(),
            channel_id: "voice-channel".into(),
        };

        store
            .set_voice_state("session-a", "guild-a", Some(voice.clone()))
            .await;
        assert_eq!(store.voice_state("session-a", "guild-a").await, Some(voice));
        assert!(store.remove("session-a", "guild-a").await);
        assert!(store.voice_state("session-a", "guild-a").await.is_none());
    }

    #[tokio::test]
    async fn native_players_are_isolated_by_session_and_guild() {
        let store = AudioPlayerStore::default();
        let track = AudioTrack {
            identifier: "track".into(),
            title: "Title".into(),
            author: "Artist".into(),
            duration_ms: Some(60_000),
            is_stream: false,
        };
        store.start("session-a", "guild-a", track).await.unwrap();
        store.pause("session-a", "guild-a", true).await.unwrap();

        let snapshot = store.snapshot("session-a", "guild-a").await.unwrap();
        assert!(snapshot.track.is_some());
        assert!(snapshot.paused);
        assert!(store.snapshot("session-b", "guild-a").await.is_none());
        assert_eq!(store.playing_len().await, 0);
    }

    #[tokio::test]
    async fn decoded_pcm_budget_is_shared_and_released_when_a_track_stops() {
        let store = AudioPlayerStore::new_with_limits(10, 24);
        let track = |identifier: &str| AudioTrack {
            identifier: identifier.into(),
            title: identifier.into(),
            author: "Artist".into(),
            duration_ms: Some(60_000),
            is_stream: false,
        };
        let frames = || {
            vec![PcmFrame {
                samples: vec![0.1; 4],
                channels: 2,
                sample_rate: 48_000,
            }]
        };

        store
            .load_decoded_track("session", "guild-a", track("a"), frames())
            .await
            .unwrap();
        assert_eq!(store.active_pcm_bytes(), 16);
        assert!(
            store
                .load_decoded_track("session", "guild-b", track("b"), frames())
                .await
                .is_err()
        );

        store.stop("session", "guild-a").await;
        assert_eq!(store.active_pcm_bytes(), 0);
        store
            .load_decoded_track("session", "guild-b", track("b"), frames())
            .await
            .unwrap();
        assert_eq!(store.active_pcm_bytes(), 16);
    }

    #[tokio::test]
    async fn native_player_volume_and_filter_updates_are_scoped() {
        let store = AudioPlayerStore::default();
        store.set_volume("s", "g", 250).await.unwrap();
        store.set_filters("s", "g", Filters::default()).await;
        assert_eq!(store.snapshot("s", "g").await.unwrap().volume, 250);
    }
}
