#!/usr/bin/env bash
# Register challenge reverse-proxy backends with the gateway registry.
#
# The in-memory registry is empty after every gateway restart/redeploy.
# Call this on master after `docker compose up` (remote-deploy hooks it).
#
# Usage:
#   GATEWAY_URL=http://127.0.0.1:8080 ./deploy/scripts/register-challenge-backends.sh
#   ./deploy/scripts/register-challenge-backends.sh --compose
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:8080}"
RELEARN_URL="${RELEARN_BACKEND_URL:-http://relearn-challenge:8095}"
RELEARN_T2I_URL="${RELEARN_T2I_BACKEND_URL:-http://relearn-t2i-challenge:8097}"
RELEARN_MM_URL="${RELEARN_MM_BACKEND_URL:-http://relearn-mm-challenge:8098}"
BOUNTY_URL="${BOUNTY_BACKEND_URL:-http://bounty-challenge:8096}"
COMPOSE_MODE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --compose) COMPOSE_MODE=1; shift ;;
    --gateway-url) GATEWAY_URL="$2"; shift 2 ;;
    --relearn-url) RELEARN_URL="$2"; shift 2 ;;
    --relearn-t2i-url) RELEARN_T2I_URL="$2"; shift 2 ;;
    --relearn-mm-url) RELEARN_MM_URL="$2"; shift 2 ;;
    --bounty-url) BOUNTY_URL="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

resolve_admin_token() {
  if [[ -n "${BASE_GATEWAY_ADMIN_TOKEN:-}" ]]; then
    printf '%s' "${BASE_GATEWAY_ADMIN_TOKEN}"
    return 0
  fi
  local f="${BASE_GATEWAY_ADMIN_TOKEN_FILE:-}"
  if [[ -z "$f" && -f "${ROOT}/deploy/secrets/gateway_admin_token" ]]; then
    f="${ROOT}/deploy/secrets/gateway_admin_token"
  fi
  if [[ -n "$f" && -f "$f" ]]; then
    tr -d '[:space:]' <"$f"
  fi
}

register_one() {
  local challenge_id="$1" base_url="$2"
  local payload http body token
  local -a auth=()
  token="$(resolve_admin_token || true)"
  if [[ -z "${token}" ]]; then
    echo "register-challenge-backends: missing gateway admin token" >&2
    echo "  set BASE_GATEWAY_ADMIN_TOKEN or deploy/secrets/gateway_admin_token" >&2
    return 1
  fi
  auth=(-H "Authorization: Bearer ${token}")
  payload="$(printf '{"challenge_id":"%s","base_url":"%s","weight":1}' "$challenge_id" "$base_url")"
  if [[ "$COMPOSE_MODE" -eq 1 ]]; then
    body="$(docker compose -f docker-compose.yml \
      -f deploy/compose/role-master.yml \
      exec -T gateway \
      curl -sS -w '\n%{http_code}' -X POST http://127.0.0.1:8080/v1/admin/backends \
      -H 'content-type: application/json' \
      "${auth[@]}" \
      -d "$payload" 2>/dev/null || true)"
  else
    body="$(curl -sS -w '\n%{http_code}' -X POST "${GATEWAY_URL%/}/v1/admin/backends" \
      -H 'content-type: application/json' \
      "${auth[@]}" \
      -d "$payload" 2>/dev/null || true)"
  fi
  http="$(printf '%s' "$body" | tail -n1)"
  body="$(printf '%s' "$body" | sed '$d')"
  case "$http" in
    201) echo "registered $challenge_id → $base_url" ;;
    409) echo "already present $challenge_id → $base_url" ;;
    *)
      echo "failed to register $challenge_id (HTTP ${http:-?}): $body" >&2
      return 1
      ;;
  esac
}

register_one relearn "$RELEARN_URL"
register_one relearn-t2i "$RELEARN_T2I_URL"
register_one relearn-mm "$RELEARN_MM_URL"
register_one bounty "$BOUNTY_URL"

for challenge_id in relearn relearn-t2i relearn-mm bounty; do
  if [[ "$COMPOSE_MODE" -eq 1 ]]; then
    docker compose -f docker-compose.yml -f deploy/compose/role-master.yml \
      exec -T gateway curl -fsS -m 5 \
      "http://127.0.0.1:8080/challenge/${challenge_id}/health" >/dev/null
  else
    curl -fsS -m 5 "${GATEWAY_URL%/}/challenge/${challenge_id}/health" >/dev/null
  fi
done
echo "challenge proxy health: ok (relearn, relearn-t2i, relearn-mm, bounty)"
