# RavaLink

RavaLink is a native Rust audio node targeting drop-in compatibility with the
Lavalink v4 server API. The goal is to let existing clients such as Wavelink,
Lavalink.py, Shoukaku and discord-player connect by changing only the node
address.

**RavaLink is under active development and is not yet a drop-in replacement.**
See [COMPATIBILITY.md](COMPATIBILITY.md) for the implementation status and
known compatibility gaps.

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
- `RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES` (default: 268435456; compressed source cache and aggregate active seek-source cache)
- `RAVALINK_AUDIO_MAX_ACTIVE_DECODED_PCM_BYTES` (default: 268435456; active native players)
- `RAVALINK_METRICS_PROMETHEUS_ENABLED`
- `RAVALINK_METRICS_PROMETHEUS_ENDPOINT`

Built-in provider configuration:
- `RAVALINK_YOUTUBE_API_BASE` (comma-separated Invidious-compatible API failover URLs)
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

The implementation already includes a bounded progressive source pipeline, a native
audio decode/encode path, Discord voice transport and the built-in provider/plugin
crates. These components still require systematic contract and failure-path
coverage; their presence in the codebase is not, by itself, proof of full
Lavalink compatibility.

The pre-RC1 work is focused on:

1. Completing and verifying the full public Lavalink v4 REST and WebSocket contract,
   including response schemas, status codes, events, session behavior and edge cases.
2. Running compatibility scenarios against upstream Lavalink v4 and supported client
   libraries, with no RavaLink-specific client changes.
3. Closing the remaining audio, source, provider, voice/DAVE and plugin behavior gaps
   that can be validated in automated tests.
4. Exercising resource limits, cancellation, malformed input, provider failures and
   reconnect paths in repeatable tests.
5. Keeping Linux x86_64, Linux ARM64 and Docker builds continuously validated on
   the active `master` branch.

The first release candidate is intended to be a usable test build, not a feature
preview: the base Lavalink v4 API and all advertised core operations must be
implemented and covered by automated tests before RC1. Real Discord guild,
provider-account and network-condition acceptance testing will then be performed
during the RC cycle.

Rust itself is not treated as proof of performance. Benchmarks must compare both
nodes on the same hardware and workloads.

## Release readiness

See [STABLE_RELEASE_CHECKLIST.md](STABLE_RELEASE_CHECKLIST.md) before publishing a stable release.
