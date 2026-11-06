---
description: Install and operate RavaLink with the official Pelican or Pterodactyl egg.
---

# Pelican and Pterodactyl

The official egg runs the published RavaLink container image directly. It does not compile the project during installation and does not download a binary every time the server starts.

## Choose the correct egg

Download the panel-specific export from [RavaLink-eggs](https://github.com/Ravalink-Project/Ravalink-eggs):

| Panel | Egg file | Format |
|---|---|---|
| Pelican | `egg-ravalink-pelican.json` | `PLCN_v3` |
| Pterodactyl | `egg-ravalink.json` | `PTDL_v2` |

Do not import the Pterodactyl export into Pelican when the native Pelican export is available.

## Create the server

1. Import the correct egg into the panel.
2. Create a server using the **RavaLink official** Docker image.
3. Allocate a TCP port for RavaLink. The default is `2333`.
4. Set `RAVALINK_PASSWORD` to a unique random secret (at least 16 characters).
5. Keep `RAVALINK_ADDRESS` at `0.0.0.0` inside the container.
6. Start the server and inspect the console.

The configured allocation must match `RAVALINK_PORT`. The node listens on that port for both REST and WebSocket traffic.

## Image and startup contract

Official image:

```text
ghcr.io/ravalink-project/ravalink:latest
```

The image is published for `linux/amd64` and `linux/arm64`. Its entrypoint is designed for Pelican/Wings and Pterodactyl/Wings:

- It runs as the unprivileged `container` user (UID/GID 1000).
- Its working directory is `/home/container`.
- It reads the panel-provided `STARTUP` command and expands panel placeholders.
- The egg's default startup command is `ravalink`.
- It exposes TCP port `2333`.

The image package on GHCR must be public for Wings to pull it without registry credentials. No install script, Java runtime, Rust toolchain or panel-side build step is required.

## Egg variables

The official egg exposes the following runtime variables. The panel passes them to the container as environment variables.

| Variable | Default | Purpose |
|---|---:|---|
| `RAVALINK_PORT` | `2333` | HTTP and WebSocket listen port; match the allocation |
| `RAVALINK_ADDRESS` | `0.0.0.0` | Bind address inside the container; keep unchanged |
| `RAVALINK_PASSWORD` | Must be changed | Node authentication secret, minimum 16 characters |
| `RAVALINK_LOG_LEVEL` | `info` | `trace`, `debug`, `info`, `warn` or `error` |
| `RAVALINK_PLAYER_UPDATE_INTERVAL` | `5` | Player position update interval in seconds |
| `RAVALINK_AUDIO_BUFFER_DURATION_MS` | `400` | Audio buffer duration |
| `RAVALINK_AUDIO_FRAME_BUFFER_DURATION_MS` | `5000` | Frame buffer duration |
| `RAVALINK_AUDIO_OPUS_ENCODING_QUALITY` | `10` | Opus encoder quality, 0–10 |
| `RAVALINK_AUDIO_RESAMPLING_QUALITY` | `low` | Resampling quality |
| `RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES` | `134217728` | Maximum compressed source size accepted by the buffered resolver |
| `RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES` | `268435456` | Maximum compressed source cache budget |
| `RAVALINK_AUDIO_MAX_ACTIVE_DECODED_PCM_BYTES` | `268435456` | Aggregate decoded PCM budget for active players |
| `RAVALINK_METRICS_PROMETHEUS_ENABLED` | `false` | Enable the Prometheus endpoint |
| `RAVALINK_METRICS_PROMETHEUS_ENDPOINT` | `/metrics` | Metrics HTTP path |

Additional environment variables supported by the server are documented in [Environment variables](../configuration/config/environment-variables.md). They can be added to the panel's server variables/environment when needed.

## Configuration file

A TOML file is optional. Without one, RavaLink uses its built-in defaults and the panel variables.

To use a file:

1. Create `/home/container/config/ravalink.toml` in the server's file manager.
2. Start from [ravalink.example.toml](https://github.com/Ravalink-Project/Ravalink/blob/master/config/ravalink.example.toml).
3. Replace the example password with a real secret, or leave the password value to be overridden by `RAVALINK_PASSWORD`.
4. Restart the server.

The default config lookup is `config/ravalink.toml`, relative to `/home/container`. Set `RAVALINK_CONFIG` only if you need a different path; when explicitly set, the file must exist.

Environment variables override TOML values. This is useful for panel-managed secrets: keep the password in the panel variable and keep non-secret tuning options in the TOML file.

## Installing community plugins

Create one folder per plugin under `/home/container/plugins` (or set `RAVALINK_PLUGINS_DIRECTORY` to another path). Each folder must contain:

- `ravalink-plugin.toml`
- The plugin executable at the path declared by `program`

For example:

```text
/home/container/plugins/
└── community-source/
    ├── ravalink-plugin.toml
    └── bin/
        └── community-source
```

Make sure the executable bit is set where the panel file manager supports permissions. A plugin executable must be compatible with the container architecture and Linux userspace. Restart RavaLink after installing or changing a plugin; plugins are discovered at startup.

See [Plugin installation and development](../plugins/index.md) for the manifest and protocol reference.

## YouTube authentication files

Optional YouTube cookies and OAuth token files must be present inside the container at stable paths and readable/writable by the `container` user as appropriate.

- Cookies: upload a Netscape-format cookies file and set `RAVALINK_YOUTUBE_COOKIES_FILE` to its container path.
- OAuth: set both `RAVALINK_YOUTUBE_OAUTH_CLIENT_ID` and `RAVALINK_YOUTUBE_OAUTH_TOKEN_FILE`. The token file's parent directory must be writable by the server. Complete the device authorization shown in the console on first start.

Treat cookies and OAuth token files as credentials. Do not share them in support tickets.

## Logs and troubleshooting

Use the panel console first. Increase `RAVALINK_LOG_LEVEL` to `debug` temporarily when investigating startup or provider issues.

| Symptom | Check |
|---|---|
| Container exits immediately | Confirm the official image is selected and the egg startup command is `ravalink` |
| Password/configuration error | Set a non-placeholder password with at least 16 characters |
| Port is unreachable | Confirm the allocation matches `RAVALINK_PORT`; keep bind address at `0.0.0.0` |
| Image pull denied | Confirm the GHCR package is public and the image name is exact |
| Plugin not loaded | Check the plugin directory, manifest syntax, executable path, permissions and CPU architecture |
| Plugin loads but no track plays | Verify the plugin returns a supported direct HTTP(S) audio stream and valid Lavalink track metadata |
| YouTube OAuth repeats | Check the client ID/token path pair and ensure the token directory is writable |

For generic Docker deployment, see [Docker deployment](../getting-started/docker.md).
