# Container images

Three images from one `Dockerfile`, all running as a non-root user (uid 10001)
and carrying no secrets. CI builds and smoke-tests them (`containers.yml`) and
pushes nothing.

| Target | Entrypoint | Use |
| --- | --- | --- |
| `cli` | `velody` | the command-line tool |
| `mcp` | `velody-mcp` | the MCP server over stdio |
| `serve-http` | `velody serve-http /srv/repo --listen 0.0.0.0:8417` | the reference HTTP server |

## Build

```sh
docker build -f containers/Dockerfile --target cli        -t velody-cli .
docker build -f containers/Dockerfile --target mcp        -t velody-mcp .
docker build -f containers/Dockerfile --target serve-http -t velody-serve-http .
```

## Run

```sh
docker volume create velody-repo
docker run --rm -v velody-repo:/srv/repo velody-cli init
docker run -d -p 8417:8417 -v velody-repo:/srv/repo \
  -e VELODY_SERVE_TOKEN="$(openssl rand -hex 16)" velody-serve-http
docker run --rm -i -v velody-repo:/srv/repo velody-mcp --repo /srv/repo
```

`serve-http` has no TLS: put a reverse proxy in front of anything public.
`containers/smoke.sh` runs the whole check locally (Linux or WSL, needs docker).

## Publishing (owner only)

The registry is `ghcr.io/lucasvascovici/velody` (see `docs/NAMES.md`; the name is
unverified until you log in). Nothing below runs in CI.

```sh
echo "$GITHUB_TOKEN" | docker login ghcr.io -u lucasvascovici --password-stdin
v=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
for t in cli mcp serve-http; do
  docker tag "velody-$t:smoke" "ghcr.io/lucasvascovici/velody-$t:$v"
  docker tag "velody-$t:smoke" "ghcr.io/lucasvascovici/velody-$t:latest"
  docker push "ghcr.io/lucasvascovici/velody-$t:$v"
  docker push "ghcr.io/lucasvascovici/velody-$t:latest"
done
```

Run `containers/smoke.sh` first; it leaves the `:smoke` tags in place.
