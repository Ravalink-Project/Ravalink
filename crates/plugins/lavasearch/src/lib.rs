//! Search syntax and result-kind contracts compatible with LavaSearch.

use ravalink_plugin_api::{PluginDescriptor, RavaLinkPlugin};

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

#[derive(Debug, Clone, Copy, Default)]
pub struct LavaSearchPlugin;

impl LavaSearchPlugin {
    pub fn parse_request(identifier: &str) -> Option<SearchRequest> {
        let (prefix, query) = identifier.split_once(':')?;
        let query = query.trim();
        if query.is_empty() {
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
}

impl RavaLinkPlugin for LavaSearchPlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "lavasearch",
            version: env!("CARGO_PKG_VERSION"),
            description: "Advanced search for tracks, albums, artists and playlists",
            capabilities: &["track-search", "album-search", "artist-search", "playlist-search"],
        }
    }

    fn supports_identifier(&self, identifier: &str) -> bool {
        Self::parse_request(identifier).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            LavaSearchPlugin::parse_request("album:Discovery").unwrap().kind,
            SearchKind::Albums
        );
        assert_eq!(
            LavaSearchPlugin::parse_request("artist:Daft Punk").unwrap().kind,
            SearchKind::Artists
        );
        assert_eq!(
            LavaSearchPlugin::parse_request("playlist:French house").unwrap().kind,
            SearchKind::Playlists
        );
    }

    #[test]
    fn rejects_unknown_or_empty_searches() {
        assert!(LavaSearchPlugin::parse_request("unknown:test").is_none());
        assert!(LavaSearchPlugin::parse_request("ytsearch: ").is_none());
    }
}
