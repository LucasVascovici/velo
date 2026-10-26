# Hosting a Velody remote with `serve-http`

A copyable deployment: `velody serve-http` behind a Caddy reverse proxy, with
bearer tokens, plus a read-only mirror variant. The CI workflow
`deploy-example` runs this stack against the real binary on every change.

## Run it

```sh
cd examples/serve-http
mkdir -p bin repos
cp ../../target/release/velody bin/velody   # cargo build --release, or download
                                        # a Linux binary from the release page
cp tokens.example tokens                # then replace the tokens
docker compose up -d --wait
./smoke.sh                              # optional: checks the whole policy
```

The binary must be a Linux build (it runs in `debian:bookworm-slim`).
Repositories live in `./repos/<name>`; create one with `velody init` inside
`repos/<name>`, then clients use `velody clone http://localhost:8080/<name>/`.

## Where TLS terminates

At the proxy. `velody serve-http` speaks plain HTTP on the internal compose
network and publishes no port. The `Caddyfile` listens on plain `:8080` so the
example runs anywhere; for production use the commented domain block, and Caddy
gets and renews a certificate by itself (publish ports 80 and 443).

## Tokens

`tokens` holds lines `read <token>` and `write <token>`. Clients send theirs
as `VELODY_TOKEN=<token> velody push`. A request with no token, or a push with a
read token, is refused with an access-denied error. `tokens.example` holds
placeholders: never commit real tokens (`tokens` is git-ignored).

## Read-only mirror

```sh
docker compose -f compose.mirror.yml up -d --wait
```

Same server with `--read-only`: fetches and clones work, every push is refused
even with a write token. Point it at the repositories a read-write stack
populated, or at a copy synced from one.

## Limits

- The `request_body { max_size 1GB }` in the Caddyfile is required. tiny_http
  drains an unread body of the declared length even after velody answers 413, so
  without a proxy limit a huge request can abort the server. Keep it at or
  below `--max-request-bytes` (default 1 GiB).
- The proxy's timeouts are the only bound on a fully stalled client.
- By default the server serves 8 connections at once (`--max-connections`).
