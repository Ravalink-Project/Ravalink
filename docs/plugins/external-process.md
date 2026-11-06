---
description: Install and build independent RavaLink plugins using the JSON-over-stdio protocol.
---

# External process plugin protocol

External plugins are standalone executables launched by RavaLink when an identifier matches their configured prefix. The host and plugin exchange one JSON request and one JSON response over standard input/output.

This is the recommended way to distribute a community resolver independently from the RavaLink release. It is not a Java plugin loader and does not provide arbitrary access to the server's internals.

## 1. Install a plugin

The default plugin directory is `plugins/`, relative to the process working directory. Override it with `RAVALINK_PLUGINS_DIRECTORY` or `plugin_directory` in TOML.

Create one folder per plugin:

```text
plugins/
└── community-source/
    ├── ravalink-plugin.toml
    └── bin/
        └── community-source
```

The executable must be compatible with the host operating system, CPU architecture and userspace. On Linux, mark it executable:

```bash
chmod 0755 plugins/community-source/bin/community-source
```

## 2. Write the manifest

Create `plugins/community-source/ravalink-plugin.toml`:

```toml
api_version = 1
name = "community-source"
version = "1.0.0"
description = "Community source integration"
capabilities = ["track", "playlist", "search", "source:community"]
identifier_prefix = "community:"
program = "bin/community-source"
args = ["--stdio"]

[environment]
API_TOKEN = "provide-this-through-a-secret-manager"

[config]
region = "eu"
```

| Field | Required | Meaning |
|---|---|---|
| `api_version` | No | Protocol version; currently `1` |
| `name` | Yes | Unique plugin name |
| `version` | Yes | Plugin version string |
| `description` | Yes | Human-readable description |
| `capabilities` | No | Descriptive capability labels |
| `identifier_prefix` | Yes | Prefix used to route identifiers to this plugin |
| `program` | Yes | Executable path; relative to the plugin folder or absolute |
| `args` | No | Arguments passed directly to the executable |
| `environment` | No | Explicit environment variables passed to the process |
| `config` | No | Arbitrary TOML value serialized into the request's `config` field |

Plugin names and prefixes must not conflict with existing plugins. Built-in names `youtube`, `lavasrc` and `lavasearch` are reserved. Discovery is one directory level deep and folders are processed in sorted order.

### Alternative: declare the plugin in the main TOML file

Instead of a manifest, add a direct declaration to `config/ravalink.toml`:

```toml
[[plugins]]
name = "community-source"
version = "1.0.0"
description = "Community source integration"
capabilities = ["track", "search"]
identifier_prefix = "community:"
program = "/opt/ravalink/plugins/community-source"
args = ["--stdio"]

[plugins.environment]
API_TOKEN = "injected-by-deployment"

[plugins.config]
region = "eu"
```

For direct declarations, `program` must be an absolute path. Use either directory discovery or a direct declaration for a plugin, not both.

## 3. Request contract

When RavaLink receives an identifier beginning with the configured prefix, it starts the executable, writes one JSON object to stdin, then closes stdin:

```json
{
  "protocolVersion": 1,
  "identifier": "community:some-track",
  "config": {
    "region": "eu"
  }
}
```

The process must read the complete request from stdin. It must write exactly one JSON response to stdout and then exit. Do not print logs, progress bars or debug output to stdout. Stderr is discarded by the current host implementation; use an external logger or a plugin-owned log file for diagnostics.

The process receives a cleared environment plus the variables explicitly listed in the manifest and `RAVALINK_PLUGIN_PROTOCOL=1`. Do not rely on PATH, HOME or other inherited variables being present.

## 4. Response contract

The response is either JSON `null` (the plugin declines the identifier) or a `PluginLoadResult` object:

```json
{
  "loadType": "track",
  "data": {
    "encoded": "BASE64_LAVALINK_TRACK",
    "info": {
      "identifier": "track-id",
      "isSeekable": true,
      "author": "Example Artist",
      "length": 180000,
      "isStream": false,
      "position": 0,
      "title": "Example Track",
      "uri": "https://provider.example/track/track-id",
      "artworkUrl": null,
      "isrc": null,
      "sourceName": "community"
    },
    "pluginInfo": {
      "streamUrl": "https://cdn.example/audio.mp3"
    },
    "userData": {}
  }
}
```

Supported `loadType` values:

| Value | Shape of `data` |
|---|---|
| `track` | One complete Lavalink track object |
| `playlist` | Object containing `info`, `pluginInfo` and `tracks` |
| `search` | Array of Lavalink track objects |

For playback, include a playable direct HTTP(S) audio URL in `pluginInfo.streamUrl`. RavaLink validates the destination and redirects and then uses its own supported source/decoder pipeline. A webpage URL, provider landing page or metadata-only response is not a playable stream. A plugin cannot currently inject a custom decoder or replace the audio engine.

If the plugin does not own the identifier, return:

```json
null
```

If it owns the identifier but cannot resolve it, exit unsuccessfully or return a structured failure through the host's error path; do not silently return a fake track.

## 5. Minimal Rust executable

This example uses `serde` and `serde_json`. It demonstrates the process protocol and returns an empty search result; it is not a complete media provider.

```toml
[package]
name = "community-source"
version = "0.1.0"
edition = "2024"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

```rust
use serde::Deserialize;
use serde_json::json;
use std::io::{self, Read};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    protocol_version: u32,
    identifier: String,
    config: Option<serde_json::Value>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let request: Request = serde_json::from_str(&input)?;

    if request.protocol_version != 1
        || !request.identifier.starts_with("community:")
    {
        println!("null");
        return Ok(());
    }

    let response = json!({
        "loadType": "search",
        "data": [],
    });

    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}
```

Build the executable for the deployment target and place it at the path declared by `program`. For a Pelican/Pterodactyl server, the binary must target the container's Linux architecture (AMD64 or ARM64) and be uploaded under `/home/container/plugins/`.

## 6. Runtime limits

The current host enforces:

| Limit | Value |
|---|---:|
| Plugin identifier length | 16 KiB |
| Maximum stdout response | 4 MiB |
| Concurrent child processes per plugin | 2 |
| Wall-clock timeout per call | 30 seconds |

A timed-out child is killed. The host reads stdout as the protocol response and does not merge stderr into it.

## 7. Security model

External plugins are trusted executables, not sandboxed code. They run with the same operating-system user and filesystem permissions as RavaLink.

- Install only plugins from sources you trust.
- Keep API tokens out of public manifests and Git repositories.
- Grant the server user access only to the plugin files and directories it needs.
- Do not assume the configured environment map is a secret vault; panel variables and secret managers are preferable for credentials.
- The host clears inherited environment variables, but the plugin still has the process user's filesystem and network access.

## 8. Troubleshooting

| Symptom | Likely cause |
|---|---|
| Plugin is not discovered | Wrong plugin directory, missing manifest, or folder nested too deeply |
| Manifest parse error | Invalid TOML or missing required metadata |
| Executable cannot be resolved | Incorrect relative path or missing file |
| Permission denied | Executable bit missing or filesystem mount is not executable |
| Exec format error | Binary built for a different OS/CPU architecture |
| Invalid JSON response | Logs or extra text were printed to stdout |
| Response exceeds limit | Plugin emitted more than 4 MiB |
| Plugin times out | Provider call is too slow or the plugin is blocked |
| Track loads but playback fails | Missing/invalid `pluginInfo.streamUrl`, unsupported media format, blocked host or decoder limitation |

Restart RavaLink after changing a manifest or replacing a plugin executable.
