# syntax=docker/dockerfile:1
FROM rust:1.99-bookworm AS builder

RUN apt-get update \
    && apt-get install --yes --no-install-recommends build-essential cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
RUN cargo build --release -p ravalink-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 ravalink \
    && useradd --system --uid 10001 --gid ravalink --home-dir /nonexistent --shell /usr/sbin/nologin ravalink

COPY --from=builder --chown=ravalink:ravalink /build/target/release/ravalink /usr/local/bin/ravalink
COPY --chown=ravalink:ravalink config/ravalink.example.toml /etc/ravalink/ravalink.toml

ENV RAVALINK_CONFIG=/etc/ravalink/ravalink.toml
EXPOSE 2333/tcp
USER ravalink:ravalink
ENTRYPOINT ["/usr/local/bin/ravalink"]
