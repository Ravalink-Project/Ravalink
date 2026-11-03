//! Runtime state shared by the protocol and audio layers.

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
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
                "voice": null,
                "filters": {}
            })
        });

        if let Some(fields) = update.as_object() {
            for (name, value) in fields {
                match name.as_str() {
                    "guildId" | "state" | "endTime" => continue,
                    "track" => player["track"] = value.clone(),
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
    async fn explicit_null_track_clears_track() {
        let store = PlayerStore::default();
        store
            .update("s", "g", json!({"track": {"encoded": "abc"}}))
            .await;
        let player = store.update("s", "g", json!({"track": null})).await;
        assert!(player["track"].is_null());
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
