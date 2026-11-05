---
description: Build and run RavaLink from source.
---

# Build from source

RavaLink is a native Rust application. It does not require a Java runtime.

## Requirements

- Current stable Rust toolchain
- Git
- OpenSSL command-line utility to generate a strong password

## Clone and build

~~~bash
git clone https://github.com/Ravalink-Project/Ravalink.git
cd Ravalink
cargo build --release -p ravalink-server
~~~

The executable is created at target/release/ravalink.

## Configure and start

~~~bash
export RAVALINK_PASSWORD="$(openssl rand -hex 32)"
cargo run --release -p ravalink-server
~~~

The server listens on 0.0.0.0:2333 by default. It refuses to start without a password of at least 16 characters.

For persistent settings, copy config/ravalink.example.toml to config/ravalink.toml and edit it. You can also set RAVALINK_CONFIG to an explicit TOML file path.

See the [configuration reference](../configuration/) and [systemd guide](systemd.md) for deployment details.
