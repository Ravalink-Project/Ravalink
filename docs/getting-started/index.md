---
description: Getting started with the native RavaLink audio node.
---

# Getting started

RavaLink is a native Rust audio node that targets the Lavalink v4 protocol. You can build it from source or run it with Docker.

> [!WARNING]
> RavaLink is in pre-release development. Review the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) before using it outside a test environment.

## Before you begin

- Choose a Lavalink v4-compatible Discord client. See [client validation](../clients.md).
- Prepare a secure node password of at least 16 characters.
- Decide whether you want a source build, a systemd service or Docker.

## Run RavaLink

- [Build and run the native binary](binary.md)
- [Run under systemd](systemd.md)
- [Run with Docker](docker.md)

## Need help?

Join the [RavaLink Discord community](https://discord.gg/PBxB5eFNUp). When reporting a problem, include the RavaLink commit, operating system, client version and sanitized logs.
