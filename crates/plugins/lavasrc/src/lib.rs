//! Native LavaSrc-compatible provider identification.
//!
//! This crate owns service URL parsing and provider-specific identity. The
//! source adapters build on these identifiers for metadata and media resolution.

use ravalink_plugin_api::{PluginDescriptor, RavaLinkPlugin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicProvider {
    Spotify,
    AppleMusic,
    Deezer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIdentifier {
    pub provider: MusicProvider,
    pub content_type: String,
    pub id: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LavaSrcPlugin;

impl LavaSrcPlugin {
    pub fn parse_identifier(identifier: &str) -> Option<ProviderIdentifier> {
        let (host, path) = split_url(identifier)?;
        let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let provider = match host.as_str() {
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
                let normalized_kind = if kind == "song" { "track" } else { kind };
                (normalized_kind, segments.last()?.trim_start_matches("id"))
            }
            MusicProvider::Deezer => {
                let kind = *segments.first()?;
                let id = *segments.get(1)?;
                (kind, id)
            }
        };

        if !matches!(content_type, "track" | "album" | "playlist" | "artist")
            || id.is_empty()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return None;
        }

        Some(ProviderIdentifier {
            provider,
            content_type: content_type.to_owned(),
            id: id.to_owned(),
        })
    }

    pub fn provider_for_identifier(identifier: &str) -> Option<MusicProvider> {
        Self::parse_identifier(identifier).map(|parsed| parsed.provider)
    }
}

impl RavaLinkPlugin for LavaSrcPlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "lavasrc",
            version: env!("CARGO_PKG_VERSION"),
            description: "Spotify, Apple Music and Deezer source integration",
            capabilities: &[
                "spotify",
                "apple-music",
                "deezer",
                "track",
                "album",
                "artist",
                "playlist",
            ],
        }
    }

    fn supports_identifier(&self, identifier: &str) -> bool {
        Self::parse_identifier(identifier).is_some()
    }
}

fn split_url(value: &str) -> Option<(String, &str)> {
    let (_, rest) = value.trim().split_once("://")?;
    let end = rest
        .find(|character| matches!(character, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    let host = rest[..end]
        .split('@')
        .next_back()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let suffix = &rest[end..];
    let path_end = suffix
        .find(|character| matches!(character, '?' | '#'))
        .unwrap_or(suffix.len());
    Some((host, &suffix[..path_end]))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parses_apple_music_and_deezer_links() {
        let apple = LavaSrcPlugin::parse_identifier(
            "https://music.apple.com/fr/album/test/123456789?i=987654321",
        )
        .unwrap();
        assert_eq!(apple.provider, MusicProvider::AppleMusic);
        assert_eq!(apple.content_type, "album");
        assert_eq!(apple.id, "123456789");

        let deezer =
            LavaSrcPlugin::parse_identifier("https://www.deezer.com/track/123456").unwrap();
        assert_eq!(deezer.provider, MusicProvider::Deezer);
        assert_eq!(deezer.id, "123456");
    }

    #[test]
    fn rejects_unknown_hosts_and_content_types() {
        assert!(LavaSrcPlugin::parse_identifier("https://example.com/track/123").is_none());
        assert!(LavaSrcPlugin::parse_identifier("https://open.spotify.com/show/123").is_none());
    }
}
