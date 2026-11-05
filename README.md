# RavaLink

<div align="center">

  <img src="docs/assets/logo.svg" alt="RavaLink" width="180" />

  <h3>A native Rust audio node for the Lavalink v4 ecosystem.</h3>
  <p>Built for Discord music infrastructure. Designed to speak the Lavalink protocol without a JVM.</p>

  <p>
    <a href="https://discord.com/invite/PBxB5eFNUp"><img src="https://discord.com/api/guilds/1566146315107442770/widget.png?style=banner2" alt="RavaLink Discord server — live members and online presence" /></a>
    <a href="https://github.com/Ravalink-Project/Ravalink"><img src="https://img.shields.io/badge/GitHub-Source%20code-181717?logo=github" alt="GitHub repository" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-green.svg" alt="MIT License" /></a>
    <a href="https://github.com/Ravalink-Project/Ravalink/actions/workflows/rust.yml"><img src="https://github.com/Ravalink-Project/Ravalink/actions/workflows/rust.yml/badge.svg?branch=master" alt="Rust CI" /></a>
    <a href="https://github.com/Ravalink-Project/Ravalink/actions/workflows/build.yml"><img src="https://github.com/Ravalink-Project/Ravalink/actions/workflows/build.yml/badge.svg?branch=master" alt="Native build CI" /></a>
  </p>

  <p>
    <a href="#getting-started">Get started</a> ·
    <a href="COMPATIBILITY.md">Compatibility</a> ·
    <a href="docs/">Documentation</a> ·
    <a href="#community-support">Discord community</a>
  </p>

</div>

---

> [!WARNING]
> **RavaLink is currently in pre-release development.** It is not yet declared a fully compatible, production-ready drop-in replacement for Lavalink. Check the [compatibility ledger](COMPATIBILITY.md) and [RC1 acceptance checklist](RC1_ACCEPTANCE.md) before evaluating it.

## About

RavaLink is an independent, native Rust audio node built to integrate with the existing Lavalink v4 ecosystem. It aims to let Discord bot developers keep their current Lavalink client and connect to a RavaLink node by changing the node endpoint and credentials.

