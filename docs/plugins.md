---
description: Native extensions and built-in source integrations for RavaLink.
---

# Plugins and extensions

RavaLink has its own native extension model. It is **not** a JVM plugin host: Java JAR plugins built for the original Lavalink server are not directly loadable.

## Built-in integrations

The Rust workspace currently includes these provider crates:

| Integration | Purpose |
|---|---|
| YouTube | YouTube search, metadata resolution and playable audio stream selection |
| LavaSrc | Spotify, Apple Music and Deezer catalog resolution and matching |
| LavaSearch | Structured search and source query handling |

Provider behavior can depend on external services, credentials, rate limits and upstream changes. See the configuration reference for provider settings and the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) for known limitations.

## External Rust extensions

RavaLink can discover community plugin manifests and launch configured native executables using a versioned JSON-over-stdio protocol.

- Extensions run as child processes.
- Requests and responses use JSON.
- The protocol has response-size and execution-time limits.
- Executables run with the RavaLink service account and are trusted code; this is not a security sandbox.
- Dynamic library loading and a stable binary ABI are not currently provided.

Read the [external process extension guide](plugins/external-process.md) for the manifest format, request contract and configuration examples.

## Contributing an integration

Before building a new integration, open a discussion in the [RavaLink Discord](https://discord.gg/PBxB5eFNUp) or the repository issue tracker. Include the provider, expected operations, authentication requirements, legal/API constraints and test strategy.
