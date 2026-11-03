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
cargo run -p ravalink-server
```

The server listens on `0.0.0.0:2333` by default.

## Configuration

Configuration is loaded from `config/ravalink.toml` by default, or from
`RAVALINK_CONFIG` when explicitly set. Environment variables override file values:

- `RAVALINK_ADDRESS`
- `RAVALINK_PORT`
- `RAVALINK_PASSWORD`
- `RAVALINK_LOG_LEVEL`
- `RAVALINK_AUDIO_MAX_SOURCE_DOWNLOAD_BYTES` (default: 134217728)
- `RAVALINK_AUDIO_MAX_DECODED_CACHE_BYTES` (default: 268435456)

A sample is available at [config/ravalink.example.toml](config/ravalink.example.toml).

## Project direction

1. Complete Lavalink v4 protocol, track-codec and session/player semantics.
2. Expand native source resolution beyond direct HTTP(S) files, then complete demux/decode, filter parity and Opus encoding.
3. Implement Discord voice transport, including DAVE and current voice encryption requirements.
4. Add a native extension API, observability and compatibility/benchmark suites.
5. Prove functional parity and measurable performance improvements.

Rust itself is not treated as proof of performance. Benchmarks must compare both
nodes on the same hardware and workloads.
