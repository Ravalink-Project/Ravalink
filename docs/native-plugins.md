# Native Rust plugins

RavaLink's built-in plugin implementations live in the same Cargo workspace as the
server. They are separate crates and share the `ravalink-plugin-api` contract.

| Crate | Lavalink equivalent | Current implementation |
|---|---|---|
| `ravalink-plugin-youtube` | YouTube Plugin | Invidious-compatible video metadata, audio-format resolution, video search and playlist expansion |
| `ravalink-plugin-lavasrc` | LavaSrc | Spotify, Apple Music and Deezer URL identification |
| `ravalink-plugin-lavasearch` | LavaSearch | Track, album, artist and playlist search syntax |

The crates are separated so they can be developed, tested, configured and eventually
distributed independently. They do not load Java `.jar` files and do not require a JVM.

## YouTube

The YouTube plugin uses an Invidious-compatible API. By default it connects to
`https://inv.nadeko.net`; override this with `RAVALINK_YOUTUBE_API_BASE` if needed.
It resolves video metadata and an audio stream URL, expands playlists, and supports
`ytsearch:` and `ytmsearch:`.

The current audio path downloads and decodes the resolved stream through the bounded
HTTP source pipeline. This is an initial functional path, not yet a production-grade
segmented/live streaming implementation. Availability depends on the configured
Invidious instance and YouTube's upstream behavior.

## Remaining plugin work

LavaSrc still needs provider metadata APIs, collection expansion, cross-provider
matching and playable-source resolution. LavaSearch still needs the actual search
providers and structured album/artist/playlist result builders. Their URL and query
contracts are in place, but those crates are not yet full replacements for their
Java counterparts.
