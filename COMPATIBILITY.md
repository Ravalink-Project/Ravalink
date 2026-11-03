# Lavalink v4 compatibility ledger

This file is the acceptance ledger for the native rewrite. A route marked
"scaffold" is not yet feature-complete and must not be advertised as compatible.

| Area | Required behavior | Current state |
|---|---|---|
| Authentication | Authorization header on REST and WebSocket | Initial implementation |
| Version | GET /version | Initial implementation |
| Server info | GET /v4/info with Lavalink v4 field names | Initial implementation; advertises implemented DSP filters |
| Statistics | GET /v4/stats | Initial implementation; CPU load pending |
| Load tracks | GET /v4/loadtracks | Route shape only; source adapters pending |
| Decode track | GET /v4/decodetrack | Lavaplayer track-info v1-v3 decoding; known built-in source payloads supported |
| Decode tracks | POST /v4/decodetracks | Batch decode using the same codec |
| WebSocket | User-Id, Client-Name, Session-Id headers and ready event | Initial handshake only |
| Session update | PATCH /v4/sessions/{sessionId} | Initial implementation |
| List players | GET /v4/sessions/{sessionId}/players | In-memory state store; no active playback contexts yet |
| Get player | GET /v4/sessions/{sessionId}/players/{guildId} | In-memory player state lookup |
| Update player | PATCH /v4/sessions/{sessionId}/players/{guildId} | Partial state updates; audio commands are not connected to playback |
| Delete player | DELETE /v4/sessions/{sessionId}/players/{guildId} | In-memory state removal |
| Audio sources | Search, resolve, stream, errors | Not implemented; `/v4/loadtracks` deliberately returns no matches |
| Audio engine | Demux, decode, seek, filters, Opus | Symphonia in-memory demux/decode; stateful PCM DSP and NativePlayer lifecycle; no continuous source streaming, REST-to-engine playback integration, time-stretch or Opus yet |
| Discord voice | Gateway voice handshake, DAVE, RTP, encryption | Not implemented |
| Plugins | Native extension API | Not implemented |
| Prometheus | Configurable metrics endpoint | Not implemented |

## Acceptance rule

A row moves to "compatible" only after automated tests cover both successful
and failure paths and the same test scenario has been checked against Lavalink v4.


## Track codec limitations

The codec reads Lavaplayer track-info versions 1 through 3 and preserves the
original encoded string. It supports built-in sources whose track payloads are
known to contain no extra data, plus the HTTP/local probing descriptor. Plugin
source managers with custom binary payloads are rejected explicitly rather
than decoded incorrectly.
