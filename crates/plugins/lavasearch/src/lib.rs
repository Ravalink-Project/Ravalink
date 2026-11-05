//! Native LavaSearch-compatible provider search.
//!
//! Track search results are resolved through LavaSrc so their playback metadata
//! and source identity are consistent with direct provider links. Collection
//! searches return structured provider metadata.

use async_trait::async_trait;
use ravalink_plugin_api::{
    PluginDescriptor, PluginError, PluginLoadResult, PluginLoadType, RavaLinkPlugin,
};
use ravalink_plugin_lavasrc::LavaSrcPlugin;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    env,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_SEARCH_QUERY_BYTES: usize = 512;
const MAX_COLLECTION_RESULTS: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProvider {
    YouTube,
    YouTubeMusic,
    SoundCloud,
    Spotify,
    AppleMusic,
    Deezer,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Tracks,
    Albums,
    Artists,
    Playlists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub provider: SearchProvider,
    pub kind: SearchKind,
    pub query: String,
}

#[derive(Clone)]
pub struct LavaSearchPlugin {
    client: reqwest::Client,
    lavasrc: LavaSrcPlugin,
    spotify_token: Arc<Mutex<Option<(String, Instant)>>>,
}

impl Default for LavaSearchPlugin {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(format!("RavaLink/{} (native LavaSearch)", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("valid default HTTP client"),
            lavasrc: LavaSrcPlugin::default(),
            spotify_token: Arc::new(Mutex::new(None)),
        }
    }
}

impl LavaSearchPlugin {
    pub fn parse_request(identifier: &str) -> Option<SearchRequest> {
        let (prefix, query) = identifier.split_once(':')?;
        let query = query.trim();
        if query.is_empty() || query.len() > MAX_SEARCH_QUERY_BYTES {
            return None;
        }
        let (provider, kind) = match prefix.to_ascii_lowercase().as_str() {
            "ytsearch" => (SearchProvider::YouTube, SearchKind::Tracks),
            "ytmsearch" => (SearchProvider::YouTubeMusic, SearchKind::Tracks),
            "scsearch" => (SearchProvider::SoundCloud, SearchKind::Tracks),
            "spsearch" => (SearchProvider::Spotify, SearchKind::Tracks),
            "amsearch" => (SearchProvider::AppleMusic, SearchKind::Tracks),
            "dzsearch" => (SearchProvider::Deezer, SearchKind::Tracks),
            "search" => (SearchProvider::All, SearchKind::Tracks),
            "album" => (SearchProvider::All, SearchKind::Albums),
            "artist" => (SearchProvider::All, SearchKind::Artists),
            "playlist" => (SearchProvider::All, SearchKind::Playlists),
            _ => return None,
        };
        Some(SearchRequest {
            provider,
            kind,
            query: query.to_owned(),
        })
    }

    async fn search_tracks(
        &self,
        provider: SearchProvider,
        query: &str,
    ) -> Result<PluginLoadResult, PluginError> {
        let links = match provider {
            SearchProvider::Spotify => self.spotify_track_links(query).await?,
            SearchProvider::AppleMusic => self.apple_track_links(query).await?,
            SearchProvider::Deezer => self.deezer_track_links(query).await?,
            SearchProvider::All => {
                let mut links = self.deezer_track_links(query).await.unwrap_or_default();
                links.extend(self.apple_track_links(query).await.unwrap_or_default());
                links.extend(self.spotify_track_links(query).await.unwrap_or_default());
                links
            }
            SearchProvider::YouTube | SearchProvider::YouTubeMusic => {
                return Err(PluginError::new(
                    "lavasearch",
                    "YouTube search is handled by the native YouTube plugin",
                ));
            }
            SearchProvider::SoundCloud => {
                return Err(PluginError::new(
                    "lavasearch",
                    "SoundCloud search is outside the enabled plugin scope",
                ));
            }
        };

        let mut tracks = Vec::new();
        for link in links.into_iter().take(5) {
            if let Ok(Some(result)) = self.lavasrc.load_tracks(&link).await
                && result.load_type == PluginLoadType::Track
            {
                tracks.push(result.data);
            }
        }
        Ok(PluginLoadResult::new(
            PluginLoadType::Search,
            Value::Array(tracks),
        ))
    }

