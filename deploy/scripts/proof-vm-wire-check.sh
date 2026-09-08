#!/usr/bin/env bash
# Proof topic-VM orchestrator — staging wire check + end-to-end probes.
#
# Runs on the staging MASTER droplet (/opt/base) or any box that reaches the
# Proof control plane and the KVM-host agent. bash + curl + python3 only, no
# cargo. Never prints the bearer (it travels in a 0600 curl config, never in
# argv). Refuses production hosts. Runbook:
# docs/runbooks/proof-vm-orchestrator.md § DigitalOcean staging.
#
# Usage:
#   proof-vm-wire-check.sh env          # CP env file: https URL, non-empty bearer file, sha256 pin, CA, ids
#   proof-vm-wire-check.sh agent        # KVM-host agent: health with the bearer; no / wrong bearer → 401
#   proof-vm-wire-check.sh cp           # control plane: /v1/status gates, no leaks, admin vm-orchestrator probe
#   proof-vm-wire-check.sh all          # env + agent + cp
#   proof-vm-wire-check.sh boot-probe   # create → attach → destroy ONE RLM VM on the KVM host (no job, no spend)
#   proof-vm-wire-check.sh submit-probe --topic ID --expect 503 [--reason SUBSTR]
#                                       # POST /v1/submissions on a custom topic; assert the fail-closed answer
#   proof-vm-wire-check.sh submit-probe --topic ID --expect 201 --allow-live-run --artifact-uri URI
#                                       # happy path: real RLM job + sister guest; prints flops_used from the row
#   proof-vm-wire-check.sh matrix       # print the fail-closed matrix as operator steps + expected 503 reasons
#
# Options (all subcommands):
#   --env-file F          CP env (default deploy/env/proof-challenge.env; process env wins when set)
#   --path-map FROM=TO    container → host path (default /run/base/proof=deploy/secrets/proof; repeatable)
#   --cp URL              Proof origin (default: first reachable of the loopback candidates)
#   --admin-token-file F  operator bearer for /v1/admin/* (default deploy/secrets/proof/admin_tokens, first line)
#   --probe-topic ID      boot-probe topic id (default wire-probe-<epoch>; slug, never a real topic)
#   --topic ID            submit-probe topic id (must be an open custom topic)
#   --expect CODE         submit-probe expected HTTP status (400 / 503; 2xx needs --allow-live-run)
#   --reason SUBSTR       submit-probe: the error text must contain this
#   --artifact-uri URI    submit-probe locator (default https://example.invalid/wire-probe.tar — never fetchable)
#   --no-artifact-uri     submit-probe without a locator (a custom topic must answer 400, no row)
#   --declared-flops N    submit-probe declaration (default: 1 for fail-closed probes; the topic's
#                         flops_budget for --expect 2xx so a real run is not rejected flops_under_declared)
#   --wait SECS           submit-probe --expect 201: how long the synchronous POST may take (default 900)
#
# Exit: 0 all PASS, 1 any FAIL, 2 refused (production host / unsafe request).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

RED() { printf '\033[31m%s\033[0m\n' "$*"; }
GRN() { printf '\033[32m%s\033[0m\n' "$*"; }
YEL() { printf '\033[33m%s\033[0m\n' "$*"; }
LOG() { printf '[wire-check] %s\n' "$*"; }

FAILS=0
WARNS=0
pass() { GRN "PASS  $*"; }
warn() { YEL "WARN  $*"; WARNS=$((WARNS + 1)); }
fail() { RED "FAIL  $*"; FAILS=$((FAILS + 1)); }

PROD_HOSTS='network\.cortex\.foundation|chain\.joinbase\.ai|api\.cortex\.foundation'
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
# subdomain of a protected host) is compared lower-cased, and the whole URL is
# also matched case-insensitively as belt and braces. DNS is case-insensitive;
# the guard must be too.
refuse_prod() {
  local url="$1" host
  host="$(url_host "$url")"
  if printf '%s\n' "$host" | grep -Eq "^(.*\.)?(${PROD_HOSTS})$" \
     || printf '%s' "$url" | grep -Eiq "$PROD_HOSTS"; then
    RED "refusing production host: $url"
    exit 2
  fi
}

# ---------------------------------------------------------------------------
# arguments
# ---------------------------------------------------------------------------
ENV_FILE="deploy/env/proof-challenge.env"
PATH_MAPS=()
CP=""
ADMIN_TOKEN_FILE="deploy/secrets/proof/admin_tokens"
PROBE_TOPIC=""
TOPIC=""
EXPECT=""
REASON=""
ALLOW_LIVE_RUN=0
ARTIFACT_URI="https://example.invalid/wire-probe.tar"
WAIT_SECS=900
DECLARED_FLOPS=""

usage() { sed -n '2,37p' "$0"; }

[[ $# -ge 1 ]] || { usage; exit 1; }
SUBCOMMAND="$1"; shift
while [[ $# -gt 0 ]]; do
  case "$1" in
    --env-file) ENV_FILE="${2:?}"; shift 2 ;;
    --path-map) PATH_MAPS+=("${2:?}"); shift 2 ;;
    --cp) CP="${2:?}"; shift 2 ;;
    --admin-token-file) ADMIN_TOKEN_FILE="${2:?}"; shift 2 ;;
    --probe-topic) PROBE_TOPIC="${2:?}"; shift 2 ;;
    --topic) TOPIC="${2:?}"; shift 2 ;;
    --expect) EXPECT="${2:?}"; shift 2 ;;
    --reason) REASON="${2:?}"; shift 2 ;;
    --allow-live-run) ALLOW_LIVE_RUN=1; shift ;;
    --artifact-uri) ARTIFACT_URI="${2:?}"; shift 2 ;;
    --no-artifact-uri) ARTIFACT_URI=""; shift ;;
    --wait) WAIT_SECS="${2:?}"; shift 2 ;;
    --declared-flops) DECLARED_FLOPS="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) RED "unknown arg: $1"; usage; exit 1 ;;
  esac
