//! Native YouTube plugin identity and identifier parsing.
//!
//! Network extraction is kept behind this crate's boundary so player/server
//! code does not need to know YouTube URL variants.

use ravalink_plugin_api::{PluginDescriptor, RavaLinkPlugin};

#[derive(Debug, Clone, Copy, Default)]
pub struct YouTubePlugin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeIdentifier {
    pub video_id: Option<String>,
    pub playlist_id: Option<String>,
    pub start_seconds: Option<u64>,
}

impl YouTubePlugin {
    pub fn parse_identifier(identifier: &str) -> Option<YouTubeIdentifier> {
        let trimmed = identifier.trim();
        let url = parse_url(trimmed)?;
        let mut video_id = None;

        match url.host.as_str() {
            "youtu.be" | "www.youtu.be" => {
                video_id = url.path.split('/').find(|part| !part.is_empty()).map(str::to_owned);
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

        let playlist_id = url.query_value("list").map(str::to_owned);
        let start_seconds = url
            .query_value("t")
            .and_then(|value| value.trim_end_matches('s').parse::<u64>().ok())
            .or_else(|| url.query_value("start").and_then(|value| value.parse().ok()));

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
            .filter(|query| !query.is_empty())
    }
}

impl RavaLinkPlugin for YouTubePlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "youtube",
            version: env!("CARGO_PKG_VERSION"),
            description: "YouTube video, playlist and search source",
            capabilities: &["video", "playlist", "search", "music-search"],
        }
    }

    fn supports_identifier(&self, identifier: &str) -> bool {
        Self::parse_identifier(identifier).is_some() || Self::is_search(identifier)
    }
}

fn valid_video_id(value: &str) -> bool {
    value.len() == 11
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
    let authority_end = rest
        .find(|character| matches!(character, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let suffix = &rest[authority_end..];
    let path_end = suffix
        .find(|character| matches!(character, '?' | '#'))
        .unwrap_or(suffix.len());
    let path = if path_end == 0 { "/" } else { &suffix[..path_end] };
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

        let playlist = YouTubePlugin::parse_identifier(
            "https://www.youtube.com/playlist?list=PL123",
        )
        .unwrap();
        assert_eq!(playlist.video_id, None);
        assert_eq!(playlist.playlist_id.as_deref(), Some("PL123"));

        assert_eq!(
            YouTubePlugin::search_query("ytsearch:lofi hip hop"),
            Some("lofi hip hop")
        );
        assert!(YouTubePlugin.supports_identifier("ytmsearch:ambient"));
    }

    #[test]
    fn rejects_non_youtube_and_malformed_video_ids() {
        assert!(YouTubePlugin::parse_identifier("https://example.com/watch?v=123").is_none());
        assert!(YouTubePlugin::parse_identifier("https://youtube.com/watch?v=short").is_none());
    }
}
