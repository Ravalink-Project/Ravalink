# Native Rust plugins

RavaLink's built-in plugin implementations live in the same Cargo workspace as the
server. They are separate crates and share the `ravalink-plugin-api` contract.

| Crate | Lavalink equivalent | Scope |
|---|---|---|
| `ravalink-plugin-youtube` | YouTube Plugin | YouTube video and playlist identifiers, YouTube search prefixes |
| `ravalink-plugin-lavasrc` | LavaSrc | Spotify, Apple Music and Deezer URL identification |
| `ravalink-plugin-lavasearch` | LavaSearch | Track, album, artist and playlist search request parsing |
 
The crates are separated so they can be developed, tested, configured and eventually
distributed independently. They do not load Java `.jar` files and do not require a JVM.

## Current implementation boundary

This first native-plugin layer establishes the workspace boundaries, shared plugin
metadata/registry contract, provider URL parsing and Lavalink search syntax. It is
not yet a claim of full source parity: provider authentication, metadata extraction,
stream URL resolution, playlist expansion, cross-provider matching and end-to-end
audio playback still need to be implemented and integrated with the server/audio
pipeline before these plugins can be considered complete replacements.