done
[[ ${#PATH_MAPS[@]} -gt 0 ]] || PATH_MAPS=("/run/base/proof=$ROOT/deploy/secrets/proof")

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------
# jget JSON dotted.path → raw scalar, compact JSON for objects/arrays, "" when absent.
jget() {
  printf '%s' "$1" | python3 -c '
import json, sys
path = sys.argv[1]
try:
    v = json.load(sys.stdin)
except Exception:
    print(""); sys.exit(0)
for k in [p for p in path.split(".") if p]:
    if isinstance(v, list):
        try: v = v[int(k)]
        except Exception: v = None
    elif isinstance(v, dict):
        v = v.get(k)
    else:
        v = None
    if v is None: break
if v is None: print("")
elif isinstance(v, bool): print("true" if v else "false")
elif isinstance(v, (dict, list)): print(json.dumps(v, separators=(",", ":")))
else: print(v)
' "$2"
}

# Value from the process env when set, else from the env file (last KEY= wins;
# commented lines ignored; surrounding quotes stripped). Empty = unset.
cfg() {
  local name="$1" val="${!1:-}" line
  if [[ -z "$val" && -f "$ENV_FILE" ]]; then
    line="$(grep -E "^[[:space:]]*(export[[:space:]]+)?${name}=" "$ENV_FILE" | tail -n1 || true)"
    val="${line#*=}"
    val="${val%\"}"; val="${val#\"}"; val="${val%\'}"; val="${val#\'}"
  fi
  printf '%s' "$val"
}

# Container path → host path through --path-map (compose bind mounts).
map_path() {
  local p="$1" m from to
  for m in "${PATH_MAPS[@]}"; do
    from="${m%%=*}"; to="${m#*=}"
    if [[ "$p" == "$from" || "$p" == "$from/"* ]]; then
      printf '%s' "${to}${p#"$from"}"
      return 0
    fi
  done
  printf '%s' "$p"
}

TMPDIR_WC="$(mktemp -d)"
umask 077

# A boot-probe VM must never outlive the probe: not on Ctrl-C, and not when
# the agent committed a create whose answer never reached us. PROBE_TOPIC_LIVE
# is set BEFORE the create goes out, so the exit path can always reconcile by
# topic (GET /v1/vms/by-topic) even without a vm id; PROBE_VM is the id once
# an answer named it. Both are cleared only after a confirmed destroy + 404.
PROBE_BASE=""; PROBE_VM=""; PROBE_TOPIC_LIVE=""; PROBE_HDR=""
AGENT_ARGS=()
on_exit() {
  if [[ -n "$PROBE_TOPIC_LIVE" ]]; then
    RED "boot-probe left topic $PROBE_TOPIC_LIVE in flight; reconciling on the agent"
    probe_reconcile_destroy || true
  fi
  rm -rf "$TMPDIR_WC"
}
trap on_exit EXIT

# curl config carrying the bearer (mode 0600; never in argv, never printed).
bearer_config() { # bearer_config FILE_WITH_TOKEN → path of a curl -K config
  local tok cfg_path="$TMPDIR_WC/hdr.$RANDOM$RANDOM"
  tok="$(tr -d '[:space:]' < "$1")"
  printf 'header = "Authorization: Bearer %s"\n' "$tok" > "$cfg_path"
  printf '%s' "$cfg_path"
}

# http METHOD URL BODY_JSON [CURL_ARGS...] → $HTTP_CODE ("000" = no answer) and $HTTP_BODY.
# Default timeout 30s; a later `-m N` in CURL_ARGS wins. Never fails the script.
HTTP_CODE=""
HTTP_BODY=""
http() {
  local method="$1" url="$2" body="$3" out="$TMPDIR_WC/body.$RANDOM$RANDOM"
  shift 3
  local -a data=()
  [[ -n "$body" ]] && data=(-H 'content-type: application/json' --data-binary "$body")
  HTTP_CODE="$(curl -sS -m 30 -o "$out" -w '%{http_code}' -X "$method" "${data[@]}" "$@" "$url" 2>"$TMPDIR_WC/err" || true)"
  HTTP_BODY="$(cat "$out" 2>/dev/null || true)"
  if [[ -z "$HTTP_CODE" || "$HTTP_CODE" == "000" ]]; then
    HTTP_BODY="$(cat "$TMPDIR_WC/err" 2>/dev/null || true)"
    HTTP_CODE="000"
  fi
  rm -f "$out"
  # Test hook (crates/proof-vm-fc/tests/wire_check_script.rs): the agent
  # committed the create but its answer never arrived. Never set by operators.
  if [[ "${PROOF_VM_WIRE_CHECK_FAULT:-}" == "lose-create-answer" && "$method" == "POST" && "$url" == */v1/vms ]]; then
    HTTP_CODE="000"; HTTP_BODY="(simulated: answer to POST /v1/vms lost)"
  fi
}

# The VM the agent holds for the probe topic, if any → $PROBE_VM ("" when none).
probe_attach() {
  http GET "$PROBE_BASE/v1/vms/by-topic/$PROBE_TOPIC_LIVE" "" "${AGENT_ARGS[@]}" -K "$PROBE_HDR"
  if [[ "$HTTP_CODE" == "200" ]]; then
    PROBE_VM="$(jget "$HTTP_BODY" handle.vm_id)"
  elif [[ "$HTTP_CODE" == "404" ]]; then
    PROBE_VM=""
  fi
  return 0
}

# Destroy whatever VM the probe topic holds — the id we were told, or the one
# the agent reports for the topic when the create's answer was lost. Clears
# PROBE_VM / PROBE_TOPIC_LIVE only on a confirmed destroy followed by a 404.
# Returns 0 when the topic is verifiably free, 1 otherwise.
probe_reconcile_destroy() {
  [[ -n "$PROBE_TOPIC_LIVE" ]] || return 0
  if [[ -z "$PROBE_VM" ]]; then
    probe_attach
    if [[ "$HTTP_CODE" == "404" ]]; then
      LOG "reconcile: agent holds no vm for $PROBE_TOPIC_LIVE"
      PROBE_TOPIC_LIVE=""
      return 0
    fi
    if [[ -z "$PROBE_VM" ]]; then
      RED "reconcile: attach for $PROBE_TOPIC_LIVE → HTTP $HTTP_CODE $(printf '%s' "$HTTP_BODY" | head -c 200); check the agent by hand"
      return 1
    fi
    LOG "reconcile: agent reports vm $PROBE_VM for $PROBE_TOPIC_LIVE"
  fi
  local body
  body="$(printf '{"topic_id":"%s","policy":"destroy"}' "$PROBE_TOPIC_LIVE")"
  http DELETE "$PROBE_BASE/v1/vms/$PROBE_VM" "$body" "${AGENT_ARGS[@]}" -K "$PROBE_HDR" -m 660
  if [[ "$HTTP_CODE" == "200" && "$(jget "$HTTP_BODY" state)" == "destroyed" && "$(jget "$HTTP_BODY" confirmed)" == "true" ]] \
     || [[ "$HTTP_CODE" == "404" ]]; then
    local destroyed="$PROBE_VM"
    PROBE_VM=""
    probe_attach
    if [[ "$HTTP_CODE" == "404" ]]; then
      LOG "reconcile: vm $destroyed destroyed; topic $PROBE_TOPIC_LIVE free"
      PROBE_TOPIC_LIVE=""
      return 0
    fi
  fi
  RED "reconcile: vm $PROBE_VM for $PROBE_TOPIC_LIVE not confirmed destroyed (HTTP $HTTP_CODE $(printf '%s' "$HTTP_BODY" | head -c 200)); on the agent host: journalctl -u proof-vm-orchestrator, ls /srv/jailer/firecracker/"
  return 1
}

# ---------------------------------------------------------------------------
# env: the control plane's side of the wire
# ---------------------------------------------------------------------------
URL=""; TOKEN_PATH=""; CA_PATH=""; DIGEST=""; IDS=""
load_env() {
  URL="$(cfg PROOF_VM_ORCHESTRATOR_URL)"
  TOKEN_PATH="$(cfg PROOF_VM_ORCHESTRATOR_TOKEN_FILE)"
  CA_PATH="$(cfg PROOF_VM_ORCHESTRATOR_CA_FILE)"
  DIGEST="$(cfg PROOF_RLM_VM_IMAGE_DIGEST)"
  IDS="$(cfg PROOF_VM_RUNNER_CUSTOM_IDS)"
  [[ -n "$TOKEN_PATH" ]] && TOKEN_PATH="$(map_path "$TOKEN_PATH")"
  [[ -n "$CA_PATH" ]] && CA_PATH="$(map_path "$CA_PATH")"
  AGENT_ARGS=()
  [[ -n "$CA_PATH" && -f "$CA_PATH" ]] && AGENT_ARGS+=(--cacert "$CA_PATH")
  # Before any other check or request: a production agent is never probed.
  [[ -n "$URL" ]] && refuse_prod "$URL"
  return 0
}

split_ids() { # split_ids "a, b,c" → one id per line, trimmed, blanks dropped
  printf '%s\n' "$1" | tr ',' '\n' | sed -e 's/[[:space:]]//g' -e '/^$/d'
}

check_env() {
  LOG "env: $ENV_FILE (process env overrides; container paths mapped: ${PATH_MAPS[*]})"
  load_env
  if [[ -z "$URL" ]]; then
    fail "PROOF_VM_ORCHESTRATOR_URL unset → proof-challenge keeps UnwiredVmOrchestrator (every custom topic 503)"
  else
    refuse_prod "$URL"
    case "$URL" in
      https://*) pass "PROOF_VM_ORCHESTRATOR_URL is https ($URL)" ;;
      http://127.0.0.1*|http://localhost*|http://\[::1\]*) warn "PROOF_VM_ORCHESTRATOR_URL is plain http on loopback (tests / local TLS terminator only)" ;;
      http://*) fail "PROOF_VM_ORCHESTRATOR_URL is plain http off loopback → refused at boot, host stays unwired" ;;
      *) fail "PROOF_VM_ORCHESTRATOR_URL is not a URL: $URL" ;;
    esac
  fi

  local raw_token
  raw_token="$(cfg PROOF_VM_ORCHESTRATOR_TOKEN_FILE)"
  if [[ -z "$raw_token" ]]; then
    fail "PROOF_VM_ORCHESTRATOR_TOKEN_FILE unset (the bearer is a FILE, never a value) → boot refuses, host stays unwired"
  elif [[ ! -f "$TOKEN_PATH" ]]; then
    fail "bearer file $raw_token → $TOKEN_PATH missing on this host → ready() 503 naming PROOF_VM_ORCHESTRATOR_TOKEN_FILE"
  elif [[ -z "$(tr -d '[:space:]' < "$TOKEN_PATH")" ]]; then
    fail "bearer file $TOKEN_PATH is empty → ready() 503 naming PROOF_VM_ORCHESTRATOR_TOKEN_FILE"
  else
    pass "bearer file present and non-empty ($raw_token → $TOKEN_PATH; contents not shown)"
    local mode owner
    mode="$(stat -c '%a' "$TOKEN_PATH" 2>/dev/null || echo '?')"
    owner="$(stat -c '%u' "$TOKEN_PATH" 2>/dev/null || echo '?')"
    [[ "$mode" == "400" || "$mode" == "600" ]] || warn "bearer file mode is $mode (want 0400)"
    [[ "$owner" == "65532" || "$owner" == "?" ]] || warn "bearer file owner uid $owner (proof-challenge reads it as uid 65532)"
  fi

  if [[ -z "$DIGEST" ]]; then
    fail "PROOF_RLM_VM_IMAGE_DIGEST unset → unpinned → ready() 503; nothing ever boots. Take sha256sum of the RLM rootfs staged on the KVM host; DO NOT INVENT ONE"
  elif [[ "$DIGEST" =~ ^sha256:[0-9a-fA-F]{64}$ ]]; then
    pass "PROOF_RLM_VM_IMAGE_DIGEST is a sha256 pin (${DIGEST:0:19}…); the agent needs images/sha256-<hex>.ext4"
  else
    fail "PROOF_RLM_VM_IMAGE_DIGEST is not sha256:<64 hex>: $DIGEST"
  fi

  local raw_ca
  raw_ca="$(cfg PROOF_VM_ORCHESTRATOR_CA_FILE)"
  if [[ -z "$raw_ca" ]]; then
    LOG "PROOF_VM_ORCHESTRATOR_CA_FILE unset: the agent certificate must chain to a public root"
  elif [[ ! -f "$CA_PATH" ]]; then
    fail "CA file $raw_ca → $CA_PATH missing (boot refuses the client, host stays unwired)"
  elif ! grep -q 'BEGIN CERTIFICATE' "$CA_PATH"; then
    fail "CA file $CA_PATH is not PEM"
  else
    pass "CA file present ($raw_ca → $CA_PATH). rustls needs a SAN on the agent cert (CN alone is refused)"
  fi

  if [[ -z "$IDS" ]]; then
    warn "PROOF_VM_RUNNER_CUSTOM_IDS unset → empty registry → every custom topic 503 (registration is an operator action)"
  else
    local id bad=0
    while IFS= read -r id; do
      [[ "$id" =~ ^[a-z0-9][a-z0-9_-]{1,63}$ ]] || { fail "custom id '$id' is malformed (skipped at boot): want [a-z0-9][a-z0-9_-]{1,63}"; bad=1; }
    done < <(split_ids "$IDS")
    [[ "$bad" -eq 0 ]] && pass "PROOF_VM_RUNNER_CUSTOM_IDS well-formed: $IDS"
  fi

  local vcpus mem
  vcpus="$(cfg PROOF_RLM_VM_VCPUS)"; mem="$(cfg PROOF_RLM_VM_MEM_MIB)"
  [[ -z "$vcpus" || "$vcpus" == "4" ]] || warn "PROOF_RLM_VM_VCPUS=$vcpus deviates from the locked 4"
  [[ -z "$mem" || "$mem" == "8192" ]] || warn "PROOF_RLM_VM_MEM_MIB=$mem deviates from the locked 8192"
  if [[ "$(cfg PROOF_FORCE_SIM)" =~ ^(1|true|TRUE|yes)$ ]]; then
    fail "PROOF_FORCE_SIM is on: sim never hosts staging/prod scoring (assert-compose-matrix.sh refuses it too)"
  fi
}

# ---------------------------------------------------------------------------
# agent: the KVM-host side, through curl (the CP-side rustls path is `cp`)
# ---------------------------------------------------------------------------
check_agent() {
  load_env
  if [[ -z "$URL" || -z "$TOKEN_PATH" || ! -s "$TOKEN_PATH" ]]; then
    fail "agent probe needs PROOF_VM_ORCHESTRATOR_URL and a non-empty bearer file (run: $0 env)"
    return 0
  fi
  refuse_prod "$URL"
  local base="${URL%/}" code hdr
  hdr="$(bearer_config "$TOKEN_PATH")"
  LOG "agent: GET $base/v1/health (bearer from $TOKEN_PATH${CA_PATH:+, --cacert $CA_PATH})"

  http GET "$base/v1/health" "" "${AGENT_ARGS[@]}" -K "$hdr"; code="$HTTP_CODE"
  if [[ "$code" != "200" ]]; then
    fail "agent health → HTTP $code: $(printf '%s' "$HTTP_BODY" | head -c 300)"
  else
    local api ready reason hv vms
    api="$(jget "$HTTP_BODY" api_version)"; ready="$(jget "$HTTP_BODY" ready)"
    reason="$(jget "$HTTP_BODY" reason)"; hv="$(jget "$HTTP_BODY" hypervisor)"; vms="$(jget "$HTTP_BODY" vms)"
    [[ "$api" == "1" ]] || fail "agent api_version '$api' (this tree speaks 1)"
    if [[ "$ready" == "true" ]]; then
      pass "agent ready (hypervisor=$hv vms=$vms)"
    else
      fail "agent not ready: $reason (firecracker + jailer + /dev/kvm + images on the KVM host)"
    fi
    [[ "$hv" == "firecracker" ]] || warn "agent hypervisor is '$hv', not firecracker (fake is CI-only)"
  fi

  http GET "$base/v1/health" "" "${AGENT_ARGS[@]}"; code="$HTTP_CODE"
  if [[ "$code" == "401" ]]; then pass "no bearer → 401 (agent fail-closed)"; else fail "no bearer → HTTP $code (want 401)"; fi
  http GET "$base/v1/health" "" "${AGENT_ARGS[@]}" -H 'Authorization: Bearer wire-check-wrong-bearer-not-a-secret'; code="$HTTP_CODE"
  if [[ "$code" == "401" ]]; then pass "wrong bearer → 401 (agent fail-closed)"; else fail "wrong bearer → HTTP $code (want 401)"; fi
  LOG "curl accepting the certificate is not proof the Rust client does (rustls wants a SAN): '$0 cp' runs the CP-side probe"
}

# ---------------------------------------------------------------------------
# cp: the control plane's view — status gates, leaks, admin vm-orchestrator probe
# ---------------------------------------------------------------------------
resolve_cp() {
  if [[ -z "$CP" ]]; then
    local cand
    for cand in http://127.0.0.1:8080/challenge/proof http://127.0.0.1:28100 http://127.0.0.1:8100; do
      if curl -fsS -m 3 "$cand/health" >/dev/null 2>&1; then CP="$cand"; break; fi
    done
  fi
  [[ -n "$CP" ]] || { fail "no reachable Proof origin (pass --cp URL)"; return 1; }
  CP="${CP%/}"
  refuse_prod "$CP"
}

admin_config() { # → curl -K path, or "" when no operator bearer is available
  if [[ -n "${PROOF_ADMIN_TOKEN:-}" ]]; then
    printf '%s' "$PROOF_ADMIN_TOKEN" > "$TMPDIR_WC/admin_tok"
    bearer_config "$TMPDIR_WC/admin_tok"
  elif [[ -s "$ADMIN_TOKEN_FILE" ]]; then
    grep -m1 -v '^[[:space:]]*$' "$ADMIN_TOKEN_FILE" > "$TMPDIR_WC/admin_tok"
    bearer_config "$TMPDIR_WC/admin_tok"
  fi
}

check_cp() {
  load_env
  resolve_cp || return 0
  LOG "cp: $CP"
  local code
  http GET "$CP/health" ""; code="$HTTP_CODE"
  if [[ "$code" == "200" && "$(jget "$HTTP_BODY" challenge_id)" == "proof" ]]; then
    pass "GET /health is proof"
  else
    fail "GET /health → $code $HTTP_BODY"
  fi

  http GET "$CP/v1/status" ""; code="$HTTP_CODE"
  if [[ "$code" != "200" ]]; then
    fail "GET /v1/status → $code"
    return 0
  fi
  local status="$HTTP_BODY" harvest can_score registered
  harvest="$(jget "$status" live_harvest_wired)"; can_score="$(jget "$status" can_score)"
  registered="$(jget "$status" registered_custom)"
  LOG "status: eval_backend=$(jget "$status" eval_backend) live_harvest_wired=$harvest can_score=$can_score baseline_sealed=$(jget "$status" baseline_sealed)"
  LOG "status: open_topics=$(jget "$status" open_topics) scorable_topics=$(jget "$status" scorable_topics) registered_custom=$registered"
  [[ "$(jget "$status" eval_backend)" == "lium" ]] || fail "eval_backend is not lium (sim never hosts staging scoring)"
  if [[ "$harvest" == "true" ]]; then
    pass "live_harvest_wired: the custom family is routed (LIUM_API_KEY + LIUM_SSH_PUBLIC_KEY_FILE present)"
  else
    fail "live_harvest_wired=false: the custom family is not routed and registered_custom stays [] whatever PROOF_VM_RUNNER_CUSTOM_IDS says"
  fi
  local leak leaked=0
  for leak in "vm_orchestrator" "/run/base" "Bearer " "PROOF_VM_ORCHESTRATOR"; do
    if printf '%s' "$status" | grep -qF "$leak"; then fail "/v1/status leaks '$leak'"; leaked=1; fi
  done
  if [[ -n "$URL" ]]; then
    local host
    host="$(url_host "$URL")"
    if printf '%s' "$status" | grep -qiF "$host"; then fail "/v1/status leaks the agent host $host"; leaked=1; fi
  fi
  [[ "$leaked" -eq 0 ]] && pass "/v1/status carries no orchestrator URL, token, or path"
  if [[ -n "$IDS" ]]; then
    local id missing=0
    while IFS= read -r id; do
      printf '%s' "$registered" | grep -qF "\"$id\"" || { fail "registered_custom lacks $id (PROOF_VM_RUNNER_CUSTOM_IDS says it is served)"; missing=1; }
    done < <(split_ids "$IDS")
    [[ "$missing" -eq 0 ]] && pass "registered_custom lists every id in PROOF_VM_RUNNER_CUSTOM_IDS"
  fi

  http GET "$CP/v1/proof/topics" ""; code="$HTTP_CODE"
  if [[ "$code" != "200" ]]; then
    fail "GET /v1/proof/topics → $code"
  elif printf '%s' "$HTTP_BODY" | grep -q 'content_sha256'; then
    fail "/v1/proof/topics leaks holdout records"
  else
    pass "/v1/proof/topics leaks no holdout"
  fi
  http GET "$CP/v1/proof/executor" ""; code="$HTTP_CODE"
  if [[ "$code" == "200" ]]; then
    LOG "executor: ready=$(jget "$HTTP_BODY" ready) reason='$(jget "$HTTP_BODY" reason)'"
  else
    fail "GET /v1/proof/executor → $code"
  fi

  local hdr
  hdr="$(admin_config)"
  if [[ -z "$hdr" ]]; then
    warn "no operator bearer (PROOF_ADMIN_TOKEN or --admin-token-file): skipping GET /v1/admin/proof/vm-orchestrator"
    return 0
  fi
  LOG "admin probe: GET $CP/v1/admin/proof/vm-orchestrator (the CP's own client: bearer file, CA, rustls)"
  http GET "$CP/v1/admin/proof/vm-orchestrator" "" -K "$hdr" -m 40; code="$HTTP_CODE"
  if [[ "$code" != "200" ]]; then
    fail "admin probe → HTTP $code $(printf '%s' "$HTTP_BODY" | head -c 200) (401 = wrong operator bearer; 503 auth_unconfigured = no PROOF_ADMIN_TOKENS_FILE; 404 = proof-challenge predates this route)"
    return 0
  fi
  local rep="$HTTP_BODY" orch ready reason a_ready a_reason a_hv
  orch="$(jget "$rep" orchestrator)"; ready="$(jget "$rep" ready)"; reason="$(jget "$rep" reason)"
  a_ready="$(jget "$rep" agent.ready)"; a_reason="$(jget "$rep" agent.reason)"; a_hv="$(jget "$rep" agent.hypervisor)"
  LOG "admin probe: orchestrator=$orch ready=$ready image=$(jget "$rep" image_digest | head -c 19)… shape=$(jget "$rep" vcpus)vCPU/$(jget "$rep" mem_mib)MiB agent=$(jget "$rep" agent) agent_error=$(jget "$rep" agent_error)"
  if [[ "$orch" == "firecracker" ]]; then pass "CP resolved FirecrackerOrchestrator"; else fail "CP orchestrator is '$orch': $reason"; fi
  if [[ "$ready" == "true" ]]; then pass "CP ready(): bearer file + RLM image pin in place"; else fail "CP not ready: $reason"; fi
  if [[ -n "$(jget "$rep" agent)" ]]; then
    if [[ "$a_ready" == "true" ]]; then pass "agent answered the CP's client: ready (hypervisor=$a_hv)"; else fail "agent answered but not ready: $a_reason"; fi
    [[ "$a_hv" == "firecracker" ]] || warn "agent hypervisor '$a_hv' is not firecracker"
  else
    fail "agent did not answer the CP's client: $(jget "$rep" agent_error)"
  fi
  if printf '%s' "$rep" | grep -qiE 'authorization|bearer [a-z0-9]'; then fail "admin probe body carries a bearer"; fi
}

# ---------------------------------------------------------------------------
# boot-probe: create → attach → destroy one RLM VM (no job, no spend)
# ---------------------------------------------------------------------------
boot_probe() {
  load_env
  if [[ -z "$URL" || ! -s "$TOKEN_PATH" || ! "$DIGEST" =~ ^sha256:[0-9a-fA-F]{64}$ ]]; then
    fail "boot-probe needs URL + non-empty bearer file + sha256 pin (run: $0 env)"
    return 0
  fi
  refuse_prod "$URL"
  local base="${URL%/}" topic="${PROBE_TOPIC:-wire-probe-$(date +%s)}" hdr code vm_id
  [[ "$topic" =~ ^[a-z0-9][a-z0-9-]{1,62}$ ]] || { fail "probe topic '$topic' is not a slug"; return 0; }
  local vcpus mem
  vcpus="$(cfg PROOF_RLM_VM_VCPUS)"; mem="$(cfg PROOF_RLM_VM_MEM_MIB)"
  vcpus="${vcpus:-4}"; mem="${mem:-8192}"
  hdr="$(bearer_config "$TOKEN_PATH")"
  LOG "boot-probe: POST $base/v1/vms topic=$topic image=${DIGEST:0:19}… ${vcpus}vCPU/${mem}MiB (boots a real RLM VM; up to 10 min)"

  http GET "$base/v1/vms/by-topic/$topic" "" "${AGENT_ARGS[@]}" -K "$hdr"; code="$HTTP_CODE"
  [[ "$code" == "404" ]] || { fail "topic $topic already has a VM or attach failed (HTTP $code): $HTTP_BODY"; return 0; }

  local spec
  spec="$(printf '{"spec":{"topic_id":"%s","template":{"image_digest":"%s","vcpus":%s,"mem_mib":%s},"sandbox":{"firecracker_required":true,"deadline_s":60},"retain":"destroy"}}' \
    "$topic" "$DIGEST" "$vcpus" "$mem")"
  # From here the topic is in flight: whatever the create's answer, the exit
  # path reconciles by topic and destroys what the agent holds for it.
  PROBE_BASE="$base"; PROBE_HDR="$hdr"; PROBE_TOPIC_LIVE="$topic"; PROBE_VM=""
  http POST "$base/v1/vms" "$spec" "${AGENT_ARGS[@]}" -K "$hdr" -m 660; code="$HTTP_CODE"
  vm_id=""
  [[ "$code" == "201" ]] && vm_id="$(jget "$HTTP_BODY" handle.vm_id)"
  if [[ -z "$vm_id" ]]; then
    # 000 (timeout / connection lost), 5xx, or a 201 we could not parse: the
    # agent may have committed the VM. Ask it by topic and destroy any hit.
    fail "create → HTTP $code: $(printf '%s' "$HTTP_BODY" | head -c 400) (503 not_ready = image/kernel/kvm on the host; 400 bad_spec; 401 bearer; 000 = answer lost)"
    if probe_reconcile_destroy; then
      LOG "reconciled: the agent holds nothing for $topic"
    else
      fail "reconcile after the ambiguous create did not free $topic"
    fi
    return 0
  fi
  PROBE_VM="$vm_id"
  if [[ "$(jget "$HTTP_BODY" handle.topic_id)" == "$topic" ]]; then pass "create bound vm $vm_id to $topic"; else fail "create bound another topic: $HTTP_BODY"; fi
  if [[ "$(jget "$HTTP_BODY" image_digest | tr '[:upper:]' '[:lower:]')" == "$(printf '%s' "$DIGEST" | tr '[:upper:]' '[:lower:]')" ]]; then
    pass "agent booted the pinned image"
  else
    fail "agent booted $(jget "$HTTP_BODY" image_digest), pinned $DIGEST"
  fi
  [[ "$(jget "$HTTP_BODY" state)" == "running" ]] || fail "state after create: $(jget "$HTTP_BODY" state)"

  http GET "$base/v1/vms/by-topic/$topic" "" "${AGENT_ARGS[@]}" -K "$hdr"; code="$HTTP_CODE"
  if [[ "$code" == "200" && "$(jget "$HTTP_BODY" handle.vm_id)" == "$vm_id" ]]; then
    pass "attach returns the same vm (one topic ↔ one VM)"
  else
    fail "attach → HTTP $code $HTTP_BODY"
  fi
  http POST "$base/v1/vms" "$spec" "${AGENT_ARGS[@]}" -K "$hdr"; code="$HTTP_CODE"
  if [[ "$code" == "409" ]]; then pass "second create for the topic → 409 already_exists"; else fail "second create → HTTP $code (want 409): $HTTP_BODY"; fi
  local wrong
  wrong="$(printf '{"topic_id":"%s-other","policy":"destroy"}' "$topic")"
  http DELETE "$base/v1/vms/$vm_id" "$wrong" "${AGENT_ARGS[@]}" -K "$hdr"; code="$HTTP_CODE"
  if [[ "$code" == "409" ]]; then pass "teardown naming another topic → 409 topic_mismatch"; else fail "teardown for another topic → HTTP $code (want 409): $HTTP_BODY"; fi

  local body
  body="$(printf '{"topic_id":"%s","policy":"destroy"}' "$topic")"
  http DELETE "$base/v1/vms/$vm_id" "$body" "${AGENT_ARGS[@]}" -K "$hdr" -m 660; code="$HTTP_CODE"
  if [[ "$code" == "200" && "$(jget "$HTTP_BODY" state)" == "destroyed" && "$(jget "$HTTP_BODY" confirmed)" == "true" ]]; then
    pass "teardown destroyed $vm_id (confirmed)"
  else
    fail "teardown → HTTP $code $HTTP_BODY — check the KVM host: /srv/jailer/firecracker/$vm_id, nft list tables"
  fi
  http GET "$base/v1/vms/by-topic/$topic" "" "${AGENT_ARGS[@]}" -K "$hdr"; code="$HTTP_CODE"
  if [[ "$code" == "404" ]]; then
    pass "attach after destroy → 404 (nothing left for $topic)"
    PROBE_VM=""; PROBE_TOPIC_LIVE=""
  else
    fail "attach after destroy → HTTP $code $HTTP_BODY"
    # The exit path retries the destroy by topic before the script ends.
  fi
  LOG "on the KVM host: journalctl -u proof-vm-orchestrator | grep -E 'topic vm booted|torn down'; ls /srv/jailer/firecracker/ must not list $vm_id"
}

# ---------------------------------------------------------------------------
# submit-probe: POST /v1/submissions on a custom topic, assert the answer
# ---------------------------------------------------------------------------
submit_probe() {
  [[ -n "$TOPIC" && -n "$EXPECT" ]] || { RED "submit-probe needs --topic ID --expect CODE"; exit 1; }
  if [[ "$EXPECT" =~ ^2 && "$ALLOW_LIVE_RUN" -ne 1 ]]; then
    RED "refusing: --expect $EXPECT means a real RLM job (topic VM + sister guest + paid inference). Pass --allow-live-run and a fetchable --artifact-uri."
    exit 2
  fi
  resolve_cp || return 0
  # A real run measures FLOPs in the sister and the CP rejects a run over its
  # declaration (flops_under_declared). The fail-closed probes never run, so
  # they declare 1; a live run declares the topic's whole budget unless told
  # otherwise (over the budget is a 400 before anything runs).
  local flops="${DECLARED_FLOPS:-1}"
  if [[ -z "$DECLARED_FLOPS" && "$EXPECT" =~ ^2 ]]; then
    http GET "$CP/v1/proof/topics/$TOPIC" ""
    flops="$(jget "$HTTP_BODY" flops_budget)"
    if [[ "$HTTP_CODE" != "200" || ! "$flops" =~ ^[0-9]+$ || "$flops" == "0" ]]; then
      fail "cannot read flops_budget of topic $TOPIC (HTTP $HTTP_CODE); pass --declared-flops N for the live run"
      return 0
    fi
    LOG "live run declares the topic budget: declared_flops=$flops"
  fi
  [[ "$flops" =~ ^[0-9]+$ ]] || { RED "--declared-flops must be an integer"; exit 1; }
  local hotkey hex uri_field="" body code
  hotkey="$(head -c 64 /dev/zero | tr '\0' 'a')"
  hex="$(printf '%s' "wire-probe-$TOPIC-$(date +%s)-$$-$RANDOM" | sha256sum | awk '{print $1}')"
  [[ -n "$ARTIFACT_URI" ]] && uri_field="$(printf '"artifact_uri":"%s",' "$ARTIFACT_URI")"
  body="$(printf '{"miner_hotkey":"%s","artifact_digest":"%s",%s"claim":"proof-vm-wire-check probe","declared_flops":%s,"topic_id":"%s","manifest":{"train_dataset_ids":["wire-probe-v0"]}}' \
    "$hotkey" "$hex" "$uri_field" "$flops" "$TOPIC")"
  LOG "submit-probe: POST $CP/v1/submissions topic=$TOPIC expect=$EXPECT declared_flops=$flops${REASON:+ reason~'$REASON'}${ARTIFACT_URI:+ artifact_uri=$ARTIFACT_URI}"
  http POST "$CP/v1/submissions" "$body" -m "$WAIT_SECS"; code="$HTTP_CODE"
  LOG "→ HTTP $code $(printf '%s' "$HTTP_BODY" | head -c 500)"
  if [[ "$code" != "$EXPECT" ]]; then
    fail "expected HTTP $EXPECT, got $code"
    return 0
  fi
  if [[ "$code" =~ ^[45] ]]; then
    local error
    error="$(jget "$HTTP_BODY" error)"
    if [[ -n "$error" ]]; then pass "HTTP $code carries an explicit error (fail-closed, no silent empty)"; else fail "HTTP $code with no error field"; fi
    if [[ -n "$REASON" ]]; then
      if printf '%s' "$error" | grep -qF "$REASON"; then pass "error names '$REASON'"; else fail "error does not name '$REASON': $error"; fi
    fi
    return 0
  fi
  local id state
  id="$(jget "$HTTP_BODY" id)"
  [[ "$id" == pf_* ]] || { fail "2xx without a pf_ id"; return 0; }
  state="$(jget "$HTTP_BODY" state)"
  pass "submission scored synchronously: $id (state=$state)"
  http GET "$CP/v1/submissions/$id" ""; code="$HTTP_CODE"
  [[ "$code" == "200" ]] || { fail "GET /v1/submissions/$id → $code"; return 0; }
  local flops pass_flag
  flops="$(jget "$HTTP_BODY" verdict.agent.flops_used)"; pass_flag="$(jget "$HTTP_BODY" verdict.pass)"
  LOG "row $id: state=$state pass=$pass_flag flops_used=$flops detail=$(jget "$HTTP_BODY" detail) failed=$(jget "$HTTP_BODY" verdict.failed)"
  if [[ "$state" == "awaiting_admin" && -n "$flops" && "$flops" != "0" ]]; then
    pass "verdict carries the sister-measured flops_used=$flops (host-stamped, never RLM-authored)"
  else
    fail "no host-measured flops_used on a scored row (a report without a measurement is 503, never a substituted number)"
  fi
  LOG "sandboxed=true lives in the artefact: docker compose cp proof-challenge:/var/lib/proof/artefacts/$TOPIC/$id.zip /tmp/ && unzip -p /tmp/$id.zip report.json"
  LOG "on the KVM host: journalctl -u proof-vm-orchestrator | grep -E 'sister guest booting|sister guest run attested|jail released'"
}

# ---------------------------------------------------------------------------
# matrix: the fail-closed table as operator steps
# ---------------------------------------------------------------------------
print_matrix() {
  load_env
  local cp="${CP:-http://127.0.0.1:8080/challenge/proof}" topic="${TOPIC:-<open-custom-topic-id>}"
  local token_host
  token_host="$(map_path "${TOKEN_PATH:-/run/base/proof/vm_orchestrator_token}")"
  cat <<EOF
# Fail-closed matrix — Proof topic-VM orchestrator (staging). Every row: POST /v1/submissions
# on an open custom topic answers 503 with the reason below, NO row is persisted, NO VM boots,
# NOTHING is rented. Flip one knob at a time, probe, restore, re-run '$0 cp'.
#
# 0. Baseline: everything wired.
$0 all && $0 boot-probe
#
# 1. URL unset → UnwiredVmOrchestrator (restart required: the URL is read at boot).
#    edit $ENV_FILE: comment out PROOF_VM_ORCHESTRATOR_URL; docker compose ... up -d proof-challenge
$0 submit-probe --cp $cp --topic $topic --expect 503 --reason PROOF_VM_ORCHESTRATOR_URL
#    restore the URL, restart proof-challenge.
#
# 2. Bearer file emptied on the CP (no restart: re-read per request).
: > $token_host
$0 submit-probe --cp $cp --topic $topic --expect 503 --reason PROOF_VM_ORCHESTRATOR_TOKEN_FILE
#    restore the bearer bytes (same as /etc/proof-vm/token on the KVM host), mode 0400, uid 65532.
#
# 3. Bearer bytes differ from the agent's (no restart) → agent 401 → CP 503.
head -c 32 /dev/urandom | base64 -w0 > $token_host
$0 submit-probe --cp $cp --topic $topic --expect 503 --reason 'refused the bearer'
#    restore the bearer bytes.
#
# 4. RLM image digest unpinned (restart required: the pin is read at boot).
#    edit $ENV_FILE: PROOF_RLM_VM_IMAGE_DIGEST= ; docker compose ... up -d proof-challenge
$0 submit-probe --cp $cp --topic $topic --expect 503 --reason PROOF_RLM_VM_IMAGE_DIGEST
#    restore the digest (sha256sum of the staged rootfs; never invented), restart.
#
# 5. Agent down (on the KVM host: systemctl stop proof-vm-orchestrator).
$0 submit-probe --cp $cp --topic $topic --expect 503 --reason 'orchestrator unreachable'
#    systemctl start proof-vm-orchestrator; then: $0 agent
#
# 6. Unknown / closed topic → 400 (no row); a custom topic without artifact_uri → 400 (no row).
$0 submit-probe --cp $cp --topic does-not-exist --expect 400 --reason 'unknown topic'
$0 submit-probe --cp $cp --topic $topic --expect 400 --no-artifact-uri --reason artifact_uri
#
# After every row: $0 cp   (the admin probe shows the same root cause: ready / reason / agent_error)
EOF
}

# ---------------------------------------------------------------------------
summary() {
  echo
  if [[ "$FAILS" -gt 0 ]]; then
    RED "proof-vm-wire-check: $FAILS FAIL, $WARNS WARN"
    exit 1
  fi
  GRN "proof-vm-wire-check: all PASS ($WARNS WARN)"
}

case "$SUBCOMMAND" in
  env) check_env; summary ;;
  agent) check_agent; summary ;;
  cp) check_cp; summary ;;
  all) check_env; check_agent; check_cp; summary ;;
  boot-probe) boot_probe; summary ;;
  submit-probe) submit_probe; summary ;;
  matrix) print_matrix ;;
  -h|--help|help) usage ;;
  *) RED "unknown subcommand: $SUBCOMMAND"; usage; exit 1 ;;
esac
