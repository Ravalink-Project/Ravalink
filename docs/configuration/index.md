---
description: Configure the native RavaLink server.
---

# Configuration

RavaLink reads TOML configuration and environment variables. Environment variables override values loaded from the TOML file.

## Configuration file

The default path is config/ravalink.toml. Start from the [example configuration](https://github.com/Ravalink-Project/Ravalink/blob/master/config/ravalink.example.toml).

Set RAVALINK_CONFIG to load a different file. If this variable is set and the file does not exist, startup fails instead of silently using defaults.

See the [configuration file reference](config/file.md).

## Environment variables

Use environment variables for secrets and deployment-specific overrides. See the [environment variable reference](config/environment-variables.md).

## Security

- Use a unique random password of at least 16 characters.
- Do not commit secrets or provider credentials.
- The Prometheus endpoint is unauthenticated when enabled; restrict it at the firewall or reverse proxy.
- Keep external plugin executables trusted and run RavaLink as an unprivileged service account.
