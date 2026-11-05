# RC1 acceptance checklist

RC1 is a testable release candidate, not a stable release. Every item in the
automated pre-RC1 section must be checked before publishing an RC tag.
Real Discord guild and provider-account acceptance tests are intentionally
performed during the RC cycle.

## Automated pre-RC1 gates

- [x] The current master commit passes rustfmt, the full workspace test suite,
      and Clippy with warnings denied.
- [x] Linux x86_64 and ARM64 optimized builds and API smoke tests pass.
- [x] Ubuntu, macOS and Windows workspace tests pass.
- [x] The Docker image builds successfully.
- [ ] REST route, request, response, status-code and error contracts are covered
      by automated tests against the supported Lavalink v4 behavior.
- [ ] WebSocket handshake, session resume, player updates, event payloads,
      heartbeat timeout and reconnect behavior have automated contract tests.
- [x] Wavelink and Shoukaku compatibility smoke tests pass without RavaLink-
      specific client patches.
- [ ] Track codec fixtures cover supported Lavaplayer versions and built-in
      source payloads; unsupported plugin payloads fail explicitly.
- [ ] Every advertised standard filter, including independent Timescale
      speed, pitch and rate controls, passes DSP and player-lifecycle tests.
- [ ] Playback tests cover seek, replacement, positive end-time and zero/negative end-time rejection,
      pause/resume, decoder failure, cancellation and bounded-resource limits.
- [ ] Provider tests cover success, empty results, malformed responses,
      unavailable providers and playable-track matching.
- [ ] Voice protocol tests cover RTP encryption modes, DAVE state transitions,
      gateway resume and UDP recovery paths.
- [ ] The release binary smoke test passes, and the release workflow builds both target binaries and generates SHA256SUMS before publishing assets.
- [ ] COMPATIBILITY.md contains no unimplemented or silently ignored base
      Lavalink v4 behavior.

## Post-RC1 real-world validation

- [ ] A real bot joins/leaves a Discord guild and plays audio end to end.
- [ ] DAVE negotiation, voice reconnect and UDP interruption are validated
      against Discord.
- [ ] Wavelink and Shoukaku are exercised by real bots against the RC build.
- [ ] YouTube, Spotify, Apple Music and Deezer are tested with live providers
      and valid credentials where required.
- [ ] Long-running playback, concurrent guilds and memory/CPU behavior are
      observed under realistic load.
- [ ] RC feedback is triaged and blocking defects are fixed before stable.
