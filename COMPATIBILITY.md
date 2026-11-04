# Lavalink v4 compatibility ledger

This file is the acceptance ledger for the native rewrite. A route marked
"scaffold" is not yet feature-complete and must not be advertised as compatible.

| Area | Required behavior | Current state |
|---|---|---|
| Authentication | Authorization header on REST and WebSocket | Initial implementation |
| Version | GET /version | Initial implementation |
| Server info | GET /v4/info with Lavalink v4 field names | Initial implementation; advertises implemented DSP filters |
| Statistics | GET /v4/stats | System and process CPU loads are measured through sysinfo; node stats are cached for five seconds; frame stats are omitted when unavailable |
| Load tracks | GET /v4/loadtracks | Direct HTTP(S) files plus built-in YouTube, Spotify, Apple Music and Deezer resolution/search; YouTube audio matching supplies playable streams for protected catalog providers; live network streaming remains pending; buffered sources decode incrementally into a bounded PCM queue |
| Decode track | GET /v4/decodetrack | Lavaplayer track-info v1-v3 decoding; known built-in source payloads supported |
| Decode tracks | POST /v4/decodetracks | Batch decode using the same codec |
| WebSocket | User-Id, Client-Name, Session-Id headers, ready, stats, player updates and track events | Handshake, session resumption, periodic stats and player-position updates are implemented; TrackStart and TrackEnd (finished, replaced, stopped, cleanup) carry full Track objects and are emitted by the native player lifecycle; TrackException carries the active Track and is emitted for native audio decoding and packet-encoding failures; TrackStuck is emitted once after 10 seconds without playback-position progress and resets when playback resumes; gateway reconnect now retries session resume with cancellable exponential backoff capped at 30 seconds; multi-client edge cases and live Discord acceptance still need parity tests |
| Session update | PATCH /v4/sessions/{sessionId} | Initial implementation |
| Route planner | GET /v4/routeplanner/status, POST /free/address, POST /free/all | Disabled-state semantics implemented: status returns 204; free operations return 500 until a native route planner is available |
| List players | GET /v4/sessions/{sessionId}/players | In-memory player state and per-session/guild native audio-player contexts |
| Get player | GET /v4/sessions/{sessionId}/players/{guildId} | In-memory player state lookup |
| Update player | PATCH /v4/sessions/{sessionId}/players/{guildId} | Direct HTTP and enabled native-plugin track resolution feed compressed sources through a bounded PCM decoder into NativePlayer; initial and subsequent seeks restart decoding at the requested position while the compressed source remains within the aggregate seek-cache budget; seeking re-decodes from the beginning of the buffered file; volume, filters, pause, stop, replacement and end-time controls update native state; Discord voice output is connected to the native gateway driver; remaining edge cases require client-level acceptance tests |
| Delete player | DELETE /v4/sessions/{sessionId}/players/{guildId} | Removes REST and native audio-player state |
| Audio sources | Search, resolve, stream, errors | Built-in YouTube, LavaSrc and LavaSearch providers plus a bounded direct HTTP(S) file resolver; compressed source cache (256 MiB default), download cap (128 MiB default), a two-request concurrent resolution limit and a per-decoded-packet PCM allocation ceiling; direct files are still downloaded completely before playback, so live network streams are not yet supported |
| Audio engine | Demux, decode, seek, filters, Opus | Symphonia in-memory demux/decode with packet-at-a-time callback and bounded Tokio-channel decoder; REST playback feeds NativePlayer through a bounded 8-frame PCM queue with underrun handling and producer backpressure; stateful PCM DSP; bounded compressed-source cache and per-packet PCM allocation ceiling; 48 kHz stereo resampling and 20 ms packetization; native Opus encoding; RTP v2 sequence/timestamp construction |
| Discord voice | Gateway voice handshake, DAVE, RTP, encryption | Native Voice Gateway v8 identify/resume, heartbeat, UDP IP discovery, encryption-mode negotiation, speaking state, DAVE MLS event handling, DAVE Opus encryption, RTP transport encryption, UDP keepalive and 20 ms audio output are wired together; automated protocol tests pass, but live Discord guild playback and reconnect behavior still require end-to-end validation |
| Plugins | Native extension API | Rust plugin contract and built-in YouTube, LavaSrc and LavaSearch crates are implemented and registered statically; external dynamic plugin discovery/loading and a stable third-party ABI are not implemented |
| Prometheus | Configurable metrics endpoint | Optional unauthenticated Prometheus exposition endpoint; uptime, sessions, players, playing players, compressed source-cache bytes, retained seek-source bytes/limit and full-decode PCM usage/limit gauges |

## Acceptance rule

A row moves to "compatible" only after automated tests cover both successful
and failure paths and the same test scenario has been checked against Lavalink v4.


## Track codec limitations

The native HTTP source path still buffers a complete direct HTTP(S) file before playback. It no longer retains the complete decoded PCM track during resolution: it validates and measures the file in a blocking worker, discarding each decoded packet. At playback, a second decode pass feeds the bounded NativePlayer channel. This reduces retained PCM memory, but does not provide live network streaming or avoid the initial full-file download. Large/live source support remains pending.

The playback queue is bounded and consumed directly by NativePlayer. Direct HTTP and
plugin-resolved tracks are cached as compressed source bytes. Resolution performs
a validation/duration decode pass that discards PCM packets instead of retaining the
whole decoded track. Playback then decodes those bytes on a blocking worker into the
bounded eight-frame channel, applying backpressure as NativePlayer consumes packets.
Initial and subsequent positions use a bounded decoder that discards the compressed source's decoded prefix without retaining it. The HTTP response itself is still downloaded fully
before decoding; this is incremental PCM production from a buffered source, not live
network streaming. Subsequent seek requests for buffered direct HTTP and built-in
plugin sources restart the bounded decoder at the requested position while their
compressed source remains within the aggregate playback seek-cache budget. Seeking
re-decodes from the beginning of the compressed file; it is not a byte-range or live
network seek.

The codec reads Lavaplayer track-info versions 1 through 3 and preserves the
original encoded string. It supports built-in sources whose track payloads are
known to contain no extra data, plus the HTTP/local probing descriptor. Plugin
source managers with custom binary payloads are rejected explicitly rather
than decoded incorrectly.
