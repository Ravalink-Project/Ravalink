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
| WebSocket | User-Id, Client-Name, Session-Id headers, ready, stats, player updates and track events | Handshake, session resumption, periodic stats and player-position updates are implemented; TrackStart and TrackEnd (finished, replaced, stopped, cleanup) carry full Track objects and are emitted by the native player lifecycle; TrackException and TrackStuck payload models carry Track objects but are not yet emitted by runtime error/stall detection; gateway reconnection and multi-client edge cases still need parity tests |
| Session update | PATCH /v4/sessions/{sessionId} | Initial implementation |
| Route planner | GET /v4/routeplanner/status, POST /free/address, POST /free/all | Disabled-state semantics implemented: status returns 204; free operations return 500 until a native route planner is available |
| List players | GET /v4/sessions/{sessionId}/players | In-memory player state and per-session/guild native audio-player contexts |
| Get player | GET /v4/sessions/{sessionId}/players/{guildId} | In-memory player state lookup |
| Update player | PATCH /v4/sessions/{sessionId}/players/{guildId} | Direct HTTP and enabled native-plugin track resolution load decoded PCM into NativePlayer; volume, filters, pause, seek, stop, replacement and end-time controls update native state; Discord voice output is connected to the native gateway driver; remaining edge cases require client-level acceptance tests |
| Delete player | DELETE /v4/sessions/{sessionId}/players/{guildId} | Removes REST and native audio-player state |
| Audio sources | Search, resolve, stream, errors | Built-in YouTube, LavaSrc and LavaSearch providers plus a bounded direct HTTP(S) file resolver; configurable download cap (128 MiB default), pending decoded-PCM cache (256 MiB default) and aggregate active-player PCM budget (256 MiB default); media is buffered before decode, so live streams and long sources are not yet supported |
| Audio engine | Demux, decode, seek, filters, Opus | Symphonia in-memory demux/decode; stateful PCM DSP and REST-to-NativePlayer loading/control; bounded HTTP download and decoded-PCM cache; 48 kHz stereo resampling and 20 ms packetization; native Opus encoding; RTP v2 sequence/timestamp construction |
| Discord voice | Gateway voice handshake, DAVE, RTP, encryption | Native Voice Gateway v8 identify/resume, heartbeat, UDP IP discovery, encryption-mode negotiation, speaking state, DAVE MLS event handling, DAVE Opus encryption, RTP transport encryption, UDP keepalive and 20 ms audio output are wired together; automated protocol tests pass, but live Discord guild playback and reconnect behavior still require end-to-end validation |
| Plugins | Native extension API | Rust plugin contract and built-in YouTube, LavaSrc and LavaSearch crates are implemented and registered statically; external dynamic plugin discovery/loading and a stable third-party ABI are not implemented |
| Prometheus | Configurable metrics endpoint | Optional unauthenticated Prometheus exposition endpoint; uptime, sessions, players, playing players, pending decoded-cache bytes and active-player PCM usage/limit gauges |

## Acceptance rule

A row moves to "compatible" only after automated tests cover both successful
and failure paths and the same test scenario has been checked against Lavalink v4.


## Track codec limitations

The native audio path currently buffers and decodes a complete direct HTTP(S) file before playback. It is not a continuous streaming engine; large or live sources are intentionally unsupported until the streaming decoder and back-pressure path are implemented.\n\nThe codec reads Lavaplayer track-info versions 1 through 3 and preserves the
original encoded string. It supports built-in sources whose track payloads are
known to contain no extra data, plus the HTTP/local probing descriptor. Plugin
source managers with custom binary payloads are rejected explicitly rather
than decoded incorrectly.
