#!/usr/bin/env bash
# Proof submit → score E2E (staging / local sim only).
#
# Never: production hosts, set_weights, master Lium rent.
# POST /v1/submissions is skipped when eval_backend=lium and can_score=true.
#
# Usage:
#   ./deploy/scripts/proof-submit-e2e.sh --http-tests
#   ./deploy/scripts/proof-submit-e2e.sh --local-sim
#   ./deploy/scripts/proof-submit-e2e.sh --probe [BASE]
#   ./deploy/scripts/proof-submit-e2e.sh --bounty [BASE]
#   ./deploy/scripts/proof-submit-e2e.sh --all
#
# Optional env:
#   PROOF_E2E_BASE     challenge origin (no trailing slash)
#   BOUNTY_E2E_BASE    bounty origin
#   PROOF_E2E_TOPIC    override topic id (default: first of dt-no-ib-v0 / muon-vs-adamw-10m-v0)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

RED() { printf '\033[31m%s\033[0m\n' "$*"; }
GRN() { printf '\033[32m%s\033[0m\n' "$*"; }
LOG() { printf '[proof-e2e] %s\n' "$*"; }

PROD_HOSTS='gateway\.cortex\.foundation|network\.cortex\.foundation|chain\.joinbase\.ai'
STAGING_TOPICS=(dt-no-ib-v0 muon-vs-adamw-10m-v0)

