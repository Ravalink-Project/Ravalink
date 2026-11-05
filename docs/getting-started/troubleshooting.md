---
description: Troubleshoot common RavaLink deployment and connection issues.
---

# Troubleshooting

## RavaLink does not start

- Confirm that RAVALINK_PASSWORD is set and contains at least 16 characters.
- Check that the TOML file is valid and that RAVALINK_CONFIG points to an existing file if explicitly set.
- Confirm that the configured address and port are available.
- Run with RAVALINK_LOG_LEVEL=debug to obtain more diagnostic output.
- Check the service logs with journalctl when running under systemd.

## The client cannot connect

- Confirm that the node address and port are reachable from the bot host.
- Verify that the client uses the same password configured on the node.
- If using a reverse proxy, make sure it forwards WebSocket upgrade requests.
- Check that TLS settings in the client match the proxy endpoint.

## The player does not join a voice channel

The Discord bot client must forward the Discord voice server and voice state updates to the Lavalink node using the player update REST endpoint. Check that the payload contains the current token, endpoint, sessionId and channelId values.

## Audio does not play

- Confirm that the selected provider can resolve the identifier.
- Check provider credentials and upstream availability where required.
- Inspect TrackException events and server logs.
- Check the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) for unsupported source/container cases.

## Enable debug logging

Set the log level in the TOML file:

~~~toml
[logging]
level = "debug"
~~~

Or use the environment variable:

~~~bash
RAVALINK_LOG_LEVEL=debug
~~~

## Get help

Join the [RavaLink Discord community](https://discord.gg/PBxB5eFNUp). When sharing logs, remove passwords, tokens, provider credentials and other secrets first.