The server is implemented as a Rust workspace. It does not require Java, a JVM, Kotlin, Gradle, Lavaplayer or JNI at runtime. The previous Java implementation is preserved separately on the [java branch](https://github.com/Ravalink-Project/Ravalink/tree/java); the default branch is the native Rust project.

**Our goal:** make a dependable, observable and extensible audio node while preserving the protocol and workflows Lavalink users already know.

## Project highlights

The codebase currently includes the following implementation areas. Their presence does not imply that every upstream behavior has passed compatibility acceptance; see [COMPATIBILITY.md](COMPATIBILITY.md) for the precise status and limitations.

- **Lavalink v4 interface** — HTTP REST endpoints, authenticated WebSocket sessions, player lifecycle and event serialization.
- **Native audio pipeline** — Symphonia-based demuxing/decoding, bounded PCM queues, DSP processing, resampling and Opus encoding.
- **Discord voice transport** — Gateway voice negotiation, RTP packetization, encryption and DAVE-related protocol handling.
- **Built-in providers** — YouTube, LavaSrc integrations and LavaSearch functionality.
- **Extension model** — Rust plugin interfaces and an optional external-process JSON protocol.
- **Operations** — TOML/environment configuration, structured logs, Docker support and optional Prometheus metrics.
- **Resource limits** — Bounded queues and caches intended to keep memory use predictable under load.

## Client ecosystem

RavaLink is designed around the Lavalink v4 protocol rather than a RavaLink-specific client API. The project currently runs compatibility smoke scenarios with:

| Client | Runtime | Smoke status |
|---|---|---|
| [Wavelink](https://github.com/PythonistaGuild/Wavelink) | Python | Automated smoke test |
| [Shoukaku](https://github.com/Deivu/Shoukaku) | Node.js | Automated smoke test |

These smoke tests are useful integration checks, not proof that every API operation or edge case is compatible. Broader contract coverage and real Discord acceptance testing remain part of the release process.

## Getting started

### Requirements

- A current stable Rust toolchain
- OpenSSL for generating a secure password
- Optional: Docker and Docker Compose for container deployment

### Build from source

~~~bash
git clone https://github.com/Ravalink-Project/Ravalink.git
cd Ravalink

cargo build --workspace --release
cargo test --workspace --all-targets

export RAVALINK_PASSWORD="$(openssl rand -hex 32)"
cargo run --release -p ravalink-server
~~~

By default, the server listens on 0.0.0.0:2333.

A password of at least 16 characters is required. RavaLink refuses to start with an empty, short or known example password.

### Docker

~~~bash
git clone https://github.com/Ravalink-Project/Ravalink.git
cd Ravalink

cp config/ravalink.example.toml ravalink.toml
export RAVALINK_PASSWORD="$(openssl rand -hex 32)"
docker compose up -d --build
~~~

The default Compose binding is localhost-only. Read the [Docker deployment guide](docs/getting-started/docker.md) before exposing the node outside your machine.

### Configuration

RavaLink loads config/ravalink.toml by default. Set RAVALINK_CONFIG to use another file. Environment variables override values from the TOML configuration.

Common settings:

| Variable | Purpose |
|---|---|
| RAVALINK_ADDRESS | Bind address |
| RAVALINK_PORT | HTTP/WebSocket port |
| RAVALINK_PASSWORD | Node authentication secret |
| RAVALINK_LOG_LEVEL | Log verbosity |
| RAVALINK_PLAYER_UPDATE_INTERVAL | Player update interval |
| RAVALINK_METRICS_PROMETHEUS_ENABLED | Enable Prometheus metrics |
| RAVALINK_METRICS_PROMETHEUS_ENDPOINT | Metrics route |

Provider credentials and advanced audio/resource settings are documented in the [example configuration](config/ravalink.example.toml) and [configuration reference](docs/configuration/).

> [!IMPORTANT]
> Generate a unique, long password before deployment. The optional Prometheus endpoint does not provide authentication by itself; restrict it using your firewall or reverse proxy.

## Documentation

- [Getting started](docs/getting-started/)
- [Configuration](docs/configuration/)
- [Docker deployment](docs/getting-started/docker.md)
- [REST API](docs/api/rest.md)
- [WebSocket API](docs/api/websocket.md)
- [Plugin system](docs/plugins.md)
- [Architecture overview](ARCHITECTURE.md)
- [Compatibility ledger and known limitations](COMPATIBILITY.md)
- [Changelog](CHANGELOG.md)
- [RC1 acceptance checklist](RC1_ACCEPTANCE.md)

## Development

Contributions, bug reports, compatibility findings and documentation improvements are welcome.

1. Read the [architecture overview](ARCHITECTURE.md) and [compatibility ledger](COMPATIBILITY.md).
2. Open an issue to discuss significant changes before starting implementation.
3. Keep changes focused and include regression tests for behavior changes.
4. Run the checks before submitting a pull request:

~~~bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
~~~

Please include reproduction steps, logs with secrets removed, and the exact client/provider versions when reporting compatibility issues.

<a id="community-support"></a>

## Community & support

Questions, development discussions, integration help and release feedback are welcome in the RavaLink Discord.

<div align="center">

  <a href="https://discord.com/invite/PBxB5eFNUp">
    <img src="https://discord.com/api/guilds/1566146315107442770/widget.png?style=banner2" alt="RavaLink Discord server — live members and online presence" />
  </a>

  <p><sub>Live server widget · Member presence · Join the community</sub></p>

</div>

For bugs or feature proposals, use [GitHub Issues](https://github.com/Ravalink-Project/Ravalink/issues) when repository issue tracking is enabled, or start a discussion in Discord.

## Release status

The first planned release candidate is **v1.0.0-rc.1**. It will be published only after the automated pre-RC acceptance gates are satisfied. Live Discord playback, provider behavior under real credentials and long-running load tests are planned for the RC validation cycle.

See [RC1_ACCEPTANCE.md](RC1_ACCEPTANCE.md) for the release gates. Do not use the current development branch as a production replacement.

## License

RavaLink is distributed under the [MIT License](LICENSE).

---

<div align="center">
  <sub>Built in the open by the RavaLink community.</sub>
</div>
