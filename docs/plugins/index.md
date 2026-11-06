---
description: Install, configure and develop RavaLink plugins.
---

# Plugins and source integrations

RavaLink has a native Rust plugin architecture. It is not a JVM plugin host: Java `.jar` plugins written for the original Lavalink server cannot be loaded.

There are two extension paths:

1. **Built-in Rust plugins** are compiled into the RavaLink release and registered by the server.
2. **External process plugins** are independent executables discovered from the plugins directory and communicate with RavaLink using a versioned JSON-over-stdio protocol.

Choose the external process model when you want to distribute a plugin independently without linking it into the RavaLink workspace. Choose a built-in Rust plugin when you are contributing a first-party source integration or need direct access to the in-process plugin API.

## Built-in plugins

| Plugin | What it does | Typical identifiers |
|---|---|---|
| YouTube | Native YouTube search, video/playlist resolution and playable source selection | YouTube URLs, `ytsearch:`, `ytmsearch:` |
| LavaSrc | Resolves Spotify, Apple Music and Deezer metadata and matches tracks to playable YouTube audio | Spotify, Apple Music and Deezer URLs/URIs |
| LavaSearch | Track search and structured album, artist and playlist discovery | `spsearch:`, `amsearch:`, `dzsearch:`, `search:`, `album:`, `artist:`, `playlist:` |

Provider support can depend on upstream availability, API changes, regional restrictions and credentials. The presence of a source in the server's source-manager list is not a guarantee that every operation is available.

### Configure provider credentials

Spotify API-backed search and collection expansion use:

- `RAVALINK_SPOTIFY_CLIENT_ID`
- `RAVALINK_SPOTIFY_CLIENT_SECRET`

Apple Music playlist expansion uses:

- `RAVALINK_APPLE_MUSIC_TOKEN`

YouTube authentication is optional for ordinary requests. Browser cookies and Google OAuth Device Flow can be configured through the [configuration reference](../configuration/config/environment-variables.md). OAuth requires both the client ID and token-file path.

## Install an external plugin

1. Obtain a plugin executable built for the host OS/architecture.
2. Create a subdirectory under the configured plugin directory (default: `plugins/`).
3. Place a `ravalink-plugin.toml` manifest and the executable in that directory.
4. Set the executable path in the manifest. Relative paths are resolved from the plugin directory.
5. Restart RavaLink. Plugins are discovered at startup.

Example layout:

```text
plugins/
└── community-source/
    ├── ravalink-plugin.toml
    └── bin/
        └── community-source
```

Example manifest:

```toml
api_version = 1
name = "community-source"
version = "1.0.0"
description = "Example community source"
capabilities = ["track", "search"]
identifier_prefix = "community:"
program = "bin/community-source"
args = ["--stdio"]

[environment]
API_TOKEN = "set-this-through-your-deployment"

[config]
region = "eu"
```

The `config` table is sent in every request to the plugin. The `environment` map is passed to the child process. The host clears inherited environment variables, so only explicitly configured values and `RAVALINK_PLUGIN_PROTOCOL=1` are present.

You can also declare a plugin directly in `config/ravalink.toml` using `[[plugins]]`. In that form, `program` must be an absolute executable path. See the [external process protocol](external-process.md) for the full contract and a working Rust example.

## Develop a built-in Rust plugin

Built-in plugins are workspace crates implementing the `RavaLinkPlugin` trait from `ravalink-plugin-api`. They are registered by the server at startup and return `PluginLoadResult` values that serialize to Lavalink load-result objects.

Read [Native Rust plugin development](native-rust.md) for the trait, registration flow, test strategy and implementation checklist.

## Operational limits and security

- Plugin discovery happens at startup; restart the node after installing or updating an extension.
- Plugin names and identifier prefixes must be unique. The built-in names `youtube`, `lavasrc` and `lavasearch` are reserved.
- An external plugin is trusted executable code running with the same OS user and filesystem permissions as RavaLink. It is not sandboxed.
- External plugin calls have a 30-second wall-clock timeout, a 4 MiB stdout response limit and at most two concurrent child processes per plugin.
- A playback plugin must return a supported direct HTTP(S) audio stream URL. The server still applies its URL, redirect, format and decoder checks.
- Do not put provider secrets in a public manifest or commit them to Git.

## Compatibility note

RavaLink does not implement the original Lavalink Java plugin class loader. Porting a Java plugin requires reimplementing its behavior using either the native Rust API or the external process protocol. Check the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) before assuming feature parity with an upstream plugin.
