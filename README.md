# RavaLink

RavaLink is a native Rust audio node targeting drop-in compatibility with the
Lavalink v4 server API. The long-term goal is to let existing clients such as
Wavelink, Lavalink.py, Shoukaku and discord-player connect by changing only the
node address.

**RavaLink is under active development and is not yet a drop-in replacement.**
See [COMPATIBILITY.md](COMPATIBILITY.md) for the implementation status.

## Native runtime

The Rust implementation is designed to run without:

- Java or a JVM
- Kotlin or Gradle
- Lavaplayer
- JNI bridges

The repository is a single Cargo workspace. Internal crates are modules of one
product, not separate repositories.

## Build

Requirements: a current stable Rust toolchain.

```bash
cargo build --workspace
cargo test --workspace
export RAVALINK_PASSWORD="$(openssl rand -hex 32)"
cargo run -p ravalink-server
```

The server listens on `0.0.0.0:2333` by default. A password of at least
16 characters is mandatory. RavaLink refuses to start with an empty, short,
or known example password.

## Configuration

Configuration is loaded from `config/ravalink.toml` by default, or from
`RAVALINK_CONFIG` when explicitly set. Environment variables override file values:

- `RAVALINK_ADDRESS`
- `RAVALINK_PORT`
- `RAVALINK_PASSWORD`
- `RAVALINK_PLAYER_UPDATE_INTERVAL` (default: 5 seconds)
- `RAVALINK_LOG_LEVEL`
- `RAVALINK_AUDIO_BUFFER_DURATION_MS`
- `RAVALINK_AUDIO_FRAME_BUFFER_DURATION_MS`
- `RAVALINK_AUDIO_OPUS_ENCODING_QUALITY`
- `RAVALINK_AUDIO_RESAMPLING_QUALITY`
- `RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES` (default: 134217728)
- `RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES` (default: 268435456; pending load cache)
- `RAVALINK_AUDIO_MAX_ACTIVE_DECODED_PCM_BYTES` (default: 268435456; active native players)
- `RAVALINK_METRICS_PROMETHEUS_ENABLED`
- `RAVALINK_METRICS_PROMETHEUS_ENDPOINT`

Built-in provider configuration:
- `RAVALINK_YOUTUBE_API_BASE` (Invidious-compatible API base URL)
- `RAVALINK_SPOTIFY_CLIENT_ID` and `RAVALINK_SPOTIFY_CLIENT_SECRET` (Spotify search and collection API)
- `RAVALINK_APPLE_MUSIC_TOKEN` (Apple Music playlist catalog API)

**Security:** replace the example/default password with a long random secret before exposing the node to the internet. The Prometheus endpoint is unauthenticated when enabled; restrict it at the firewall or reverse proxy.

Prometheus metrics are disabled by default. Enable them with `[metrics].prometheus_enabled = true` in the TOML file and optionally change `[metrics].prometheus_endpoint` (default: `/metrics`). The endpoint is intentionally unauthenticated for Prometheus scraping; restrict access at the network or reverse-proxy layer.

A sample is available at [config/ravalink.example.toml](config/ravalink.example.toml).

## Docker

A native multi-stage image and Compose file are included:

```sh
cp config/ravalink.example.toml ravalink.toml
export RAVALINK_PASSWORD="$(openssl rand -hex 32)"
docker compose up -d --build
```

The default Compose port binding is localhost-only. See [Docker deployment](docs/getting-started/docker.md) before exposing the node publicly.

## Project direction

1. Finish the remaining Lavalink v4 REST/WebSocket edge cases and verify them against upstream acceptance scenarios.
2. Replace whole-file buffering with a segmented, back-pressured streaming demux/decode pipeline.
3. Improve source reliability and YouTube extraction beyond a single Invidious-compatible service.
4. Design stable external Rust plugin distribution/loading without exposing an unsafe Rust ABI.
5. Expand end-to-end Discord voice, compatibility and performance testing on real nodes and multiple client libraries.

Rust itself is not treated as proof of performance. Benchmarks must compare both
nodes on the same hardware and workloads.
