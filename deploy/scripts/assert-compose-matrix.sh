#!/usr/bin/env bash
# Assert compose role × env matrix is consistent.
#
#   assert-compose-matrix.sh
#
# Verifies:
#   - validator role never renders gateway
#   - evil-gateway never renders outside its profile
#   - prod env never renders e2e or evil-gateway overrides
#   - master role renders gateway
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

cd "$ROOT"

fail() { echo "assert-compose-matrix: FAIL: $*" >&2; exit 1; }

# deploy/env/*.env holds real config and is gitignored, so it is absent on a
# fresh checkout. `docker compose config` validates `env_file: required: true`,
# so without these the renders below fail and every grep passes vacuously.
# Materialise the examples for the duration of the run.
CREATED=()
cleanup() { for f in ${CREATED+"${CREATED[@]}"}; do rm -f "$f"; done; }
trap cleanup EXIT
for ex in deploy/env/*.env.example; do
  [ -e "$ex" ] || continue
  real="${ex%.example}"
  if [ ! -e "$real" ]; then
    cp "$ex" "$real"
    CREATED+=("$real")
  fi
done

# Render a compose combination, failing loudly instead of yielding "" on error.
render() {
  local out
  if ! out=$(docker compose "$@" 2>&1); then
    echo "$out" >&2
    fail "docker compose $* failed to render"
  fi
  printf '%s\n' "$out"
}

# --- validator role: no gateway, no challenge execution surface ---
services=$(render \
  -f docker-compose.yml \
  -f deploy/compose/role-validator.yml \
  config --services)
if echo "$services" | grep -qx "gateway"; then
  fail "validator role renders gateway (must not)"
fi
for banned in relearn-challenge relearn-t2i-challenge relearn-mm-challenge bounty-challenge socket-proxy design-challenge design-egress-proxy prism-challenge; do
  if echo "$services" | grep -qx "$banned"; then
    fail "validator role renders $banned (master-only; must not)"
  fi
done
echo "OK: validator role does not render gateway or challenge services"

# --- master role: gateway + challenge services present; no on-chain validator ---
services=$(render \
  -f docker-compose.yml \
  -f deploy/compose/role-master.yml \
  --profile master \
  config --services)
if ! echo "$services" | grep -qx "gateway"; then
  fail "master role does not render gateway (must)"
fi
for required in relearn-challenge relearn-t2i-challenge relearn-mm-challenge bounty-challenge socket-proxy; do
  if ! echo "$services" | grep -qx "$required"; then
    fail "master role does not render $required (must)"
  fi
done
for retired in design-challenge design-egress-proxy prism-challenge; do
  if echo "$services" | grep -qx "$retired"; then
    fail "master role still renders retired $retired"
  fi
done
if echo "$services" | grep -qx "validator"; then
  fail "master role renders validator (dual submitter; must not — use validator host)"
fi
echo "OK: master role renders gateway and challenge services (no validator)"

# --- evil-gateway not in default or master ---
services=$(render \
  -f docker-compose.yml \
  --profile master \
  config --services)
if echo "$services" | grep -qx "evil-gateway"; then
  fail "evil-gateway renders under master profile (must not)"
fi
echo "OK: evil-gateway not under master profile"

services=$(render \
  -f docker-compose.yml \
  config --services)
if echo "$services" | grep -qx "evil-gateway"; then
  fail "evil-gateway renders under default profile (must not)"
fi
echo "OK: evil-gateway not under default profile"

# --- prod env + validator: no evil-gateway, no e2e, no challenges ---
services=$(render \
  -f docker-compose.yml \
  -f deploy/compose/role-validator.yml \
  -f deploy/compose/env-prod.yml \
  config --services)
if echo "$services" | grep -qx "evil-gateway"; then
  fail "prod validator renders evil-gateway (must not)"
fi
for banned in relearn-challenge relearn-t2i-challenge relearn-mm-challenge bounty-challenge socket-proxy design-challenge design-egress-proxy prism-challenge; do
  if echo "$services" | grep -qx "$banned"; then
    fail "prod validator renders $banned (master-only; must not)"
  fi
done
echo "OK: prod validator does not render evil-gateway or challenge services"

# --- staging/prod never enable host SimSandbox ---
for env_file in deploy/compose/env-staging.yml deploy/compose/env-prod.yml; do
  rendered=$(render \
    -f docker-compose.yml \
    -f deploy/compose/role-master.yml \
    -f "$env_file" \
    --profile master \
    config)
  if echo "$rendered" | grep -qE 'BASE_ALLOW_HOST_SIM:[[:space:]]*["'\'']?(1|true|TRUE|yes)["'\'']?'; then
    fail "$env_file enables BASE_ALLOW_HOST_SIM (host Sim forbidden on droplets)"
  fi
  if echo "$rendered" | grep -qE 'DESIGN_FORCE_SIM:[[:space:]]*["'\'']?(1|true|TRUE|yes)["'\'']?'; then
    fail "$env_file enables DESIGN_FORCE_SIM (retired; must not ship)"
  fi
  for sim_var in RELEARN_FORCE_SIM RELEARN_T2I_FORCE_SIM RELEARN_MM_FORCE_SIM; do
    if echo "$rendered" | grep -qE "${sim_var}:[[:space:]]*[\"']?(1|true|TRUE|yes)[\"']?"; then
      fail "$env_file enables $sim_var (sim is local-only; must not ship on droplets)"
    fi
  done
done
echo "OK: staging/prod do not enable host SimSandbox"

# --- all four live challenges present in default; design/prism retired ---
default_services=$(render \
  -f docker-compose.yml \
  config --services)
for required in relearn-challenge relearn-t2i-challenge relearn-mm-challenge bounty-challenge; do
  if ! echo "$default_services" | grep -qx "$required"; then
    fail "$required not in default compose"
  fi
done
for retired in prism-challenge design-challenge design-egress-proxy; do
  if echo "$default_services" | grep -qx "$retired"; then
    fail "retired $retired still in default compose"
  fi
done
echo "OK: relearn, relearn-t2i, relearn-mm, bounty in default compose; design/prism retired"

# --- no fake chain backend survives anywhere in the matrix ---
for env_file in deploy/compose/env-staging.yml deploy/compose/env-prod.yml; do
  for role_file in deploy/compose/role-master.yml deploy/compose/role-validator.yml; do
    rendered=$(render \
      -f docker-compose.yml \
      -f "$role_file" \
      -f "$env_file" \
      --profile master \
      config)
    if echo "$rendered" | grep -qi "fake_owner\|BASE_CHAIN_BACKEND"; then
      fail "$role_file + $env_file still references a fake chain backend"
    fi
    if ! echo "$rendered" | grep -q "BASE_CHAIN_ENDPOINT"; then
      fail "$role_file + $env_file does not set BASE_CHAIN_ENDPOINT"
    fi
  done
done
echo "OK: no fake chain backend in any role x env combination"

# --- each env pins its own netuid and endpoint ---
staging=$(render -f docker-compose.yml -f deploy/compose/role-master.yml \
  -f deploy/compose/env-staging.yml --profile master config)
echo "$staging" | grep -q "test.finney.opentensor.ai" \
  || fail "staging does not point at the testnet endpoint"
echo "$staging" | grep -q "BASE_NETUID: \"541\"" \
  || fail "staging netuid is not 541"

prod=$(render -f docker-compose.yml -f deploy/compose/role-master.yml \
  -f deploy/compose/env-prod.yml --profile master config)
echo "$prod" | grep -q "entrypoint-finney.opentensor.ai" \
  || fail "prod does not point at the mainnet endpoint"
echo "$prod" | grep -q "BASE_NETUID: \"100\"" \
  || fail "prod netuid is not 100"
echo "$prod" | grep -q "test.finney" \
  && fail "prod references the testnet endpoint"
echo "OK: staging pins testnet/541 and prod pins mainnet/100"

# --- local overlay: still testnet, no fake backend ---
local_rendered=$(render \
  -f docker-compose.yml \
  -f deploy/compose/role-master.yml \
  -f deploy/compose/env-staging.yml \
  -f deploy/compose/env-local.yml \
  --profile master \
  config)
if echo "$local_rendered" | grep -qi "fake_owner\|BASE_CHAIN_BACKEND"; then
  fail "env-local introduces a fake chain backend"
fi
echo "$local_rendered" | grep -q "test.finney.opentensor.ai" \
  || fail "env-local stack lost the testnet endpoint"
echo "$local_rendered" | grep -q "BASE_NETUID: \"541\"" \
  || fail "env-local stack lost netuid 541"
local_services=$(render \
  -f docker-compose.yml \
  -f deploy/compose/role-master.yml \
  -f deploy/compose/env-staging.yml \
  -f deploy/compose/env-local.yml \
  --profile master \
  config --services)
echo "$local_services" | grep -qx "gateway" \
  || fail "env-local master stack does not render gateway"
echo "$local_services" | grep -qx "validator" \
  || fail "env-local master stack does not render co-located validator"
for required in relearn-challenge relearn-t2i-challenge relearn-mm-challenge bounty-challenge; do
  echo "$local_services" | grep -qx "$required" \
    || fail "env-local master stack does not render $required"
done
for probe in 28095:relearn-challenge 28096:bounty-challenge \
             28097:relearn-t2i-challenge 28098:relearn-mm-challenge; do
  port=${probe%%:*}
  svc=${probe#*:}
  echo "$local_rendered" | grep -qE "published: \"?${port}\"?" \
    || fail "env-local does not publish $svc on $port"
done
for banned in agent-challenge hypertraining-challenge miner-agent miner-socket-proxy base-agent; do
  if echo "$local_services" | grep -qx "$banned"; then
    fail "removed service still rendered: $banned"
  fi
done
# Default compose must also forbid removed services.
for banned in agent-challenge hypertraining-challenge miner-agent miner-socket-proxy base-agent; do
  if echo "$default_services" | grep -qx "$banned"; then
    fail "removed service still in default compose: $banned"
  fi
done
echo "OK: env-local preserves testnet/541; challenges on 28095-28098; removed agent/hypertraining/miner services"

echo "assert-compose-matrix: all checks passed"
