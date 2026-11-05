---
description: Configure the RavaLink server runtime.
---

# Server settings

The server section controls the HTTP/WebSocket listener and Lavalink session update interval.

~~~toml
[server]
address = "0.0.0.0"
port = 2333
password = "replace-with-a-long-random-secret"
player_update_interval = 5
~~~

| Setting | Default | Description |
|---|---:|---|
| address | 0.0.0.0 | Address to bind |
| port | 2333 | HTTP and WebSocket port |
| password | Required | Authentication secret; at least 16 characters |
| player_update_interval | 5 | Player position update interval in seconds |

The password can also be supplied using RAVALINK_PASSWORD. Environment variables take precedence over the TOML file.

The example password is intentionally rejected at startup. Generate a unique secret before deploying RavaLink.
