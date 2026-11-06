---
template: home.html
title: Home
description: Complete documentation for the native Rust Lavalink-compatible audio node.
hide:
  - footer
  - navigation
  - navigation.tabs
  - toc
  - path
---

> [!WARNING]
> RavaLink is in pre-release development. Review the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) and [RC1 acceptance checklist](https://github.com/Ravalink-Project/Ravalink/blob/master/RC1_ACCEPTANCE.md) before using it outside a test environment.

## Built for the Lavalink ecosystem

::cards::

- title: Quick start
  icon: ':material-rocket-launch:'
  url: getting-started/
- title: Pelican and Pterodactyl
  icon: ':material-server:'
  url: deployment/pelican-pterodactyl.md
- title: Docker
  icon: ':simple-docker:'
  url: getting-started/docker.md
- title: Configuration
  icon: ':material-tune:'
  url: configuration/
- title: Plugin development
  icon: ':material-puzzle:'
  url: plugins/
- title: REST API
  icon: ':material-api:'
  url: api/rest.md
- title: WebSocket API
  icon: ':material-swap-horizontal:'
  url: api/websocket.md
- title: Client compatibility
  icon: ':material-connection:'
  url: clients.md

::/cards::

## Choose your path

- **I want to run a node:** start with [Getting started](getting-started/), then choose [Docker](getting-started/docker.md) or [Pelican/Pterodactyl](deployment/pelican-pterodactyl.md).
- **I want to configure a node:** read the [TOML reference](configuration/config/file.md) and [environment variables](configuration/config/environment-variables.md).
- **I want to use an existing plugin:** follow [Plugin installation](plugins/index.md).
- **I want to create a plugin:** choose [Native Rust development](plugins/native-rust.md) or the [external process protocol](plugins/external-process.md).
- **I want to integrate a Discord bot:** read the [REST API](api/rest.md), [WebSocket API](api/websocket.md) and [client compatibility](clients.md).

## Compatibility first

RavaLink targets the Lavalink v4 protocol and aims to let existing clients connect by changing their node endpoint and credentials. Automated smoke tests do not establish complete API or playback compatibility. The [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) records implementation status and known limitations.

## Community and contributions

Join the [RavaLink Discord](https://discord.gg/PBxB5eFNUp) for help and development discussion, or browse the [GitHub repository](https://github.com/Ravalink-Project/Ravalink).
