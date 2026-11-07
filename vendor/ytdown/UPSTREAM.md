# Vendored ytdown

- Upstream: https://github.com/4thel00z/ytdown
- Version: 0.8.1
- Upstream revision: a65d02d3cf3d62880eeedab3b2bc267addd29f08
- License: MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE).

## Local patch

RavaLink adds an optional YouTube TV OAuth token to the YouTube extractor. The
token is attached only to the `/youtubei/v1/player` request when the selected
InnerTube client is `TVHTML5` (client id 7). Search, playlists, visitor-data,
player JavaScript, Android, iOS, and other requests remain unauthenticated.

This mirrors Lavalink's `YoutubeHttpContextFilter` behavior and avoids a
global `Authorization` header on the HTTP client.