    async fn search_collections(
        &self,
        kind: SearchKind,
        query: &str,
    ) -> Result<PluginLoadResult, PluginError> {
        let (endpoint, result_key) = match kind {
            SearchKind::Albums => ("album", "albums"),
            SearchKind::Artists => ("artist", "artists"),
            SearchKind::Playlists => ("playlist", "playlists"),
            SearchKind::Tracks => {
                return Err(PluginError::new(
                    "lavasearch",
                    "track search must use the track search endpoint",
                ));
            }
        };
        let deezer_data = self
            .deezer_search(endpoint, query)
            .await
            .ok()
            .and_then(|data| data.get("data").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        let mut results = deezer_data
            .iter()
            .map(|item| match kind {
                SearchKind::Albums => json!({
                    "id": item.get("id"),
                    "name": item.get("title"),
                    "uri": item.get("link"),
                    "artworkUrl": item.get("cover_xl"),
                    "artist": item.get("artist").and_then(|artist| artist.get("name")),
                    "source": "deezer",
                }),
                SearchKind::Artists => json!({
                    "id": item.get("id"),
                    "name": item.get("name"),
                    "uri": item.get("link"),
                    "artworkUrl": item.get("picture_xl"),
                    "source": "deezer",
                }),
                SearchKind::Playlists => json!({
                    "id": item.get("id"),
                    "name": item.get("title"),
                    "uri": item.get("link"),
                    "artworkUrl": item.get("picture_xl"),
                    "author": item.get("user").and_then(|user| user.get("name")),
                    "source": "deezer",
                }),
                SearchKind::Tracks => Value::Null,
            })
            .collect::<Vec<_>>();

        if let Ok(spotify) = self.spotify_search(query, endpoint).await {
            let path = format!("/{endpoint}s/items");
            if let Some(items) = spotify.pointer(&path).and_then(Value::as_array) {
                results.extend(items.iter().map(|item| {
                    let artwork = item
                        .get("images")
                        .and_then(Value::as_array)
                        .and_then(|images| images.first())
                        .and_then(|image| image.get("url"));
                    let artist = item
                        .get("artists")
                        .and_then(Value::as_array)
                        .and_then(|artists| artists.first())
                        .and_then(|artist| artist.get("name"));
                    json!({
                        "id": item.get("id"),
                        "name": item.get("name"),
                        "uri": item.pointer("/external_urls/spotify"),
                        "artworkUrl": artwork,
                        "artist": artist,
                        "author": item.get("owner").and_then(|owner| owner.get("display_name")),
                        "source": "spotify",
                    })
                }));
            }
        }

        if kind != SearchKind::Playlists {
            let entity = match kind {
                SearchKind::Albums => "album",
                SearchKind::Artists => "musicArtist",
                SearchKind::Tracks | SearchKind::Playlists => unreachable!(),
            };
            if let Ok(apple) = self.apple_search(query, entity).await
                && let Some(items) = apple.get("results").and_then(Value::as_array)
            {
                results.extend(items.iter().map(|item| {
                    let (id, name, uri) = if kind == SearchKind::Albums {
                        (
                            item.get("collectionId"),
                            item.get("collectionName"),
                            item.get("collectionViewUrl"),
                        )
                    } else {
                        (
                            item.get("artistId"),
                            item.get("artistName"),
                            item.get("artistLinkUrl"),
                        )
                    };
                    json!({
                        "id": id,
                        "name": name,
                        "uri": uri,
                        "artworkUrl": item.get("artworkUrl100"),
                        "artist": item.get("artistName"),
                        "source": "applemusic",
                    })
                }));
            }
        }

        let mut seen = HashSet::new();
        results.retain(|item| {
            let source = item
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = item.get("id").map(Value::to_string).unwrap_or_default();
            seen.insert((source.to_owned(), id))
        });
        results.truncate(MAX_COLLECTION_RESULTS);

        if results.is_empty() {
            return Err(PluginError::new(
                "lavasearch",
                "no search provider returned results; check provider availability and credentials",
            ));
        }
        let mut payload = serde_json::Map::new();
        payload.insert(result_key.to_owned(), Value::Array(results));
        Ok(PluginLoadResult::new(
            PluginLoadType::Search,
            Value::Object(payload),
        ))
    }
    async fn deezer_search(&self, kind: &str, query: &str) -> Result<Value, PluginError> {
        let endpoint = if kind == "track" {
            "https://api.deezer.com/search".to_owned()
        } else {
            format!("https://api.deezer.com/search/{kind}")
        };
        self.client
            .get(endpoint)
            .query(&[("q", query)])
            .send()
            .await
            .map_err(|error| {
                PluginError::new("lavasearch", format!("Deezer request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasearch", format!("Deezer returned an error: {error}"))
            })?
            .json()
            .await
            .map_err(|error| {
                PluginError::new("lavasearch", format!("invalid Deezer response: {error}"))
            })
    }

    async fn apple_search(&self, query: &str, entity: &str) -> Result<Value, PluginError> {
        self.client
            .get("https://itunes.apple.com/search")
            .query(&[("term", query), ("entity", entity), ("limit", "5")])
            .send()
            .await
            .map_err(|error| {
                PluginError::new("lavasearch", format!("Apple Music request failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new(
                    "lavasearch",
                    format!("Apple Music returned an error: {error}"),
                )
            })?
            .json()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasearch",
                    format!("invalid Apple Music response: {error}"),
                )
            })
    }

