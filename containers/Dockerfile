# Container images for velo. Build one target:
#   docker build -f containers/Dockerfile --target cli -t velo-cli .
# Targets: cli, mcp, serve-http. Nothing here pushes anywhere; see README.md.

# 1.88 is the workspace MSRV (Cargo.toml rust-version).
FROM rust:1.88-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p velo-cli -p velo-mcp

# Shared runtime base: a non-root user and no secrets. Tokens are passed at
# run time (VELO_SERVE_TOKEN, VELO_TOKEN), never baked into a layer.
FROM debian:bookworm-slim AS base
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/velo velo \
    && mkdir -p /srv/repo \
    && chown -R velo:velo /srv
LABEL org.opencontainers.image.source="https://github.com/LucasVascovici/velo" \
      org.opencontainers.image.vendor="velo"
USER velo
WORKDIR /srv/repo

FROM base AS cli
LABEL org.opencontainers.image.title="velo" \
      org.opencontainers.image.description="The velo command-line tool"
COPY --from=builder /src/target/release/velo /usr/local/bin/velo
ENTRYPOINT ["velo"]

FROM base AS mcp
LABEL org.opencontainers.image.title="velo-mcp" \
      org.opencontainers.image.description="velo MCP server (stdio)"
COPY --from=builder /src/target/release/velo-mcp /usr/local/bin/velo-mcp
ENTRYPOINT ["velo-mcp"]

FROM base AS serve-http
LABEL org.opencontainers.image.title="velo serve-http" \
      org.opencontainers.image.description="velo reference HTTP server"
COPY --from=builder /src/target/release/velo /usr/local/bin/velo
EXPOSE 8417
VOLUME /srv
ENTRYPOINT ["velo", "serve-http"]
CMD ["/srv/repo", "--listen", "0.0.0.0:8417"]
