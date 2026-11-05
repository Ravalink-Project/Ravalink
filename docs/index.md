---
template: home.html
title: Home
description: A native Rust audio node for the Lavalink v4 ecosystem.
hide:
  - footer
  - navigation
  - navigation.tabs
  - toc
  - path
---

> [!WARNING]
> RavaLink is in pre-release development. Full Lavalink v4 compatibility and production readiness have not yet been declared. See the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) and [RC1 acceptance checklist](https://github.com/Ravalink-Project/Ravalink/blob/master/RC1_ACCEPTANCE.md).

## Built for the Lavalink ecosystem

::cards::

- title: Native Rust runtime
  icon: ':material-language-rust:'
  url: https://github.com/Ravalink-Project/Ravalink/blob/master/ARCHITECTURE.md
- title: Lavalink v4 REST API
  icon: ':material-api:'
  url: api/rest.md
- title: WebSocket sessions
  icon: ':material-swap-horizontal:'
  url: api/websocket.md
- title: Discord voice and DAVE
  icon: ':material-headphones:'
- title: Audio DSP and Timescale
  icon: ':material-waveform:'
- title: Built-in source providers
  icon: ':material-music-box-multiple:'
- title: Prometheus metrics
  icon: ':simple-prometheus:'
  url: configuration/
- title: Docker deployment
  icon: ':simple-docker:'
  url: getting-started/docker.md
- title: Native plugin extensions
  icon: ':material-puzzle:'
  url: plugins.md

::/cards::

## Get started

Build RavaLink from source using the current stable Rust toolchain:

~~~bash
git clone https://github.com/Ravalink-Project/Ravalink.git
cd Ravalink
cargo build --release -p ravalink-server
~~~

Then configure a secure node password using the [configuration guide](configuration/).

## Compatibility first

RavaLink aims to work with existing Lavalink v4 clients. Automated smoke scenarios currently exercise Wavelink and Shoukaku, but these checks do not establish complete API compatibility. The [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) documents implemented behavior and known limitations.

## Join the community

Need help, want to contribute or have compatibility feedback? Join the [RavaLink Discord server](https://discord.gg/PBxB5eFNUp) or browse the [GitHub repository](https://github.com/Ravalink-Project/Ravalink).
