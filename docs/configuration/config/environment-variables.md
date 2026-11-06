---
description: Complete environment variable reference for RavaLink.
---

# Environment variables

Environment variables use the `RAVALINK_` prefix and override matching values loaded from TOML. Provider-specific integrations may also read their own variables without this prefix.

## Server

| Variable | Type | Default | Description |
|---|---|---|---|
| `RAVALINK_CONFIG` | Path | `config/ravalink.toml` | Explicit TOML configuration path. If set, the file must exist. |
| `RAVALINK_ADDRESS` | String | `0.0.0.0` | HTTP/WebSocket bind address |
| `RAVALINK_PORT` | Integer | `2333` | HTTP/WebSocket port (1–65535) |
| `RAVALINK_PASSWORD` | String | None | Required node password; minimum 16 characters and must not be an example value |
| `RAVALINK_PLAYER_UPDATE_INTERVAL` | Integer | `5` | Player update interval in seconds; must be greater than zero |
| `RAVALINK_LOG_LEVEL` | String | `info` | Tracing filter, typically `trace`, `debug`, `info`, `warn` or `error` |

## Audio and memory limits

All byte limits are decimal integer environment values representing bytes.

| Variable | Type | Default | Description |
|---|---|---:|---|
| `RAVALINK_AUDIO_BUFFER_DURATION_MS` | Integer | `400` | Audio buffer duration in milliseconds |
| `RAVALINK_AUDIO_FRAME_BUFFER_DURATION_MS` | Integer | `5000` | Frame buffer duration in milliseconds |
| `RAVALINK_AUDIO_OPUS_ENCODING_QUALITY` | Integer | `10` | Opus encoder quality, from 0 to 10 |
| `RAVALINK_AUDIO_RESAMPLING_QUALITY` | String | `low` | Resampling quality setting |
| `RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES` | Integer | `134217728` | Maximum compressed source size accepted by the buffered HTTP resolver |
| `RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES` | Integer | `268435456` | Maximum compressed source bytes retained by the pending playback cache (legacy setting name) |
| `RAVALINK_AUDIO_MAX_ACTIVE_DECODED_PCM_BYTES` | Integer | `268435456` | Aggregate decoded PCM memory budget for active players |

Set resource limits according to available RAM and concurrent players. Lower limits reduce worst-case memory use but can reject larger media or reduce buffering headroom.

## Plugin discovery

| Variable | Type | Default | Description |
|---|---|---|---|
| `RAVALINK_PLUGINS_DIRECTORY` | Path | `plugins` | Directory containing one subdirectory per external plugin |

Plugin discovery occurs at startup. Restart RavaLink after changing manifests or executables.

## YouTube authentication

| Variable | Type | Default | Description |
|---|---|---|---|
| `RAVALINK_YOUTUBE_COOKIES_FILE` | Path | Disabled | Optional Netscape-format browser cookies file |
| `RAVALINK_YOUTUBE_OAUTH_CLIENT_ID` | String | Disabled | Google OAuth client ID for Device Authorization Flow |
| `RAVALINK_YOUTUBE_OAUTH_TOKEN_FILE` | Path | Disabled | File used to persist OAuth access and refresh tokens |

OAuth client ID and token file must be configured together. Use a Google OAuth client type intended for TVs or limited-input devices. On first startup, follow the verification URL and code printed in the server logs. The refresh token is sensitive and must be stored in a directory writable by the RavaLink process.

Cookies and OAuth can enable authenticated requests, but they do not guarantee access to every restricted video. Treat both files as credentials.

## Prometheus

| Variable | Type | Default | Description |
|---|---|---|---|
| `RAVALINK_METRICS_PROMETHEUS_ENABLED` | Boolean | `false` | Enable the Prometheus endpoint |
| `RAVALINK_METRICS_PROMETHEUS_ENDPOINT` | Path | `/metrics` | Absolute endpoint path |

The metrics endpoint does not have independent authentication. Restrict it to trusted monitoring infrastructure.

## Provider credentials

These provider variables are read by the built-in integrations:

| Variable | Required for | Description |
|---|---|---|
| `RAVALINK_SPOTIFY_CLIENT_ID` | Spotify API-backed search and collection expansion | Spotify developer application client ID |
| `RAVALINK_SPOTIFY_CLIENT_SECRET` | Spotify API-backed search and collection expansion | Spotify developer application client secret |
| `RAVALINK_APPLE_MUSIC_TOKEN` | Apple Music playlist/catalog operations that require a developer token | Apple Music developer token |

Do not commit provider credentials to TOML files or public plugin manifests. Prefer panel variables, environment files with restricted permissions or a secrets manager.

## Example

```bash
export RAVALINK_ADDRESS=0.0.0.0
export RAVALINK_PORT=2333
export RAVALINK_PASSWORD="$(openssl rand -hex 32)"
export RAVALINK_LOG_LEVEL=info
export RAVALINK_METRICS_PROMETHEUS_ENABLED=false
cargo run --release -p ravalink-server
```

See the [TOML configuration reference](file.md), [Docker deployment](../../getting-started/docker.md) and [panel deployment guide](../../deployment/pelican-pterodactyl.md).
