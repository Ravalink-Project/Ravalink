# Stable release acceptance checklist

A release is stable only when every item below is checked and the
[compatibility ledger](COMPATIBILITY.md) contains no scaffold, pending, or
unverified implementation rows.

## Automated build and compatibility

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo test --workspace --all-targets` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] The optimized server builds for Linux x86_64 and aarch64.
- [ ] The API smoke test passes against the optimized binary.
- [ ] The Docker image builds for each published architecture.
- [ ] REST and WebSocket responses are contract-tested against the supported
      Lavalink v4 release, including error and empty-result cases.
- [ ] Track codec fixtures cover every supported Lavaplayer version and source
      payload; unsupported custom payloads fail explicitly.

## Discord voice acceptance

- [ ] A real bot joins and leaves a test guild voice channel.
- [ ] Legacy RTP encryption modes negotiate and transmit successfully.
- [ ] DAVE session establishment, MLS proposals, commits, welcomes, user
      connect/disconnect and epoch transitions are verified against Discord.
- [ ] Voice Gateway resume works after a real websocket interruption.
- [ ] UDP interruption and endpoint changes recover without leaving stale
      player or voice-task state.
- [ ] Speaking state, silence frames, pause, resume and track replacement are
      verified in a real guild.

## Playback and providers

- [ ] YouTube search, single-video playback, playlists, timestamps and
      configured Invidious failover are tested with live providers.
- [ ] Spotify, Apple Music and Deezer metadata, albums, playlists and playable
      track matching are tested with valid provider credentials where required.
- [ ] Long tracks, initial offsets, repeated seeks, pause/resume, end-time,
      volume, every advertised filter and source replacement are tested.
- [ ] Decoder failures, truncated downloads, HTTP status errors, cache pressure
      and consumer cancellation produce the expected Lavalink events.
- [ ] Concurrent players stay within configured download, source-cache,
      seek-cache and PCM limits.

## Security and release artifacts

- [ ] HTTP source resolution is tested against redirects, private/reserved IPs,
      DNS rebinding and oversized responses.
- [ ] Default configuration refuses weak public passwords.
- [ ] Docker runs as a non-root user and exposes only documented ports.
- [ ] Egg startup is tested with the actual published x86_64 and aarch64 assets.
- [ ] Release binaries have SHA-256 checksums and the published tag points to
      the exact source revision that passed verification.
- [ ] Known limitations, supported providers and configuration defaults are
      documented before publishing.
