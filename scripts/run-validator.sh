#!/usr/bin/env bash
# Run a Cortex validator: verify the sealed weights served by the Cortex
# gateway and submit them on Bittensor. This script never starts the master,
# the challenge supervisor or a VM host; those are for the master operator only.
#
# Usage (from anywhere; paths resolve to this repository):
#   export WALLET_NAME=validator WALLET_HOTKEY=default
#   export GATEWAY_PUBLIC=<independently pinned gateway hotkey>
#   scripts/run-validator.sh --verify-only   # one verification, never submits
#   scripts/run-validator.sh                 # verify and submit every epoch
#
# Required env: WALLET_NAME, WALLET_HOTKEY, GATEWAY_PUBLIC.
# Optional env: GATEWAY (https://chain.joinbase.ai), NETUID (100),
#   NETWORK (finney), STATE_DB ($HOME/.cortex/validator.sqlite3),
#   WALLET_PATH ($HOME/.bittensor/wallets), VERSION_KEY (default: live
#   subnet weights_version read from the chain).
# Flags: --verify-only, --once, --dry-run, --help. Run `uv sync --locked
# --extra chain` once before the first run.
set -euo pipefail

die() { echo "run-validator: $*" >&2; exit 1; }

# Hard guard: refuse any master-only intent before doing anything else.
for arg in "$@"; do
  case "$arg" in
    master | challenge-supervisor | vm-host)
      echo "run-validator: refused: '$arg' is a master-operator command; this script only runs a validator" >&2
      exit 2 ;;
  esac
done
if [[ "${CORTEX_MASTER:-}" == 1 || -n "${BASE_MASTER_SECRETS_HOST_DIR+x}" || -n "${BASE_CHALLENGE_SECRETS_HOST_DIR+x}" ]]; then
  echo "run-validator: refused: master environment detected (CORTEX_MASTER=1, BASE_MASTER_SECRETS_HOST_DIR or BASE_CHALLENGE_SECRETS_HOST_DIR); unset it to run a validator" >&2
  exit 2
fi

usage() { sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; }

verify_only=0 once=0 dry_run=0
for arg in "$@"; do
  case "$arg" in
    --verify-only) verify_only=1 ;;
    --once) once=1 ;;
    --dry-run) dry_run=1 ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown argument: $arg (see --help)" ;;
  esac
done

GATEWAY="${GATEWAY:-https://chain.joinbase.ai}"
GATEWAY="${GATEWAY%/}"
NETUID="${NETUID:-100}"
NETWORK="${NETWORK:-finney}"
STATE_DB="${STATE_DB:-$HOME/.cortex/validator.sqlite3}"
WALLET_PATH="${WALLET_PATH:-$HOME/.bittensor/wallets}"
: "${WALLET_NAME:?set WALLET_NAME to your validator wallet name}"
: "${WALLET_HOTKEY:?set WALLET_HOTKEY to your registered validator hotkey}"
[[ -n "${GATEWAY_PUBLIC:-}" ]] || die "GATEWAY_PUBLIC is not set: export the gateway hotkey you pinned from an independent, trusted source (never copy it from the gateway response)"
[[ "$NETUID" =~ ^[0-9]+$ ]] || die "NETUID must be an integer"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
CONFIG="$ROOT/config"
UV=(uv run --locked --extra chain)

command -v uv >/dev/null || die "uv not found: install it from https://docs.astral.sh/uv/"
command -v curl >/dev/null || die "curl not found"
body="$(mktemp)"
trap 'rm -f "$body"' EXIT
status="$(curl -sS -m 20 -o "$body" -w '%{http_code}' "$GATEWAY/v1/weights/latest")" \
  || die "cannot reach $GATEWAY/v1/weights/latest"
[[ "$status" == 200 ]] || die "$GATEWAY/v1/weights/latest answered HTTP $status, expected 200"

read -r challenges_version measurements_version epoch sealed < <(
  "${UV[@]}" python - "$body" "$NETUID" "$CONFIG" <<'PY' || echo FAIL
import json, sys, tomllib
from pathlib import Path
if sys.version_info < (3, 12):
    sys.exit(f"python >= 3.12 required, found {sys.version.split()[0]}")
body, netuid, config = sys.argv[1], int(sys.argv[2]), Path(sys.argv[3])
try:
    latest = json.loads(Path(body).read_text())
except ValueError:
    sys.exit("gateway answered non-JSON for /v1/weights/latest")
if not isinstance(latest, dict) or latest.get("netuid") != netuid:
    sys.exit(f"gateway netuid {latest.get('netuid') if isinstance(latest, dict) else None} != NETUID {netuid}")
versions = [tomllib.loads((config / n).read_text())["version"] for n in ("challenges.toml", "measurements.toml")]
print(*versions, latest.get("epoch"), str(latest.get("sealed")).lower())
PY
)
[[ "$challenges_version" != FAIL && -n "$sealed" ]] || die "preflight failed (see message above)"
echo "run-validator: gateway $GATEWAY netuid=$NETUID epoch=$epoch sealed=$sealed" >&2

hotkey_file="$WALLET_PATH/$WALLET_NAME/hotkeys/$WALLET_HOTKEY"
if [[ ! -f "$hotkey_file" ]]; then
  ((dry_run)) || die "wallet hotkey file not found: $hotkey_file"
  echo "run-validator: warning: wallet hotkey file not found (ignored in --dry-run)" >&2
fi

if [[ -z "${VERSION_KEY:-}" ]]; then
  VERSION_KEY="$("${UV[@]}" python -c '
import sys
from bittensor import Subtensor
s = Subtensor(network=sys.argv[1])
try:
    print(s.get_subnet_hyperparameters(int(sys.argv[2])).weights_version)
finally:
    s.close()
' "$NETWORK" "$NETUID" 2>/dev/null | tail -n 1)" || true
  [[ "$VERSION_KEY" =~ ^[0-9]+$ ]] || die "cannot read weights_version for netuid $NETUID on $NETWORK; set VERSION_KEY explicitly"
fi

cmd=(cortex validator
  --gateway "$GATEWAY" --netuid "$NETUID" --gateway-public "$GATEWAY_PUBLIC"
  --network "$NETWORK"
  --owner-public "$CONFIG/owner.pubkey"
  --challenges "$CONFIG/challenges.toml" --measurements "$CONFIG/measurements.toml"
  --minimum-challenges-version "$challenges_version"
  --minimum-measurements-version "$measurements_version"
  --version-key "$VERSION_KEY"
  --wallet-name "$WALLET_NAME" --wallet-hotkey "$WALLET_HOTKEY" --wallet-path "$WALLET_PATH"
  --state-db "$STATE_DB")
if ((verify_only)); then cmd+=(--verify-only --once); elif ((once)); then cmd+=(--once); fi

if ((dry_run)); then
  shown=() previous=
  for word in "${cmd[@]}"; do
    case "$previous" in --wallet-name | --wallet-hotkey) word='***' ;; esac
    shown+=("$word") previous="$word"
  done
  printf '%s' "${UV[*]}"; printf ' %q' "${shown[@]}"; printf '\n'
  exit 0
fi

mkdir -p "$(dirname "$STATE_DB")"
rm -f "$body"
exec "${UV[@]}" "${cmd[@]}"
