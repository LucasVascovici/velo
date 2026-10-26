#!/usr/bin/env bash
# Build the three images and smoke-test them. Needs docker; pushes nothing.
# Run from anywhere:  containers/smoke.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
tag=$$
net=velody-smoke-$tag
vol=velody-smoke-repo-$tag
srv=velody-smoke-srv-$tag
token=smoke-token-$tag
fail() { echo "FAIL: $*" >&2; exit 1; }

cleanup() {
  docker rm -f "$srv" >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
  docker volume rm "$vol" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for target in cli mcp serve-http; do
  docker build -f "$root/containers/Dockerfile" --target "$target" -t "velody-$target:smoke" "$root"
done

# 1. The CLI image runs, as a non-root user.
docker run --rm velody-cli:smoke --version | grep -q '^velody' || fail "velody --version"
uid=$(docker run --rm --entrypoint id velody-cli:smoke -u)
[ "$uid" != 0 ] || fail "cli image runs as root"
echo "ok: cli --version, non-root"

# 2. serve-http with a token over a fresh repository volume.
docker network create "$net" >/dev/null
docker volume create "$vol" >/dev/null
docker run --rm -v "$vol:/srv/repo" velody-cli:smoke init >/dev/null
docker run -d --name "$srv" --network "$net" -v "$vol:/srv/repo" \
  -e VELODY_SERVE_TOKEN="$token" velody-serve-http:smoke >/dev/null

# A client container: init, save, push without and with the token, then clone.
client='
set -euo pipefail
url=http://$SRV:8417/
for i in $(seq 1 30); do
  (exec 3<>/dev/tcp/$SRV/8417) 2>/dev/null && break
  sleep 1
done
mkdir /tmp/work && cd /tmp/work
velody init >/dev/null
echo hello > hello.txt
velody save smoke >/dev/null
velody remote add origin "$url" >/dev/null
if env -u VELODY_TOKEN velody push origin >/tmp/out 2>&1; then
  echo "FAIL: anonymous push accepted" >&2; exit 1
fi
echo "ok: anonymous push refused"
velody push origin >/dev/null
echo "ok: authorised push"
cd /tmp && velody clone "$url" cloned >/dev/null
[ -f cloned/hello.txt ] || { echo "FAIL: clone has no hello.txt" >&2; exit 1; }
echo "ok: clone over HTTP"
'
docker run --rm --network "$net" --entrypoint bash \
  -e SRV="$srv" -e VELODY_TOKEN="$token" velody-cli:smoke -c "$client" \
  || { docker logs "$srv" >&2 || true; fail "serve-http exchange"; }

# 3. velody-mcp: initialize plus tools/list over stdio.
out=$(printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | docker run --rm -i -v "$vol:/srv/repo" velody-mcp:smoke --repo /srv/repo)
echo "$out" | grep -q '"protocolVersion"' || fail "mcp initialize: $out"
echo "$out" | grep -q '"velody_save"' || fail "mcp tools/list: $out"
echo "ok: mcp initialize and tools/list"

echo "containers smoke: all checks passed"
