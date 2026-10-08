#!/usr/bin/env bash
# Cortex canary for the server image, run with the supervisor's container flags.
#   1. label/user check (scripts/verify_image.sh)
#   2. no secrets: /version 200 with slug+contract, /health 503, root filesystem read-only
#   3. secrets mounted read-only at /run/secrets: /health 200, get_weights 200 (401 bad token)
# Exit 0 only if every assertion passes.  IMAGE (default hypertrain:local), ENGINE (docker, as the supervisor;
# podman 5 rejects the uid= tmpfs option), HOST_PORT (18000; another if taken).
set -euo pipefail
cd "$(dirname "$0")/.."
image=${IMAGE:-hypertrain:local}
engine=${ENGINE:-docker}
port=${HOST_PORT:-18000}
name=hypertrain-canary-$$
base=http://127.0.0.1:$port
secrets=$(mktemp -d)
cleanup() {
  "$engine" logs "$name" 2>&1 | sed 's/^/  [container] /' || true
  "$engine" rm -f "$name" >/dev/null 2>&1 || true
  rm -rf "$secrets"
}
trap cleanup EXIT

pass() { echo "PASS: $*"; }
die() { echo "FAIL: $*" >&2; exit 1; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

start() { # extra run args...
  "$engine" rm -f "$name" >/dev/null 2>&1 || true
  "$engine" run -d --name "$name" --read-only --user 65532:65532 --cap-drop ALL \
    --security-opt no-new-privileges --tmpfs /tmp --tmpfs /data:uid=65532,gid=65532 \
    -p "127.0.0.1:$port:8000" "$@" "$image" >/dev/null
  for _ in $(seq 120); do
    [ "$(code "$base/version")" = 200 ] && return 0
    sleep 1
  done
  die "/version never answered 200"
}

ENGINE="$engine" scripts/verify_image.sh "$image" && pass "image labels and user"

start
curl -fsS "$base/version" | jq -e '.slug == "hypertrain" and .contract == 1' >/dev/null \
  || die "/version body"
pass "/version 200 slug=hypertrain contract=1 (no secrets)"
h=$(code "$base/health"); [ "$h" = 503 ] || die "/health without secrets is $h, expected 503"
pass "/health 503 without secrets"
w=$(code -H 'authorization: Bearer x' -H 'x-platform-challenge-slug: hypertrain' \
  "$base/internal/v1/get_weights?epoch=1")
[ "$w" = 503 ] || die "get_weights without secrets is $w, expected 503"
pass "get_weights 503 without secrets"
ro=$("$engine" exec "$name" python -c "
import errno
try:
    open('/tmp/../canary-write', 'w')
except OSError as e:
    print(errno.errorcode.get(e.errno))
else:
    print('WRITABLE')" || true)
# EROFS (not EACCES) proves the root is mounted read-only, not merely root-owned.
[ "$ro" = EROFS ] || die "root filesystem write gave '$ro', expected EROFS"
[ "$("$engine" inspect "$name" --format '{{.HostConfig.ReadonlyRootfs}}')" = true ] \
  || die "ReadonlyRootfs is not set"
pass "root filesystem write refused with EROFS"
"$engine" exec "$name" python -c "open('/data/canary-write', 'w').write('ok')" \
  || die "/data is not writable for 65532"
pass "/data writable by 65532"
[ "$("$engine" exec "$name" id -u)" = 65532 ] || die "process uid is not 65532"
pass "process uid 65532"

internal=$(openssl rand -hex 32)
openssl rand -hex 32 >"$secrets/admin.token"
openssl rand -hex 32 >"$secrets/worker.token"
openssl rand -hex 32 >"$secrets/coord.key"
printf '%s\n' "$internal" >"$secrets/internal.token"
chown -R 65532:65532 "$secrets" 2>/dev/null || chmod 0444 "$secrets"/*
chmod 0500 "$secrets"; chmod 0400 "$secrets"/* 2>/dev/null || true
start -v "$secrets:/run/secrets:ro"
h=$(code "$base/health"); [ "$h" = 200 ] || die "/health with secrets is $h, expected 200"
pass "/health 200 with secrets"
curl -fsS -H "authorization: Bearer $internal" -H 'x-platform-challenge-slug: hypertrain' \
  "$base/internal/v1/get_weights?epoch=1" \
  | jq -e '.challenge_slug == "hypertrain" and .epoch == 1 and (.weights | type) == "object"' \
  >/dev/null || die "get_weights body"
pass "get_weights 200 with the internal token"
w=$(code -H 'authorization: Bearer wrong' -H 'x-platform-challenge-slug: hypertrain' \
  "$base/internal/v1/get_weights?epoch=1")
[ "$w" = 401 ] || die "get_weights with a wrong token is $w, expected 401"
pass "get_weights 401 with a wrong token"
echo "CANARY OK"
