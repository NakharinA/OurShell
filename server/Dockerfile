# syntax=docker/dockerfile:1
#
# Build from the WORKSPACE ROOT:
#   docker build -f server/Dockerfile -t ourshell-server .
# (docker-compose.yml at the repo root already sets `context: .` and
# `dockerfile: server/Dockerfile` for you.)
#
# Everything here is pure-Rust TLS (rustls + ring, never aws-lc-sys/OpenSSL),
# so the only native build dependency is a C compiler for `ring`'s small
# amount of C/assembly.

FROM rust:1-slim-bookworm AS builder
WORKDIR /usr/src/ourshell

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential \
    && rm -rf /var/lib/apt/lists/*

# Copy the whole workspace: `ourshell-server` doesn't depend on
# `ourshell-core`/`ourshell-cli`/`ourshell-mcp`, so `-p ourshell-server`
# below only ever compiles this crate, `ourshell-proto`, and their own
# dependencies -- the other crates are never built here.
COPY . .

RUN cargo build --release --locked -p ourshell-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /app --shell /usr/sbin/nologin ourshell

WORKDIR /app
COPY --from=builder /usr/src/ourshell/target/release/ourshell-server /usr/local/bin/ourshell-server
COPY --from=builder /usr/src/ourshell/server/migrations /app/migrations

ENV MIGRATIONS_DIR=/app/migrations
ENV BIND_ADDR=0.0.0.0:8443

USER ourshell
EXPOSE 8443

ENTRYPOINT ["/usr/local/bin/ourshell-server"]