# url_host URL → the host part, lower-cased (no scheme, userinfo, port, path,
# query; a trailing dot dropped; IPv6 literal kept bracketed).
url_host() {
  local h="$1"
  h="${h#*://}"; h="${h%%/*}"; h="${h%%\?*}"; h="${h%%\#*}"; h="${h##*@}"
  if [[ "$h" == \[* ]]; then h="${h%%]*}]"; else h="${h%%:*}"; fi
  h="${h%.}"
  printf '%s' "$h" | tr '[:upper:]' '[:lower:]'
}
# Refuse a production origin however it is spelled: the parsed host (or any
# subdomain of a protected host) is compared lower-cased. DNS is
# case-insensitive; the guard must be too.
refuse_prod() {
  local url="${1:-}" host
  host="$(url_host "$url")"
  if printf '%s\n' "$host" | grep -Eq "^(.*\.)?(${PROD_HOSTS})$"; then
    RED "refusing production host: $url"
    exit 2
  fi
}

http_tests() {
  LOG "cargo test -p proof-http (in-process submit→score + live skip unless PROOF_E2E_BASE)"
  cargo test -p proof-http -- --nocapture
  LOG "cargo test -p ctx topic list items wrapper"
  cargo test -p ctx -- topic_list_reads_the_items_wrapper
  GRN "PASS  --http-tests"
}

local_sim() {
  LOG "cargo test -p proof-challenge-bin --test submit_e2e (force_sim binary + both staging topic ids)"
  cargo test -p proof-challenge-bin --test submit_e2e -- --nocapture
  GRN "PASS  --local-sim"
}

# Probe a running Proof origin. Prints status / topics / submit result shapes.
# Exit 0 on scored 201 or explicit 400/503. Exit 1 on silent empty / unexpected.
probe_proof() {
  local base="${1:-${PROOF_E2E_BASE:-}}"
  if [[ -z "$base" ]]; then
    for cand in \
      http://127.0.0.1:28100 \
      http://127.0.0.1:8100 \
      http://159.223.159.205/challenge/proof \
      http://159.223.159.205:8080/challenge/proof \
      http://159.223.159.205:8100 \
      http://staging.api.joinbase.ai/challenge/proof
    do
      if curl -fsS -m 3 "$cand/health" >/dev/null 2>&1; then
        base="$cand"
        break
      fi
    done
  fi
  if [[ -z "$base" ]]; then
    LOG "no reachable Proof origin (set PROOF_E2E_BASE); skip --probe"
    return 0
  fi
  base="${base%/}"
  refuse_prod "$base"
  LOG "probing $base"

  local health status topics code body
  health="$(curl -fsS -m 8 "$base/health")"
  echo "$health" | grep -q '"challenge_id":"proof"' || { RED "health is not proof: $health"; return 1; }
  LOG "GET /health → $health"

  status="$(curl -fsS -m 8 "$base/v1/status")"
  LOG "GET /v1/status → $status"
  echo "$status" | grep -q '"challenge_id":"proof"' || { RED "status missing challenge_id"; return 1; }
  if echo "$status" | grep -q 'api_key\|content_sha256'; then
    RED "status leaked a secret or holdout fingerprint"
    return 1
  fi

  topics="$(curl -fsS -m 8 "$base/v1/proof/topics")"
  LOG "GET /v1/proof/topics → $(echo "$topics" | head -c 400)…"
  echo "$topics" | grep -q 'content_sha256' && { RED "topics leaked holdout records"; return 1; }

  code="$(curl -sS -m 8 -o /tmp/proof-e2e-empty.json -w '%{http_code}' \
    -X POST -H 'content-type: application/json' \
    -d '{"miner_hotkey":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","artifact_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","claim":"probe","declared_flops":1,"topic_id":"","manifest":{"train_dataset_ids":["e2e-mix-v0"]}}' \
    "$base/v1/submissions")"
  body="$(cat /tmp/proof-e2e-empty.json)"
  LOG "POST /v1/submissions empty topic_id → HTTP $code $body"
  [[ "$code" == "400" ]] || { RED "empty topic_id expected 400, got $code"; return 1; }
  echo "$body" | grep -q '"error"' || { RED "400 had no error field"; return 1; }

  local backend can_score
  backend="$(echo "$status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("eval_backend",""))')"
  can_score="$(echo "$status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("can_score", False))')"

  if [[ "$can_score" == "True" && "$backend" == "lium" ]]; then
    LOG "skip POST: host is Lium + can_score (would rent). Fail-closed probe already passed."
    GRN "PASS  --probe $base (status+400 only; no Lium rent)"
    return 0
  fi

  local topics_to_hit=()
  if [[ -n "${PROOF_E2E_TOPIC:-}" ]]; then
    topics_to_hit=("$PROOF_E2E_TOPIC")
  else
    for id in "${STAGING_TOPICS[@]}"; do
      echo "$topics" | grep -q "\"$id\"" && topics_to_hit+=("$id")
    done
    if [[ ${#topics_to_hit[@]} -eq 0 ]]; then
      topics_to_hit=(dt-no-ib-v0)
    fi
  fi

  local topic hex sid row any_scored=0
  for topic in "${topics_to_hit[@]}"; do
    hex="$(printf '%s' "e2e-$topic-$RANDOM-$$" | sha256sum | awk '{print $1}')"
    code="$(curl -sS -m 20 -o /tmp/proof-e2e-submit.json -w '%{http_code}' \
      -X POST -H 'content-type: application/json' \
      -d "{\"miner_hotkey\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"artifact_digest\":\"$hex\",\"claim\":\"e2e sim submit against $topic\",\"declared_flops\":1,\"topic_id\":\"$topic\",\"manifest\":{\"train_dataset_ids\":[\"e2e-mix-v0\"]}}" \
      "$base/v1/submissions")"
    body="$(cat /tmp/proof-e2e-submit.json)"
    LOG "POST /v1/submissions topic_id=$topic → HTTP $code $body"
    case "$code" in
      201)
        echo "$body" | grep -q '"id":"pf_' || { RED "201 missing pf_ id"; return 1; }
        echo "$body" | grep -q "\"topic_id\":\"$topic\"" || { RED "201 topic_id mismatch"; return 1; }
        sid="$(echo "$body" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')"
        row="$(curl -fsS -m 8 "$base/v1/submissions/$sid")"
        LOG "GET /v1/submissions/$sid → $row"
        echo "$row" | grep -q '"verdict"' || { RED "scored row missing verdict"; return 1; }
        any_scored=1
        GRN "PASS  --probe $base submit→score HTTP 201 topic=$topic"
        ;;
      400|503)
        echo "$body" | grep -q '"error"' || { RED "HTTP $code silent empty"; return 1; }
        GRN "PASS  --probe $base fail-closed HTTP $code topic=$topic (explicit error)"
        ;;
      *)
        RED "unexpected HTTP $code topic=$topic (want 201/400/503, never silent empty)"
        return 1
        ;;
    esac
  done
  if [[ "$any_scored" == "1" ]]; then
    GRN "PASS  --probe $base scored ${#topics_to_hit[@]} topic(s)"
  fi
}

