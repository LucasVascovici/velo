# Container images for velody. Build one target:
#   docker build -f containers/Dockerfile --target cli -t velody-cli .
# Targets: cli, mcp, serve-http. Nothing here pushes anywhere; see README.md.

# 1.88 is the workspace MSRV (Cargo.toml rust-version).
FROM rust:1.88-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p velody-cli -p velody-mcp

# Shared runtime base: a non-root user and no secrets. Tokens are passed at
# run time (VELODY_SERVE_TOKEN, VELODY_TOKEN), never baked into a layer.
FROM debian:bookworm-slim AS base
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/velody velody \
    && mkdir -p /srv/repo \
    && chown -R velody:velody /srv
LABEL org.opencontainers.image.source="https://github.com/LucasVascovici/velody" \
      org.opencontainers.image.vendor="velody"
USER velody
WORKDIR /srv/repo

FROM base AS cli
LABEL org.opencontainers.image.title="velody" \
      org.opencontainers.image.description="The velody command-line tool"
COPY --from=builder /src/target/release/velody /usr/local/bin/velody
ENTRYPOINT ["velody"]

FROM base AS mcp
LABEL org.opencontainers.image.title="velody-mcp" \
      org.opencontainers.image.description="velody MCP server (stdio)"
COPY --from=builder /src/target/release/velody-mcp /usr/local/bin/velody-mcp
ENTRYPOINT ["velody-mcp"]

FROM base AS serve-http
LABEL org.opencontainers.image.title="velody serve-http" \
      org.opencontainers.image.description="velody reference HTTP server"
COPY --from=builder /src/target/release/velody /usr/local/bin/velody
EXPOSE 8417
VOLUME /srv
ENTRYPOINT ["velody", "serve-http"]
CMD ["/srv/repo", "--listen", "0.0.0.0:8417"]
