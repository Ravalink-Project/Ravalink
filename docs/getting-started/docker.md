---
description: Deploy RavaLink using the official GHCR image.
---

# Docker deployment

RavaLink publishes an official multi-architecture container image for **Linux AMD64 and ARM64**:

```text
ghcr.io/ravalink-project/ravalink:latest
```

For a version-pinned deployment, use the matching release tag, for example:

```text
ghcr.io/ravalink-project/ravalink:v1.0.0-rc.1
```

The published image is the same runtime image used by the official Pelican/Pterodactyl egg. It contains the native RavaLink binary and a panel-compatible entrypoint. You do not need to clone the repository, compile Rust, install Java, or build a local Dockerfile to run a published release.

> [!IMPORTANT]
> RavaLink is still a release candidate. Check the [compatibility ledger](https://github.com/Ravalink-Project/Ravalink/blob/master/COMPATIBILITY.md) before using it for production workloads.

## Recommended: Pelican or Pterodactyl

If you manage containers through a game panel, use the official egg rather than creating a generic Docker container manually.

1. Open the [RavaLink-eggs repository](https://github.com/Ravalink-Project/Ravalink-eggs).
2. Import the panel-specific egg:
   - Pelican: `egg-ravalink-pelican.json` (native `PLCN_v3` export).
   - Pterodactyl: `egg-ravalink.json` (legacy-compatible `PTDL_v2` export).
3. Create a server and select the **RavaLink official** image:
   `ghcr.io/ravalink-project/ravalink:latest`.
4. Allocate the server port (default `2333`).
5. Set a unique `RAVALINK_PASSWORD` of at least 16 characters.
6. Start the server.

The panel pulls the image from GHCR. The package must be public so Wings can pull it without registry credentials. The egg's startup command is `ravalink`; the image entrypoint expands the panel's `STARTUP` variable and keeps the process attached to the container console.

See [Pelican and Pterodactyl deployment](../deployment/pelican-pterodactyl.md) for the full panel workflow, variables, plugin installation and updates.

## Run the official image with Docker Compose

For a standalone Docker host, use the published image directly. The following Compose file keeps the HTTP/WebSocket port bound to localhost and persists the configuration and plugin directory.

Create `compose.yml`:

```yaml
services:
  ravalink:
    image: ghcr.io/ravalink-project/ravalink:latest
    container_name: ravalink
    restart: unless-stopped
    environment:
      STARTUP: "ravalink"
      RAVALINK_ADDRESS: "0.0.0.0"
      RAVALINK_PORT: "2333"
      RAVALINK_PASSWORD: "REPLACE_WITH_A_LONG_RANDOM_SECRET"
      RAVALINK_PLUGINS_DIRECTORY: "/home/container/plugins"
    ports:
      - "127.0.0.1:2333:2333"
    volumes:
      - ./config:/home/container/config
      - ./plugins:/home/container/plugins
```

Create the directories and start the node:

```bash
mkdir -p config plugins
docker compose up -d
docker compose logs -f ravalink
```

Generate a secret instead of keeping the example value:

```bash
openssl rand -hex 32
```

Place the generated value in your Compose environment or a protected `.env` file. Do not commit secrets to source control.

The image's panel-compatible entrypoint expects `STARTUP`. Set it to `ravalink` when running the image outside a panel. The process reads `config/ravalink.toml` relative to `/home/container` if that file exists; otherwise built-in defaults are used and environment variables provide the deployment configuration.

## Run with docker run

```bash
docker run -d \
  --name ravalink \
  --restart unless-stopped \
  -e STARTUP=ravalink \
  -e RAVALINK_ADDRESS=0.0.0.0 \
  -e RAVALINK_PORT=2333 \
  -e RAVALINK_PASSWORD="$(openssl rand -hex 32)" \
  -p 127.0.0.1:2333:2333 \
  -v "$PWD/config:/home/container/config" \
  -v "$PWD/plugins:/home/container/plugins" \
  ghcr.io/ravalink-project/ravalink:latest
```

## Networking and security

- Port `2333/tcp` serves both the REST API and the Lavalink WebSocket endpoint.
- Keep the host binding on `127.0.0.1` when a reverse proxy runs on the same machine.
- If remote clients connect directly, expose the port deliberately and restrict it with a firewall. Prefer TLS through a reverse proxy.
- The `Authorization` header contains the node password. Never publish it in bot logs, screenshots or issue reports.
- Prometheus metrics are disabled by default. If enabled, the metrics endpoint has no separate authentication; do not expose it publicly.
- Keep OAuth token files, browser cookies and provider credentials private.

## Updating

For an automatically updated deployment, use `latest`. For reproducible deployments, pin a version tag.

- Compose: run `docker compose pull` and then `docker compose up -d`.
- Docker: pull the new tag and recreate the container with the same environment and volumes.
- Pelican/Pterodactyl: use the panel's image refresh/reinstall workflow so Wings pulls the new image. Server files and variables should be preserved by the panel; verify your panel's reinstall behavior before proceeding.

Every published release is intended to publish both the versioned image and the moving `latest` tag for AMD64 and ARM64. Check the [release workflow](https://github.com/Ravalink-Project/Ravalink/actions/workflows/release.yml) and [GHCR package](https://github.com/orgs/Ravalink-Project/packages/container/package/ravalink) if an image tag is not available.

## Build from source instead

Building locally is useful for development, but it is not the normal deployment path. Follow [Build from source](binary.md) for the Rust toolchain and Cargo commands.
