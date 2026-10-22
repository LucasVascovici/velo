# Container images

Three images from one `Dockerfile`, all running as a non-root user (uid 10001)
and carrying no secrets. CI builds and smoke-tests them (`containers.yml`) and
pushes nothing.

| Target | Entrypoint | Use |
| --- | --- | --- |
| `cli` | `velo` | the command-line tool |
| `mcp` | `velo-mcp` | the MCP server over stdio |
| `serve-http` | `velo serve-http /srv/repo --listen 0.0.0.0:8417` | the reference HTTP server |

## Build

```sh
docker build -f containers/Dockerfile --target cli        -t velo-cli .
docker build -f containers/Dockerfile --target mcp        -t velo-mcp .
docker build -f containers/Dockerfile --target serve-http -t velo-serve-http .
```

## Run

```sh
docker volume create velo-repo
docker run --rm -v velo-repo:/srv/repo velo-cli init
docker run -d -p 8417:8417 -v velo-repo:/srv/repo \
  -e VELO_SERVE_TOKEN="$(openssl rand -hex 16)" velo-serve-http
docker run --rm -i -v velo-repo:/srv/repo velo-mcp --repo /srv/repo
```

`serve-http` has no TLS: put a reverse proxy in front of anything public.
`containers/smoke.sh` runs the whole check locally (Linux or WSL, needs docker).

## Publishing (owner only)

The registry is `ghcr.io/lucasvascovici/velo` (see `docs/NAMES.md`; the name is
unverified until you log in). Nothing below runs in CI.

```sh
echo "$GITHUB_TOKEN" | docker login ghcr.io -u lucasvascovici --password-stdin
v=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
for t in cli mcp serve-http; do
  docker tag "velo-$t:smoke" "ghcr.io/lucasvascovici/velo-$t:$v"
  docker tag "velo-$t:smoke" "ghcr.io/lucasvascovici/velo-$t:latest"
  docker push "ghcr.io/lucasvascovici/velo-$t:$v"
  docker push "ghcr.io/lucasvascovici/velo-$t:latest"
done
```

Run `containers/smoke.sh` first; it leaves the `:smoke` tags in place.
