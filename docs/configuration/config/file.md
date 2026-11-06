---
description: Configure RavaLink using TOML.
---

# Configuration file

RavaLink uses TOML rather than the application.yml format used by the original Java server.

The default configuration path is config/ravalink.toml. The repository provides an [example TOML file](https://github.com/Ravalink-Project/Ravalink/blob/master/config/ravalink.example.toml).

Copy the example into the config directory:

~~~bash
mkdir -p config
cp config/ravalink.example.toml config/ravalink.toml
~~~

The file contains sections for:

- server address, port, authentication password and player update interval
- logging verbosity
- audio buffering, Opus quality and memory limits
- YouTube extraction, optional browser cookies and Google OAuth Device Flow authentication
- optional Prometheus metrics
- native plugin discovery and external plugin declarations

Environment variables override the corresponding TOML settings. See [Environment variables](environment-variables.md).

For security, replace the example password with a unique secret before starting the node.
