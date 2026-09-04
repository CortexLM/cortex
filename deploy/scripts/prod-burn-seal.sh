#!/usr/bin/env bash
# Interim prod burn-seal: keep a fresh sealed bundle on the master gateway so
# the Rust validator can Match + CRV4 submit every rate-limit window.
#
# Why: the validator verifies the bundle metagraph at the seal's pinned block.
# The public Finney RPC prunes state after ~256 blocks (~51 min), so a seal
# older than that can never Match and weights freeze on-chain. This script
# re-seals at the chain tip; run it on a systemd timer
# (deploy/systemd/base-burn-seal.timer, 21 min — just above the 100-block
# WeightsSetRateLimit, well inside the pruning window).
#
# Runs on the prod master only. No secrets are stored in this file; the
# challenge mini-secrets stay at $BASE_BOUNTY_SK_FILE / $BASE_PROOF_SK_FILE
# (mode 0400).
#
# D24 (exact-E): every challenge with emission_share_bps > 0 must have a
# complete leaf set. Live trust root is bounty = 2000 / proof = 8000, so this
# script emits bounty then proof NoScore, then seals. All-NoScore still
# aggregates to uid-0 burn. A paid challenge with no leaves 409s the seal
# for every challenge.
set -euo pipefail

BASE_HOME="${BASE_HOME:-/opt/base}"
GATEWAY="${BASE_GATEWAY_ENDPOINT:-http://127.0.0.1:8080}"
NETUID="${BASE_NETUID:-100}"
# Ordered failover list wins; weights-smoke passes it straight to
# chain-live, which cools a rate-limited endpoint and tries the next.
CHAIN="${BASE_CHAIN_ENDPOINTS:-${BASE_CHAIN_ENDPOINT:-wss://entrypoint-finney.opentensor.ai:443}}"
BOUNTY_SK="${BASE_BOUNTY_SK_FILE:-${BASE_HOME}/deploy/secrets/bounty_sk}"
PROOF_SK="${BASE_PROOF_SK_FILE:-${BASE_HOME}/deploy/secrets/proof_sk}"
BIN="${WEIGHTS_SMOKE_BIN:-${BASE_HOME}/bin/weights-smoke}"
LOG="${BURN_SEAL_LOG:-/var/log/base-burn-seal.log}"
LOCK="${BURN_SEAL_LOCK:-/run/base-burn-seal.lock}"
# Admin bearer for /v1/admin/seal (required once gateway enforces it).
if [[ -z "${BASE_GATEWAY_ADMIN_TOKEN:-}" && -z "${BASE_GATEWAY_ADMIN_TOKEN_FILE:-}" \
  && -f "${BASE_HOME}/deploy/secrets/gateway_admin_token" ]]; then
  export BASE_GATEWAY_ADMIN_TOKEN_FILE="${BASE_HOME}/deploy/secrets/gateway_admin_token"
fi

exec 9>"${LOCK}"
if ! flock -n 9; then
  echo "$(date -Is) skip: another run holds ${LOCK}" >>"${LOG}"
  exit 0
fi

smoke() {
  local challenge_id="$1" sk="$2"
  shift 2
  "${BIN}" --gateway "${GATEWAY}" --burn --netuid "${NETUID}" \
    --chain-endpoint "${CHAIN}" --challenge-id "${challenge_id}" \
    --challenge-sk "${sk}" "$@"
}

{
  echo "$(date -Is) seal start gateway=${GATEWAY} netuid=${NETUID} challenges=bounty,proof"
  if out="$( { smoke bounty "${BOUNTY_SK}" --skip-seal && smoke proof "${PROOF_SK}"; } 2>&1 )"; then
    echo "${out}" | grep -E 'seal ok|latest OK' || echo "${out}" | tail -3
    echo "$(date -Is) seal ok"
  else
    rc=$?
    echo "${out}" | tail -8
    echo "$(date -Is) seal FAILED rc=${rc}"
    exit "${rc}"
  fi
} >>"${LOG}" 2>&1
