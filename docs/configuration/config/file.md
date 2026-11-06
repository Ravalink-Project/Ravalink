---
description: TOML configuration reference for RavaLink.
---

# TOML configuration

RavaLink uses TOML, not the Java Lavalink `application.yml` format.

The default path is `config/ravalink.toml`, relative to the server working directory. Set `RAVALINK_CONFIG` to use a different path. If `RAVALINK_CONFIG` is explicitly set and the file does not exist or is invalid, startup fails.

Start with [config/ravalink.example.toml](https://github.com/Ravalink-Project/Ravalink/blob/master/config/ravalink.example.toml).

## Complete example

```toml
[server]
address = "0.0.0.0"
port = 2333
password = "replace-with-a-long-random-secret"
player_update_interval = 5

[logging]
level = "info"

[audio]
buffer_duration_ms = 400
frame_buffer_duration_ms = 5000
opus_encoding_quality = 10
resampling_quality = "low"
max_source_download_bytes = 134217728
max_decoded_cache_bytes = 268435456
max_active_decoded_pcm_bytes = 268435456

[youtube]
# Optional; protect these files like credentials.
# cookies_file = "/home/container/config/youtube-cookies.txt"
# oauth_client_id = "GOOGLE_OAUTH_CLIENT_ID"
# oauth_token_file = "/home/container/config/youtube-oauth-token.json"

[metrics]
prometheus_enabled = false
prometheus_endpoint = "/metrics"

plugin_directory = "plugins"

# Optional external process plugin.
# [[plugins]]
# name = "community-source"
# version = "1.0.0"
# description = "Community source integration"
# capabilities = ["track", "playlist", "search"]
# identifier_prefix = "community:"
# program = "/opt/ravalink/plugins/community-source"
# args = ["--stdio"]
#
# [plugins.environment]
# API_TOKEN = "inject-at-deployment-time"
#
# [plugins.config]
# region = "eu"
```

## Section reference

### `[server]`

| Key | Default | Description |
|---|---:|---|
| `address` | `0.0.0.0` | HTTP/WebSocket bind address |
| `port` | `2333` | HTTP/WebSocket port |
| `password` | Empty (invalid at startup) | Node authentication secret; minimum 16 characters |
| `player_update_interval` | `5` | Player update interval in seconds; must be greater than zero |

### `[logging]`

| Key | Default | Description |
|---|---|---|
| `level` | `info` | Tracing filter, such as `error`, `warn`, `info`, `debug` or `trace` |

### `[audio]`

| Key | Default | Description |
|---|---:|---|
| `buffer_duration_ms` | `400` | Audio buffer duration |
| `frame_buffer_duration_ms` | `5000` | Frame buffer duration |
| `opus_encoding_quality` | `10` | Opus encoding quality, 0–10 |
| `resampling_quality` | `low` | Resampling quality setting |
| `max_source_download_bytes` | `134217728` | Maximum compressed source size accepted by the buffered HTTP resolver |
| `max_decoded_cache_bytes` | `268435456` | Maximum compressed source bytes retained by the pending playback cache; name retained for compatibility |
| `max_active_decoded_pcm_bytes` | `268435456` | Aggregate decoded PCM memory budget for active players |

All limits must be greater than zero. Increasing limits can increase worst-case memory use; tune them against available RAM and expected concurrency.

### `[youtube]`

| Key | Default | Description |
|---|---|---|
| `cookies_file` | Disabled | Optional Netscape-format browser cookies file |
| `oauth_client_id` | Disabled | Google OAuth client ID for Device Authorization Flow |
| `oauth_token_file` | Disabled | Path where access/refresh tokens are persisted |

OAuth client ID and token file must be configured together. On first startup, complete the device authorization flow printed in the logs. The token file is sensitive and its parent directory must be writable by the server process.

### `[metrics]`

| Key | Default | Description |
|---|---|---|
| `prometheus_enabled` | `false` | Enable the Prometheus endpoint |
| `prometheus_endpoint` | `/metrics` | Absolute URL path for metrics |

The endpoint is unauthenticated. Restrict it to trusted monitoring clients.

### Plugin discovery

`plugin_directory` is a top-level setting (not inside a section). It defaults to `plugins` and points to the directory containing community plugin subdirectories.

External plugins can alternatively be declared using one or more `[[plugins]]` tables. See [Plugin installation and development](../../plugins/index.md) for the manifest, protocol, limits and security model.

## Environment overrides

Environment variables override TOML values. The complete list, including provider credentials, is in the [environment variable reference](environment-variables.md).

## Security checklist

- Replace the example password before starting the server.
- Keep the password, cookies, OAuth tokens and provider credentials out of Git.
- Restrict permissions on configuration and token files.
- Do not expose unauthenticated Prometheus metrics to the public internet.
- Install only trusted external plugin executables.
