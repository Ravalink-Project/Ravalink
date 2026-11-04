# Lavalink v4 compatibility ledger

This file is the acceptance ledger for the native rewrite. A route marked
"scaffold" is not yet feature-complete and must not be advertised as compatible.

| Area | Required behavior | Current state |
|---|---|---|
| Authentication | Authorization header on REST and WebSocket | Initial implementation |
| Version | GET /version | Initial implementation |
| Server info | GET /v4/info with Lavalink v4 field names | Initial implementation; advertises implemented DSP filters |
| Statistics | GET /v4/stats | Initial implementation; CPU load pending |
| Load tracks | GET /v4/loadtracks | Direct HTTP(S) files plus built-in YouTube, Spotify, Apple Music and Deezer resolution/search; YouTube audio matching supplies playable streams for protected catalog providers; live streams and continuous streaming decode remain pending |
| Decode track | GET /v4/decodetrack | Lavaplayer track-info v1-v3 decoding; known built-in source payloads supported |
| Decode tracks | POST /v4/decodetracks | Batch decode using the same codec |
| WebSocket | User-Id, Client-Name, Session-Id headers, ready, stats, player updates and track events | Handshake, session resumption, periodic stats and player-position updates are implemented; TrackStart and TrackEnd (finished, replaced, stopped, cleanup) carry full Track objects and are emitted by the native player lifecycle; TrackException carries the active Track and is emitted for native audio packet-encoding failures; TrackStuck is emitted once after 10 seconds without playback-position progress and resets when playback resumes; gateway reconnection and multi-client edge cases still need parity tests |
| Session update | PATCH /v4/sessions/{sessionId} | Initial implementation |
| Route planner | GET /v4/routeplanner/status, POST /free/address, POST /free/all | Disabled-state semantics implemented: status returns 204; free operations return 500 until a native route planner is available |
| List players | GET /v4/sessions/{sessionId}/players | In-memory player state and per-session/guild native audio-player contexts |
| Get player | GET /v4/sessions/{sessionId}/players/{guildId} | In-memory player state lookup |
| Update player | PATCH /v4/sessions/{sessionId}/players/{guildId} | Direct HTTP and enabled native-plugin track resolution load decoded PCM into NativePlayer; volume, filters, pause, seek, stop, replacement and end-time controls update native state; Discord voice output is connected to the native gateway driver; remaining edge cases require client-level acceptance tests |
| Delete player | DELETE /v4/sessions/{sessionId}/players/{guildId} | Removes REST and native audio-player state |
| Audio sources | Search, resolve, stream, errors | Built-in YouTube, LavaSrc and LavaSearch providers plus a bounded direct HTTP(S) file resolver; configurable download cap (128 MiB default), pending decoded-PCM cache (256 MiB default) and aggregate active-player PCM budget (256 MiB default); media is buffered before decode, so live streams and long sources are not yet supported |
| Audio engine | Demux, decode, seek, filters, Opus | Symphonia in-memory demux/decode with packet-at-a-time callback and bounded Tokio-channel decoder; REST playback feeds NativePlayer through a bounded 8-frame PCM queue with underrun handling and producer backpressure; stateful PCM DSP; bounded HTTP download and decoded-PCM cache; 48 kHz stereo resampling and 20 ms packetization; native Opus encoding; RTP v2 sequence/timestamp construction |
| Discord voice | Gateway voice handshake, DAVE, RTP, encryption | Native Voice Gateway v8 identify/resume, heartbeat, UDP IP discovery, encryption-mode negotiation, speaking state, DAVE MLS event handling, DAVE Opus encryption, RTP transport encryption, UDP keepalive and 20 ms audio output are wired together; automated protocol tests pass, but live Discord guild playback and reconnect behavior still require end-to-end validation |
| Plugins | Native extension API | Rust plugin contract and built-in YouTube, LavaSrc and LavaSearch crates are implemented and registered statically; external dynamic plugin discovery/loading and a stable third-party ABI are not implemented |
| Prometheus | Configurable metrics endpoint | Optional unauthenticated Prometheus exposition endpoint; uptime, sessions, players, playing players, pending decoded-cache bytes and active-player PCM usage/limit gauges |

## Acceptance rule

A row moves to "compatible" only after automated tests cover both successful
and failure paths and the same test scenario has been checked against Lavalink v4.


## Track codec limitations

The native HTTP source path still buffers a complete direct HTTP(S) file before playback. The codec layer now exposes `decode_bytes_with`, which transfers each decoded packet to a caller-owned sink and enforces a cumulative PCM budget without collecting the whole decoded track. The bounded-channel decoder is now available as a reusable primitive, but the HTTP source adapter and NativePlayer are not yet wired through it. This is not yet end-to-end continuous or live streaming. Large/live source support remains pending.

The playback queue is bounded and consumed directly by NativePlayer. Its
producer currently owns the already-decoded frame vector, so this separates
player-side buffering and provides backpressure at the playback boundary but
does not reduce the total PCM retained by the process during decoding. The HTTP
source adapter still downloads and decodes the complete file before playback.
Non-zero initial positions use the seekable decoded-frame path; live HTTP
sources and end-to-end incremental network decoding remain unsupported.

The codec reads Lavaplayer track-info versions 1 through 3 and preserves the
original encoded string. It supports built-in sources whose track payloads are
known to contain no extra data, plus the HTTP/local probing descriptor. Plugin
source managers with custom binary payloads are rejected explicitly rather
than decoded incorrectly.
