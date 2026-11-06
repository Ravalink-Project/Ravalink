# Plugins and source integrations

The maintained plugin documentation has moved into a dedicated section:

- [Plugin overview and installation](plugins/index.md)
- [Native Rust plugin development](plugins/native-rust.md)
- [External process protocol and Rust example](plugins/external-process.md)

RavaLink does not load Java `.jar` plugins. It provides built-in Rust integrations and a versioned JSON-over-stdio interface for independently installed executable plugins.

For provider credentials and plugin discovery settings, see the [configuration reference](configuration/config/environment-variables.md).
