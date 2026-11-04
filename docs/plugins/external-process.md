# External Rust extensions

RavaLink supports community extensions as standalone Rust executables. This avoids
loading third-party Rust dynamic libraries into the server process, where compiler,
allocator and dependency-version differences can make a Rust ABI unsafe.

## Configuration

Add one `[[plugins]]` entry to `config/ravalink.toml`:

```toml
[[plugins]]
name = "community-source"
version = "1.0.0"
description = "Community source integration"
capabilities = ["track", "playlist", "search"]
identifier_prefix = "community:"
program = "/opt/ravalink/plugins/community-source"
args = ["--stdio"]

[plugins.environment]
API_TOKEN = "provided-by-the-deployment"
```

The executable path must be absolute. Plugin names and identifier prefixes must
be unique; built-in names (`youtube`, `lavasrc`, `lavasearch`) are reserved.
The process runs under the same operating-system account as RavaLink. It is not
a security sandbox: install only trusted extensions. The host clears inherited
environment variables and passes only the protocol marker plus the explicitly
configured environment map.

## Protocol v1

For each matching identifier, RavaLink starts the configured executable, writes
one JSON request to stdin, closes stdin, reads stdout, and waits for the process:

```json
{"protocolVersion":1,"identifier":"community:some-track"}
```

The executable must write exactly one JSON response to stdout. It may return
`null` when it does not handle the identifier, or a Lavalink-compatible result:

```json
{"loadType":"track","data":{"encoded":"...","info":{"identifier":"...","isSeekable":true,"author":"...","length":180000,"isStream":false,"position":0,"title":"...","uri":"https://...","sourceName":"community"},"pluginInfo":{"streamUrl":"https://public-cdn.example/audio.mp3"},"userData":{}}}
```

Supported `loadType` values are `track`, `playlist`, and `search`. Track
playback integrations must include a complete Lavalink track object and a playable
`pluginInfo.streamUrl` pointing to a supported direct HTTP(S) audio resource.
RavaLink applies its normal public-IP, redirect, download-size, format and decoder
checks to that URL.

The host accepts at most 4 MiB of stdout per invocation and applies a 30-second
wall-clock timeout to each plugin call. A timed-out child is killed. Stderr is
discarded so diagnostic output cannot corrupt the JSON protocol; send diagnostics
to a file or external logging service.

A minimal Rust implementation can use `serde` and `serde_json`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{self, Read};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    protocol_version: u32,
    identifier: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    load_type: &'static str,
    data: serde_json::Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let request: Request = serde_json::from_str(&input)?;
    if request.protocol_version != 1 || !request.identifier.starts_with("community:") {
        println!("null");
        return Ok(());
    }
    let response = Response {
        load_type: "search",
        data: json!({"tracks": [], "pluginInfo": {}}),
    };
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}
```

The sample demonstrates the wire protocol only. Real track playback requires a
complete Lavalink track and a supported public HTTP(S) stream URL.
