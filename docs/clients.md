---
description: Client libraries and their current RavaLink validation status.
---

# Client compatibility

RavaLink implements the Lavalink v4 protocol so existing clients can connect without a RavaLink-specific SDK. Compatibility is still being validated; the table below distinguishes automated smoke coverage from general protocol intent.

## Automated smoke coverage

| Client | Runtime | Tested version | Current validation |
|---|---|---:|---|
| [Wavelink](https://github.com/PythonistaGuild/Wavelink) | Python | 3.5.2 | CI smoke test: connect, read info/stats, resolve an unsupported identifier, create/list/delete a player |
| [Shoukaku](https://github.com/Deivu/Shoukaku) | Node.js | 4.3.0 | CI smoke test: connect, read info/stats, resolve an unsupported identifier, create/list/delete a player |

These are integration smoke tests, not complete client certification. They do not currently prove every operation, event, reconnection scenario or real Discord voice playback path.

## Using another Lavalink client

In principle, clients that support Lavalink v4 should be able to target a RavaLink node using their normal Lavalink connection settings:

- Node address: the RavaLink host and port
- Password: the configured RavaLink authentication secret
- Secure connection: enable TLS when using an HTTPS/WSS reverse proxy

Check the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) before deploying a client that is not listed above.

## Report a compatibility issue

When reporting a client issue, include:

- Client name and exact version
- RavaLink commit or release tag
- Operating system and architecture
- The request/event that failed
- Sanitized logs and a minimal reproduction

Join the [RavaLink Discord](https://discord.gg/PBxB5eFNUp) for integration discussion, or use the repository issue tracker when enabled.
