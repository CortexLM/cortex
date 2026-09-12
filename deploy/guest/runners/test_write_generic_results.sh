#!/bin/sh
# Unit tests for write-generic-results.sh (POSIX; no python3).
set -eu
ROOT="$(CDPATH= cd "$(dirname "$0")" && pwd)"
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/write-generic-results-XXXXXX")"
cleanup() { rm -rf "$WORKDIR"; }
trap cleanup EXIT

export PROOF_OUTPUT_DIR="$WORKDIR/out"
export PROOF_TOPIC_ID=topic-a
export PROOF_CUSTOM_ID=custom-a
export PROOF_SUBMISSION_DIGEST=aa
export PROOF_ARTIFACT_DIGEST=bb
mkdir -p "$PROOF_OUTPUT_DIR"
unset PROOF_PARAM_RESULTS_PATH || true

# Nested evidence must not bind. Root 0.75 / true wins over 999 / false.
cat > "$PROOF_OUTPUT_DIR/report.json" <<'EOF'
{
  "evidence": {"primary_value": 999, "claim_holds": false},
  "primary_value": 0.75,
  "claim_holds": true
}
EOF
# shellcheck source=write-generic-results.sh
. "$ROOT/write-generic-results.sh"
[ -f "$PROOF_OUTPUT_DIR/results.json" ] || fail "default results.json missing"
grep -q '"primary_value":0.75' "$PROOF_OUTPUT_DIR/results.json" \
    || fail "nested primary_value was bound: $(cat "$PROOF_OUTPUT_DIR/results.json")"
grep -q '"claim_holds":true' "$PROOF_OUTPUT_DIR/results.json" \
    || fail "nested claim_holds was bound: $(cat "$PROOF_OUTPUT_DIR/results.json")"
pass "root primary_value / claim_holds win over nested keys"

rm -f "$PROOF_OUTPUT_DIR/results.json" "$PROOF_OUTPUT_DIR/audit.v1.final.json"
export PROOF_PARAM_RESULTS_PATH=audit.v1.final.json
# shellcheck source=write-generic-results.sh
. "$ROOT/write-generic-results.sh"
[ -f "$PROOF_OUTPUT_DIR/audit.v1.final.json" ] || fail "multi-dot pin was not written"
[ ! -f "$PROOF_OUTPUT_DIR/results.json" ] || fail "multi-dot pin fell back to results.json"
grep -q '"primary_value":0.75' "$PROOF_OUTPUT_DIR/audit.v1.final.json" \
    || fail "pinned file missing root bind"
pass "audit.v1.final.json pin is honored (no results.json fallback)"

unset PROOF_PARAM_RESULTS_PATH
export PROOF_PARAM_RESULTS_PATH='../../outside.json'
if sh "$ROOT/write-generic-results.sh"; then
    fail "traversal pin must exit"
fi
[ ! -f "$WORKDIR/outside.json" ] || fail "traversal pin wrote outside the output dir"
pass "traversal pin is fail-closed"

echo "all write-generic-results tests passed"
