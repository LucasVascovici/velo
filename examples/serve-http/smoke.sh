#!/bin/sh
# Smoke test for the serve-http example. Run from examples/serve-http against a
# running stack:  ./smoke.sh            (read-write stack)
#                 ./smoke.sh mirror     (read-only mirror stack)
set -eu

mode=${1:-rw}
here=$(cd "$(dirname "$0")" && pwd)
velody=${VELO:-$here/bin/velody}
base=${BASE_URL:-http://localhost:8080}
repo=demo
read_token=$(awk '$1=="read"{print $2}' "$here/tokens")
write_token=$(awk '$1=="write"{print $2}' "$here/tokens")

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

# The server needs an initialised repository to push into; the rw stack
# creates it in the shared ./repos directory, the mirror reuses it.
if [ "$mode" != mirror ] && [ ! -f "$here/repos/$repo/.velo/velo.db" ]; then
  mkdir -p "$here/repos/$repo"
  (cd "$here/repos/$repo" && "$velody" init >/dev/null)
fi

# Wait for the proxy to answer at all.
i=0
until curl -s -o /dev/null "$base/"; do
  i=$((i + 1)); [ $i -lt 30 ] || fail "proxy not answering at $base"
  sleep 1
done

mkdir "$work/src"
cd "$work/src"
"$velody" init >/dev/null
echo "hello" > hello.txt
"$velody" save "smoke" >/dev/null
"$velody" remote add origin "$base/$repo/" >/dev/null

if [ "$mode" = mirror ]; then
  if VELODY_TOKEN=$write_token "$velody" push origin >"$work/out" 2>&1; then
    fail "mirror accepted a push with the write token"
  fi
  echo "ok: mirror refused a push with the write token"
else
  VELODY_TOKEN=$write_token "$velody" push origin >"$work/out" 2>&1 \
    || { cat "$work/out" >&2; fail "push with the write token failed"; }
  echo "ok: push with the write token"

  if "$velody" push origin >"$work/out" 2>&1; then fail "anonymous push accepted"; fi
  grep -qi "refused access" "$work/out" || { cat "$work/out" >&2; fail "anonymous push: no access-denied error"; }
  echo "ok: anonymous push refused"

  if VELODY_TOKEN=$read_token "$velody" push origin >"$work/out" 2>&1; then
    fail "push with the read token accepted"
  fi
  grep -qi "refused access" "$work/out" || { cat "$work/out" >&2; fail "read-token push: no access-denied error"; }
  echo "ok: push with the read token refused"
fi

cd "$work"
VELODY_TOKEN=$read_token "$velody" clone "$base/$repo/" cloned >"$work/out" 2>&1 \
  || { cat "$work/out" >&2; fail "clone with the read token failed"; }
[ -f cloned/hello.txt ] || fail "clone has no hello.txt"
echo "ok: clone with the read token"
echo "smoke ($mode): all checks passed"
