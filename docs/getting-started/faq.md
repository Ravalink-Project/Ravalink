---
description: Frequently asked questions about RavaLink.
---

# FAQ

## What is RavaLink?

RavaLink is an independent native Rust audio node designed to implement the Lavalink v4 server protocol for Discord music bots. Unlike the original Lavalink Java server, the RavaLink runtime does not require a JVM or Lavaplayer.

## Is RavaLink a drop-in Lavalink replacement?

That is the project goal, but full compatibility has not yet been declared. Automated smoke tests cover Wavelink and Shoukaku integration paths. See the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) for current implementation status and limitations.

## How do I install RavaLink?

Follow the [source build guide](binary.md) or the [Docker guide](docker.md). Prebuilt release assets will be published with the first release candidate after its acceptance gates pass.

## How do I configure RavaLink?

RavaLink uses a TOML file and RAVALINK_* environment variables. Start with the [configuration guide](../configuration/).

## Can I use my existing Lavalink client?

RavaLink is designed to support clients that speak Lavalink v4. Wavelink 3.5.2 and Shoukaku 4.3.0 have automated CI smoke tests; other clients and full feature parity are not yet certified. See [client compatibility](../clients.md).

## Can I load Java Lavalink plugins?

No. RavaLink is not a JVM plugin host. It provides built-in Rust provider integrations and an external-process extension protocol. See [Plugins and extensions](../plugins.md).

## How do I report a problem?

Join the [RavaLink Discord](https://discord.gg/PBxB5eFNUp) and include the commit or release tag, client version, operating system, reproduction steps and sanitized logs.
