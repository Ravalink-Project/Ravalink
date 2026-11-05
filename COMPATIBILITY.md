# Lavalink v4 compatibility ledger

This file is the acceptance ledger for the native rewrite. A route marked
"scaffold" is not yet feature-complete and must not be advertised as compatible.

| Area | Required behavior | Current state |
|---|---|---|
| Authentication | Authorization header on REST and WebSocket | Initial implementation |
| Version | GET /version | Initial implementation |
| Server info | GET /v4/info with Lavalink v4 field names | Initial implementation; advertises implemented DSP filters |
| REST errors | Lavalink error response shape and optional `trace=true` stack trace | Standard error fields are implemented; the optional `trace` query parameter and trace field are not implemented yet |\n| Statistics | GET /v4/stats | System and process CPU loads are measured through sysinfo; node stats are cached for five seconds; frame stats are omitted when unavailable |
| Load tracks | GET /v4/loadtracks | Direct HTTP(S) files plus built-in YouTube, Spotify, Apple Music and Deezer resolution/search; YouTube audio matching supplies playable streams for protected catalog providers; direct HTTP(S) sources and plugin stream URLs are opened at player start and decoded progressively through bounded queues; generic URL duration is unknown |
| Decode track | GET /v4/decodetrack | Lavaplayer track-info v1-v3 decoding; known built-in source payloads supported |
| Decode tracks | POST /v4/decodetracks | Batch decode using the same codec |
| WebSocket | User-Id, Client-Name, Session-Id headers, ready, stats, player updates and track events | Handshake, session resumption, periodic stats and player-position updates are implemented; TrackStart and TrackEnd (finished, replaced, stopped, cleanup) carry full Track objects and are emitted by the native player lifecycle; TrackException carries the active Track and is emitted for native audio decoding and packet-encoding failures; TrackStuck is emitted once after 10 seconds without playback-position progress and resets when playback resumes; gateway heartbeats are monitored for missing acknowledgements (two heartbeat intervals); reconnect retries session resume with cancellable exponential backoff capped at 30 seconds and a 15-second connect/handshake deadline; multi-client edge cases and live Discord acceptance still need parity tests |
| Session update | PATCH /v4/sessions/{sessionId} | Initial implementation |
| Route planner | GET /v4/routeplanner/status, POST /free/address, POST /free/all | Disabled-state semantics implemented: status returns 204; free operations return 500 until a native route planner is available |
| List players | GET /v4/sessions/{sessionId}/players | In-memory player state and per-session/guild native audio-player contexts |
| Get player | GET /v4/sessions/{sessionId}/players/{guildId} | In-memory player state lookup |
| Update player | PATCH /v4/sessions/{sessionId}/players/{guildId} | Direct HTTP and enabled native-plugin track resolution feed compressed sources through a bounded PCM decoder into NativePlayer; initial and subsequent seeks restart decoding at the requested position while the compressed source remains within the aggregate seek-cache budget; seeking re-decodes from the beginning of the buffered file; volume, implemented PCM filters, pause, stop, replacement and end-time controls update native state; the timescale filter is implemented in the voice output path with streaming WSOLA tempo processing and pitch/rate resampling (effective tempo and pitch/rate ratios are currently limited to 0.25–4.0); plugin-specific filters are rejected explicitly; Discord voice output is connected to the native gateway driver; remaining edge cases require client-level acceptance tests |
| Delete player | DELETE /v4/sessions/{sessionId}/players/{guildId} | Removes REST and native audio-player state |
| Audio sources | Search, resolve, stream, errors | Built-in YouTube, LavaSrc and LavaSearch providers; legacy buffered helper limits (256 MiB cache, 128 MiB download cap), two concurrent metadata resolutions, up to 128 active progressive streams and a 4 MiB per-decoded-packet PCM allocation ceiling; direct HTTP(S) and plugin URLs use a bounded four-chunk compressed-byte bridge and decode while the response is still arriving; no complete compressed file or decoded PCM track is retained; generic URL duration is unknown and progressive HTTP streams cannot be sought after playback begins; initial timestamp offsets are honored by decoding and discarding the prefix; DNS resolution is bounded to five seconds and pinned once per streaming client, embedded URL credentials are rejected, response headers have a 15-second deadline, and stream connections have a 60-second idle timeout |
| Audio engine | Demux, decode, seek, filters, Opus | Timescale uses a streaming WSOLA processor for pitch-preserving speed changes, with sample-rate conversion for independent pitch and rate controls; the effective tempo and pitch/rate ratios are validated to the supported 0.25–4.0 range. Unknown plugin filters are preserved by the protocol model but rejected by the player until a native filter handler exists. Symphonia in-memory demux/decode with packet-at-a-time callback and bounded Tokio-channel decoder; REST playback feeds NativePlayer through a bounded 8-frame PCM queue with underrun handling and producer backpressure; stateful PCM DSP; bounded compressed-source cache and per-packet PCM allocation ceiling; 48 kHz stereo resampling and 20 ms packetization; native Opus encoding; RTP v2 sequence/timestamp construction |
| Discord voice | Gateway voice handshake, DAVE, RTP, encryption | Native Voice Gateway v8 identify/resume, heartbeat, UDP IP discovery, encryption-mode negotiation, speaking state, DAVE MLS event handling, DAVE Opus encryption, RTP transport encryption, UDP keepalive and 20 ms audio output are wired together; automated protocol tests pass, but live Discord guild playback and reconnect behavior still require end-to-end validation |
| Plugins | Native extension API | Rust plugin contract and built-in YouTube, LavaSrc and LavaSearch crates are implemented; optional external Rust extensions can be registered as short-lived child processes through a versioned JSON-over-stdio protocol, with an allowlisted environment, a 4 MiB response cap, two concurrent invocations per extension and a 30-second per-plugin timeout; in-process dynamic library loading and a stable binary ABI are intentionally not provided |
| Prometheus | Configurable metrics endpoint | Optional unauthenticated Prometheus exposition endpoint; uptime, sessions, players, playing players, compressed source-cache bytes, retained seek-source bytes/limit and full-decode PCM usage/limit gauges |

