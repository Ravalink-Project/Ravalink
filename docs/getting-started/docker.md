# Run RavaLink with Docker

RavaLink is built as a native Rust binary. No Java runtime or JVM image is used.

## Build and start

Copy the example configuration and set a strong password:

```sh
cp config/ravalink.example.toml ravalink.toml
```

Set the password in the environment and start the container:

```sh
export RAVALINK_PASSWORD="$(openssl rand -hex 32)"
docker compose up -d --build
```

The supplied `compose.yml` binds port 2333 to `127.0.0.1` by default. If the node must be reachable from another machine, change the host binding deliberately and restrict access with a firewall or reverse proxy.

The container runs as an unprivileged `ravalink` user. Its configuration is mounted read-only at `/etc/ravalink/ravalink.toml`; environment variables override values from the file.

Compose also mounts the named `ravalink-data` volume at `/data`. This keeps provider state, including the YouTube OAuth refresh token, across container recreation. The default token path is `data/youtube-oauth.json`, which resolves to `/data/youtube-oauth.json` in the container.

## Configuration

The compose file requires `RAVALINK_PASSWORD`. Other supported variables are documented in the [README](../../README.md#configuration). Prometheus metrics are disabled by default and should not be exposed publicly without access controls.

To enable Lavalink-compatible YouTube TV authentication, set this in `ravalink.toml`:

```toml
[youtube.oauth]
enabled = true
```

On startup, RavaLink prints a device URL and code. Open [google.com/device](https://google.com/device), enter the code, and authorize with a secondary YouTube account. No Google API key or user-created OAuth client is required. The refresh token is stored in the persistent data volume.

## Manual build

```sh
docker build -t ravalink:local .
docker volume create ravalink-data

docker run --rm \
  -p 127.0.0.1:2333:2333 \
  -v ravalink-data:/data \
  -e RAVALINK_PASSWORD="$(openssl rand -hex 32)" \
  ravalink:local
```

For persistent configuration, mount a TOML file at `/etc/ravalink/ravalink.toml`. Keep the `ravalink-data` volume mounted whenever YouTube OAuth is enabled.