probe_bounty() {
  local base="${1:-${BOUNTY_E2E_BASE:-}}"
  if [[ -z "$base" ]]; then
    for cand in \
      http://127.0.0.1:28096 \
      http://127.0.0.1:8096 \
      http://159.223.159.205:8096 \
      http://159.223.159.205:8080/challenge/bounty \
      http://staging.api.joinbase.ai/challenge/bounty
    do
      if curl -fsS -m 3 "$cand/health" >/dev/null 2>&1; then
        base="$cand"
        break
      fi
    done
  fi
  if [[ -z "$base" ]]; then
    LOG "no reachable Bounty origin (set BOUNTY_E2E_BASE); skip --bounty"
    return 0
  fi
  base="${base%/}"
  refuse_prod "$base"
  LOG "probing bounty $base"
  local status
  status="$(curl -fsS -m 8 "$base/v1/status")"
  LOG "GET /v1/status → $status"
  echo "$status" | grep -q '"challenge_id":"bounty"' || { RED "not bounty"; return 1; }

  local code
  code="$(curl -sS -m 8 -o /tmp/bounty-e2e-report.json -w '%{http_code}' \
    -X POST -H 'content-type: application/json' \
    -d '{"session":"not-a-session","title":"e2e","body":"e2e","repro_steps":"e2e"}' \
    "$base/v1/reports")"
  LOG "POST /v1/reports (thin) → HTTP $code $(cat /tmp/bounty-e2e-report.json)"
  if echo "$status" | grep -q '"scoring_backend":"unconfigured"'; then
    [[ "$code" == "503" ]] || { RED "unconfigured bounty must 503, got $code"; return 1; }
    GRN "PASS  --bounty $base fail-closed 503"
  else
    # Feed configured: do not file a real report. Session gate is enough.
    [[ "$code" == "401" || "$code" == "400" || "$code" == "503" ]] \
      || { RED "configured bounty unexpected $code"; return 1; }
    GRN "PASS  --bounty $base ingest reached an explicit gate HTTP $code (no prod write)"
  fi
}

usage() {
  sed -n '2,20p' "$0"
}

DO_HTTP=0
DO_LOCAL=0
DO_PROBE=0
DO_BOUNTY=0
PROBE_BASE=""
BOUNTY_BASE=""

if [[ $# -eq 0 ]]; then
  usage
  exit 1
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --http-tests) DO_HTTP=1; shift ;;
    --local-sim) DO_LOCAL=1; shift ;;
    --probe)
      DO_PROBE=1
      if [[ "${2:-}" != --* && -n "${2:-}" ]]; then PROBE_BASE="$2"; shift; fi
      shift
      ;;
    --bounty)
      DO_BOUNTY=1
      if [[ "${2:-}" != --* && -n "${2:-}" ]]; then BOUNTY_BASE="$2"; shift; fi
      shift
      ;;
    --all) DO_HTTP=1; DO_LOCAL=1; DO_PROBE=1; DO_BOUNTY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) RED "unknown arg: $1"; usage; exit 1 ;;
  esac
done

[[ "$DO_HTTP" -eq 1 ]] && http_tests
[[ "$DO_LOCAL" -eq 1 ]] && local_sim
[[ "$DO_PROBE" -eq 1 ]] && probe_proof "$PROBE_BASE"
[[ "$DO_BOUNTY" -eq 1 ]] && probe_bounty "$BOUNTY_BASE"
GRN "proof-submit-e2e done"