    async fn spotify_token(&self) -> Result<String, PluginError> {
        if let Ok(cache) = self.spotify_token.lock()
            && let Some((token, expires_at)) = cache.as_ref()
            && *expires_at > Instant::now()
        {
            return Ok(token.clone());
        }

        let client_id = env::var("RAVALINK_SPOTIFY_CLIENT_ID").map_err(|_| {
            PluginError::new(
                "lavasearch",
                "Spotify search requires RAVALINK_SPOTIFY_CLIENT_ID and RAVALINK_SPOTIFY_CLIENT_SECRET",
            )
        })?;
        let client_secret = env::var("RAVALINK_SPOTIFY_CLIENT_SECRET").map_err(|_| {
            PluginError::new(
                "lavasearch",
                "Spotify search requires RAVALINK_SPOTIFY_CLIENT_ID and RAVALINK_SPOTIFY_CLIENT_SECRET",
            )
        })?;
        let response: Value = self
            .client
            .post("https://accounts.spotify.com/api/token")
            .basic_auth(client_id, Some(client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasearch",
                    format!("Spotify token request failed: {error}"),
                )
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new(
                    "lavasearch",
                    format!("Spotify authentication failed: {error}"),
                )
            })?
            .json()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasearch",
                    format!("invalid Spotify token response: {error}"),
                )
            })?;
        let token = response
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| PluginError::new("lavasearch", "Spotify returned no access token"))?
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

    async fn spotify_search(&self, query: &str, kind: &str) -> Result<Value, PluginError> {
        let token = self.spotify_token().await?;
        self.client
            .get("https://api.spotify.com/v1/search")
            .bearer_auth(token)
            .query(&[("q", query), ("type", kind), ("limit", "10")])
            .send()
            .await
            .map_err(|error| {
                PluginError::new("lavasearch", format!("Spotify search failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                PluginError::new("lavasearch", format!("Spotify returned an error: {error}"))
            })?
            .json()
            .await
            .map_err(|error| {
                PluginError::new(
                    "lavasearch",
                    format!("invalid Spotify search response: {error}"),
                )
            })
    }

    async fn spotify_track_links(&self, query: &str) -> Result<Vec<String>, PluginError> {
        let data = self.spotify_search(query, "track").await?;
        Ok(data
            .pointer("/tracks/items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.pointer("/external_urls/spotify")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect())
    }

    async fn apple_track_links(&self, query: &str) -> Result<Vec<String>, PluginError> {
        let data = self.apple_search(query, "song").await?;
        Ok(data
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("trackViewUrl")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect())
    }

    async fn deezer_track_links(&self, query: &str) -> Result<Vec<String>, PluginError> {
        let data = self.deezer_search("track", query).await?;
        Ok(data
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("link")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        item.get("id")
                            .and_then(Value::as_u64)
                            .map(|id| format!("https://www.deezer.com/track/{id}"))
                    })
            })
            .collect())
    }
}

#[async_trait]
impl RavaLinkPlugin for LavaSearchPlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "lavasearch".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Advanced search for tracks, albums, artists and playlists".into(),
            capabilities: vec![
                "track-search".into(),
                "album-search".into(),
                "artist-search".into(),
                "playlist-search".into(),
            ],
        }
    }

    fn supports_identifier(&self, identifier: &str) -> bool {
        Self::parse_request(identifier).is_some()
    }

    async fn load_tracks(&self, identifier: &str) -> Result<Option<PluginLoadResult>, PluginError> {
        let Some(request) = Self::parse_request(identifier) else {
            return Ok(None);
        };
        let result = match request.kind {
            SearchKind::Tracks => self.search_tracks(request.provider, &request.query).await?,
            kind => self.search_collections(kind, &request.query).await?,
        };
        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_search_queries() {
        let query = format!("ytsearch:{}", "x".repeat(MAX_SEARCH_QUERY_BYTES + 1));
        assert!(LavaSearchPlugin::parse_request(&query).is_none());
    }

    #[test]
    fn parses_standard_lavalink_search_prefixes() {
        let request = LavaSearchPlugin::parse_request("ytsearch:Daft Punk").unwrap();
        assert_eq!(request.provider, SearchProvider::YouTube);
        assert_eq!(request.kind, SearchKind::Tracks);
        assert_eq!(request.query, "Daft Punk");

        let request = LavaSearchPlugin::parse_request("spsearch:Discovery").unwrap();
        assert_eq!(request.provider, SearchProvider::Spotify);
    }

    #[test]
    fn parses_structured_search_kinds() {
        assert_eq!(
            LavaSearchPlugin::parse_request("album:Discovery")
                .unwrap()
                .kind,
            SearchKind::Albums
        );
        assert_eq!(
            LavaSearchPlugin::parse_request("artist:Daft Punk")
                .unwrap()
                .kind,
            SearchKind::Artists
        );
        assert_eq!(
            LavaSearchPlugin::parse_request("playlist:French house")
                .unwrap()
                .kind,
            SearchKind::Playlists
        );
    }

    #[test]
    fn rejects_unknown_or_empty_searches() {
        assert!(LavaSearchPlugin::parse_request("unknown:test").is_none());
        assert!(LavaSearchPlugin::parse_request("ytsearch: ").is_none());
    }
}
