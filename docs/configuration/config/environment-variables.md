---
description: Configure RavaLink using environment variables.
---

# Environment variables

RavaLink environment variables use the RAVALINK_ prefix. They override values loaded from the TOML configuration file.

| Variable | Description |
|---|---|
| RAVALINK_CONFIG | Explicit TOML configuration path |
| RAVALINK_ADDRESS | HTTP bind address |
| RAVALINK_PORT | HTTP/WebSocket port |
| RAVALINK_PASSWORD | Node authentication secret (minimum 16 characters) |
| RAVALINK_PLAYER_UPDATE_INTERVAL | Player position update interval in seconds |
| RAVALINK_LOG_LEVEL | Log filter, such as info, debug or trace |
| RAVALINK_AUDIO_BUFFER_DURATION_MS | Audio buffer duration |
| RAVALINK_AUDIO_FRAME_BUFFER_DURATION_MS | Audio frame buffer duration |
| RAVALINK_AUDIO_OPUS_ENCODING_QUALITY | Opus quality, from 0 to 10 |
| RAVALINK_AUDIO_RESAMPLING_QUALITY | Resampling quality setting |
| RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES | Maximum compressed source download size |
| RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES | Compressed source cache/seek budget |
| RAVALINK_AUDIO_MAX_ACTIVE_DECODED_PCM_BYTES | Aggregate decoded PCM memory budget for active players |
| RAVALINK_PLUGINS_DIRECTORY | Directory used to discover community plugin manifests |
| RAVALINK_YOUTUBE_COOKIES_FILE | Optional Netscape-format YouTube browser cookies file |
| RAVALINK_METRICS_PROMETHEUS_ENABLED | Enable the Prometheus endpoint |
| RAVALINK_METRICS_PROMETHEUS_ENDPOINT | Prometheus endpoint path |

Provider integrations also read provider-specific environment variables:

- YouTube extraction is built in and uses multiple native InnerTube clients with local player-script signature deciphering. Optional browser cookies enable authenticated requests; no public Invidious instance is required.
- RAVALINK_SPOTIFY_CLIENT_ID
- RAVALINK_SPOTIFY_CLIENT_SECRET
- RAVALINK_APPLE_MUSIC_TOKEN

Use a secrets manager or a restricted environment file for credentials. Never commit real credentials to the repository.

For the complete default values and plugin examples, consult the [example TOML configuration](https://github.com/Ravalink-Project/Ravalink/blob/master/config/ravalink.example.toml).
