# syntax=docker/dockerfile:1
#
# Build from the WORKSPACE ROOT:
#   docker build -f server/Dockerfile -t ourshell-server .
# (docker-compose.yml at the repo root already sets `context: .` and
# `dockerfile: server/Dockerfile` for you.)
#
# Published as one multi-arch image (linux/amd64 + linux/arm64) at
# ghcr.io/nakharina/ourshell-server by .github/workflows/release.yml; each
# architecture is built natively, never under QEMU.
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
# dependencies -- the other crates are never built here. The binary embeds
# server/web/* and assets/logo/ourshell.svg (include_str!), so both must stay
# in the build context (see .dockerignore).
COPY . .

# The cargo registry and target dir live in BuildKit cache mounts so
# rebuilds are incremental; the binary is copied out of the cache mount
# because cache mounts are not part of the resulting layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/usr/src/ourshell/target,sharing=locked \
    cargo build --release --locked -p ourshell-server \
    && cp target/release/ourshell-server /usr/local/bin/ourshell-server

FROM debian:bookworm-slim AS runtime

ARG VERSION=dev

LABEL org.opencontainers.image.source="https://github.com/NakharinA/OurShell" \
      org.opencontainers.image.title="OurShell sync server" \
      org.opencontainers.image.description="Self-hosted, end-to-end encrypted sync server for OurShell" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /app --shell /usr/sbin/nologin ourshell \
    && mkdir -p /data \
    && chown ourshell:ourshell /data \
    && chmod 0700 /data

WORKDIR /app
COPY --from=builder /usr/local/bin/ourshell-server /usr/local/bin/ourshell-server
COPY --from=builder /usr/src/ourshell/server/migrations /app/migrations

# config.toml, the setup code and other server state live in /data; a fresh
# named volume inherits the directory's ownership, so it is writable by
# `ourshell`.
ENV OURSHELL_DATA_DIR=/data \
    MIGRATIONS_DIR=/app/migrations \
    BIND_ADDR=0.0.0.0:8443
VOLUME /data

USER ourshell
# 8443: HTTP (or the setup wizard). 8444: HTTPS when Let's Encrypt is
# enabled (OURSHELL_TLS_BIND).
EXPOSE 8443 8444

# No curl in the image: the server binary checks /v1/health itself.
HEALTHCHECK --interval=30s --timeout=10s --start-period=20s --start-interval=2s --retries=3 \
    CMD ["/usr/local/bin/ourshell-server", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/ourshell-server"]
CMD ["serve"]
