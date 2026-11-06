---
description: Develop first-party and workspace-native RavaLink plugins in Rust.
---

# Native Rust plugin development

This guide is for integrations compiled into the RavaLink workspace. It describes the in-process API; it is different from the independently deployed [external process plugin protocol](external-process.md).

## Architecture

The shared crate is `ravalink-plugin-api`. A native plugin implements:

- `descriptor()`: stable name, version, description and advertised capabilities.
- `supports_identifier()`: whether the plugin owns an identifier or search prefix.
- `load_tracks()`: resolve the identifier into a Lavalink-compatible result, return `None` when it does not handle the identifier, or return a `PluginError`.

The trait is asynchronous and requires `Send + Sync`. The registry tries matching plugins in registration order. A plugin resolution is bounded by the registry's default 30-second timeout.

## Workspace setup

Create a crate under `crates/plugins/` and add it to the workspace members in the root `Cargo.toml`.

A plugin crate typically depends on:

```toml
[dependencies]
async-trait = { workspace = true }
serde_json = { workspace = true }
ravalink-plugin-api = { path = "../plugin-api" }
ravalink-protocol = { path = "../../protocol" }
```

Adjust relative paths if the crate is nested differently. Keep shared dependency versions in the workspace manifest.

## Minimal plugin implementation

The following example handles a custom identifier and returns an empty search result. It demonstrates registration metadata and dispatch; it does not resolve real media.

```rust
use async_trait::async_trait;
use ravalink_plugin_api::{
    PluginDescriptor, PluginError, PluginLoadResult, PluginLoadType, RavaLinkPlugin,
};

pub struct ExamplePlugin;

#[async_trait]
impl RavaLinkPlugin for ExamplePlugin {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            name: "example".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Example RavaLink source plugin".into(),
            capabilities: vec!["search".into()],
        }
    }

    fn supports_identifier(&self, identifier: &str) -> bool {
        identifier.starts_with("example:")
    }

    async fn load_tracks(
        &self,
        identifier: &str,
    ) -> Result<Option<PluginLoadResult>, PluginError> {
        let query = identifier
            .strip_prefix("example:")
            .unwrap_or_default()
            .trim();

        if query.is_empty() {
            return Ok(Some(PluginLoadResult::new(
                PluginLoadType::Search,
                serde_json::json!([]),
            )));
        }

        // Replace this placeholder with provider lookup and track conversion.
        Ok(Some(PluginLoadResult::new(
            PluginLoadType::Search,
            serde_json::json!([]),
        )))
    }
}
```

The example uses `serde_json::json!`; add `serde_json` as a dependency. For real results, return the expected Lavalink payload:

- `Track`: one complete encoded track object.
- `Playlist`: an object containing playlist `info`, `pluginInfo` and a `tracks` array.
- `Search`: an array of track objects.

Use the types and encoding helpers in `ravalink-protocol` rather than hand-building encoded track strings.

## Register the plugin

The server constructs a `PluginRegistry` and registers the built-in plugins during startup. To integrate a new first-party crate:

1. Add the crate as a dependency of `crates/server/Cargo.toml`.
2. Construct the plugin during server initialization.
3. Register it with `PluginRegistry::register` (or `register_shared` for an existing `Arc`).
4. Ensure its identifier prefixes do not overlap with existing source managers.
5. Add it to the server's source-manager information and startup tests if the integration is intended to be publicly advertised.

Follow the existing YouTube, LavaSrc and LavaSearch crate patterns for dependency injection, HTTP client timeouts, response-size limits and provider error mapping.

## Track resolution responsibilities

A production source plugin should:

1. Parse only identifiers it explicitly supports.
2. Validate identifier length and input shape before network requests.
3. Use bounded HTTP timeouts and response sizes.
4. Handle provider rate limits, missing media and malformed responses without panicking.
5. Return a complete Lavalink track object with correct source name, identifier, duration, seekability, URI and artwork metadata where available.
6. Preserve provider-specific metadata in `pluginInfo`.
7. Ensure the audio pipeline receives a playable source supported by the server's current resolver and decoder.
8. Return `Ok(None)` only when the plugin does not own the identifier; return a `PluginError` for a failed resolution it does own.

Do not assume that returning metadata alone makes a track playable. The playback path must be supported by the current audio source pipeline.

## Tests

At minimum, add tests for:

- Identifier parsing, prefix matching and invalid inputs.
- Track, playlist and search result serialization.
- Provider errors, empty results, timeouts and oversized responses.
- Metadata correctness and playable source selection.
- Registration and interaction with other source managers.

Run the workspace checks before opening a pull request:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

## When to use an external plugin instead

If the plugin should be installed without recompiling RavaLink, implement the [external process protocol](external-process.md). It is a stable JSON boundary and avoids linking a third-party Rust dynamic library into the server process. It is currently a resolver interface, not a general-purpose hook into the audio engine, Discord voice transport or REST router.
