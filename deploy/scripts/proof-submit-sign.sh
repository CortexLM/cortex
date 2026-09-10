#!/usr/bin/env bash
# Sign a Proof submit payload (`base-proof-submit-v1`).
#
# Prints JSON: miner_hotkey, hotkey_signature, submit_nonce, manifest, domain.
# The manifest is part of the signed bytes: POST exactly the `manifest`
# printed here (or one that parses to the same lists). Each run draws a fresh
# single-use submit_nonce unless --submit-nonce pins one.
# Never prints the mini-secret.
#
# Usage:
#   deploy/scripts/proof-submit-sign.sh \
#     --topic-id ID --artifact-digest HEX --declared-flops N --claim TEXT \
#     [--train-dataset ID]... [--train-hash HEX]... \
#     [--submit-nonce HEX64] [--secret-file PATH]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TOPIC=""
DIGEST=""
FLOPS=""
CLAIM=""
SECRET=""
NONCE=""
MANIFEST_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --topic-id) TOPIC="$2"; shift 2 ;;
    --artifact-digest) DIGEST="$2"; shift 2 ;;
    --declared-flops) FLOPS="$2"; shift 2 ;;
    --claim) CLAIM="$2"; shift 2 ;;
    --secret-file) SECRET="$2"; shift 2 ;;
    --submit-nonce) NONCE="$2"; shift 2 ;;
    --train-dataset|--train-hash) MANIFEST_ARGS+=("$1" "$2"); shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$TOPIC" || -z "$DIGEST" || -z "$FLOPS" || -z "$CLAIM" ]]; then
  echo "need --topic-id --artifact-digest --declared-flops --claim" >&2
  exit 2
fi

cleanup=""
if [[ -z "$SECRET" ]]; then
  SECRET="$(mktemp)"
  cleanup="$SECRET"
  # In-repo probe mini-secret (32 bytes as 64 hex). Not a live miner.
  printf '%s' '4211111111111111111111111111111111111111111111111111111111111111' >"$SECRET"
fi

SIGN_ARGS=(
  --secret-file "$SECRET"
  --topic-id "$TOPIC"
  --artifact-digest "$DIGEST"
  --declared-flops "$FLOPS"
  --claim "$CLAIM"
)
[[ -n "$NONCE" ]] && SIGN_ARGS+=(--submit-nonce "$NONCE")
if [[ ${#MANIFEST_ARGS[@]} -gt 0 ]]; then
  SIGN_ARGS+=("${MANIFEST_ARGS[@]}")
fi

run_ctx() {
  if command -v ctx >/dev/null 2>&1; then
    ctx --json proof sign "${SIGN_ARGS[@]}"
    return
  fi
  if command -v cargo >/dev/null 2>&1; then
    cargo run -q -p ctx -- --json proof sign "${SIGN_ARGS[@]}"
    return
  fi
  echo "need ctx or cargo to sign a Proof submit (base-proof-submit-v1)" >&2
  exit 1
}

cd "$ROOT"
out="$(run_ctx)"
if [[ -n "$cleanup" ]]; then rm -f "$cleanup"; fi
printf '%s\n' "$out"
