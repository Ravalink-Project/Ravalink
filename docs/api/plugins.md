---
description: RavaLink plugin API and extension development.
---

# Plugin API and extensions

RavaLink does not load Java JAR files or implement the original Lavalink Spring plugin API.

There are two supported extension paths:

- **Native Rust plugins**, compiled into the RavaLink workspace and implementing the `RavaLinkPlugin` trait.
- **External process plugins**, independent executables communicating through versioned JSON over stdin/stdout.

Start with the [plugin overview](../plugins/index.md), then choose the [native Rust development guide](../plugins/native-rust.md) or the [external process protocol](../plugins/external-process.md).

The external process interface is a resolver extension point. It does not expose arbitrary server internals, Discord voice transport hooks, custom decoder registration or REST route registration.
