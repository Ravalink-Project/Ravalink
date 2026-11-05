---
description: Build a native extension for RavaLink.
---

# Build a RavaLink extension

RavaLink does not load Java JAR files or use the original Lavalink Spring plugin API.

The supported external extension mechanism launches a configured native executable as a child process and exchanges versioned JSON messages over standard input and output.

See the [external process extension guide](../plugins/external-process.md) for the plugin manifest format, process configuration, request and response schemas, execution limits and security considerations.

Built-in provider integrations live in the Rust workspace. Significant new extension points should be discussed with the maintainers in the [RavaLink Discord](https://discord.gg/PBxB5eFNUp) before implementation.
