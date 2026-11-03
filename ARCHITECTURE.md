# RavaLink architecture

RavaLink is a native Rust reimplementation of the Lavalink v4 server contract.
The server must not require a JVM, Lavaplayer, Kotlin or Java at runtime.

## Repository layout

This repository is one Git repository and one Cargo workspace:

- `crates/protocol`: Lavalink v4 wire models and Lavaplayer track-info codec. No HTTP or audio dependencies.
- `crates/core`: sessions and shared runtime state.
- `crates/audio`: Symphonia media decoding, native PCM player state and stateful DSP processing.
- `crates/server`: HTTP/WebSocket server and process entry point.

Source adapters and streaming, Opus output, full filter/time-stretch parity, voice transport, plugin API and metrics remain to be implemented. They will remain in this same repository.

## Dependency direction

```text
server -> protocol
server -> core
core   -> protocol (only when required)
audio  -> protocol
```

The protocol crate must never depend on the audio engine. HTTP handlers translate
wire requests into core commands; the audio pipeline never serializes REST
responses itself.

## Runtime rules

- Tokio owns asynchronous I/O.
- CPU-heavy decode, resampling and DSP work must not block Tokio executor threads; `decode_bytes` is synchronous and callers should dispatch it with `spawn_blocking`.
- Per-player state must not be guarded by a process-wide lock.
- Internal queues must be bounded and apply backpressure.
- Audio buffers should be reused where practical.
- No Java process, JNI bridge or Lavaplayer fallback is allowed.

## Compatibility policy

The observable Lavalink v4 contract is the source of truth: route paths,
methods, status codes, headers, JSON omission/null behavior, WebSocket handshake,
ready payload, events, session semantics and player state transitions.

A feature is not considered compatible merely because a route exists. It must
pass contract tests against the reference implementation.