The native release smoke test exercises the Lavalink v4 version string, authentication failures, required info/stats fields, omission of frameStats, route-planner disabled-state responses, missing-input decode errors, unsupported-source empty results, missing-session error serialization, and disabled metrics routing. This is a server smoke test, not a substitute for the full REST/WebSocket contract suite or upstream differential tests.

External process extensions use the contract documented in [External Rust extensions](docs/plugins/external-process.md). They run as the RavaLink service account and are trusted code, not sandboxed workloads.

## Acceptance rule

A row moves to "compatible" only after automated tests cover both successful
and failure paths and the same test scenario has been checked against Lavalink v4.


## Track codec limitations

The direct HTTP(S) playback path now opens the response when the player starts and forwards chunks through a bounded four-item compressed-byte channel into Symphonia's non-seekable demuxer. Decoded PCM is forwarded through the existing bounded eight-frame player queue, so backpressure reaches the HTTP reader and memory use does not scale with track duration. Generic HTTP tracks report unknown duration and are not seekable. Containers that require random access (notably non-fragmented MP4) may still fail in this progressive path and need a range-aware seekable implementation.

The playback queue is bounded and consumed directly by NativePlayer. Progressive
HTTP and plugin streams are not cached as complete compressed files: the server
opens the HTTP response on player start, forwards at most four compressed chunks
to the demuxer, and forwards decoded PCM into the bounded eight-frame queue.
Backpressure propagates from NativePlayer to the decoder and HTTP producer.
Initial timestamp offsets are handled by decoding and discarding the prefix.
Subsequent seeking is unavailable because the source is non-seekable. The legacy
buffered decode helper remains available internally for compatibility tests and
specialized callers, but it is no longer the REST player's default path.

The progressive HTTP admission policy limits metadata probes to two concurrent
requests (each waiting at most five seconds) and active media streams to 128.
The compressed bridge has four queue slots; each queued chunk is split to at
most 64 KiB before enqueueing. This bounds queued compressed data independently
of track duration, while the decoder's PCM queue applies downstream backpressure.

The codec reads Lavaplayer track-info versions 1 through 3 and preserves the
original encoded string. It supports built-in sources whose track payloads are
known to contain no extra data, plus the HTTP/local probing descriptor. Plugin
source managers with custom binary payloads are rejected explicitly rather
than decoded incorrectly.
