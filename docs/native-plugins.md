# Native Rust plugins

RavaLink's built-in plugin implementations live in the same Cargo workspace as the
server. They are separate crates and share the `ravalink-plugin-api` contract.

| Crate | Lavalink equivalent | Current implementation |
|---|---|---|
| `ravalink-plugin-youtube` | YouTube Plugin | Invidious-compatible video metadata, AAC audio-format selection, timestamped starts, video search and playlist expansion |
| `ravalink-plugin-lavasrc` | LavaSrc | Spotify, Apple Music and Deezer track metadata with YouTube audio matching; Deezer album, playlist and artist-top expansion |
| `ravalink-plugin-lavasearch` | LavaSearch | YouTube, Spotify, Apple Music and Deezer track search; Deezer album, artist and playlist discovery |

The crates are separate so they can be developed, tested, configured and eventually
distributed independently. They do not load Java `.jar` files and do not require a JVM.

## YouTube

The YouTube plugin uses an Invidious-compatible API. By default it connects to
`https://inv.nadeko.net`; override this with `RAVALINK_YOUTUBE_API_BASE` if needed.
It resolves video metadata and an audio stream URL, expands playlists, and supports
`ytsearch:` and `ytmsearch:`. When an AAC audio format is available it is preferred
over Opus/WebM so the current Symphonia decoder can process the stream.

## LavaSrc

Track links from Spotify, Apple Music and Deezer are resolved to provider metadata,
then matched against YouTube for a playable audio stream. Track identity and metadata
remain associated with the original provider in the returned Lavalink track. Deezer
albums, playlists and artist top tracks are expanded into playable tracks. Spotify and
Apple Music collection expansion remain to be implemented.

## LavaSearch

Track search is available through `spsearch:`, `amsearch:`, `dzsearch:` and the
generic `search:` prefix. Search results are resolved to playable provider tracks.
Album, artist and playlist discovery currently uses Deezer's public search API.

Spotify search requires a Spotify developer application's client credentials:
`RAVALINK_SPOTIFY_CLIENT_ID` and `RAVALINK_SPOTIFY_CLIENT_SECRET`.

## Current playback limitation

Resolved media is currently downloaded and decoded through the bounded HTTP source
pipeline. This is a functional initial playback path, not yet a production-grade
segmented/live streaming implementation. Availability depends on the configured
Invidious instance and each provider's upstream behavior.
