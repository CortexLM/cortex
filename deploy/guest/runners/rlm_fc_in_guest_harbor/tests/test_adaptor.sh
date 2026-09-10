#!/bin/bash
# Unit tests for lib.sh + run-harbor without a real Harbor install.
set -euo pipefail
ADAPTOR="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=../lib.sh
. "$ADAPTOR/lib.sh"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/harbor-adaptor-XXXXXX")"
cleanup() { rm -rf "$WORKDIR"; }
trap cleanup EXIT

export PROOF_WORK_DIR="$WORKDIR/work"
export PROOF_OUTPUT_DIR="$WORKDIR/out"
export PROOF_PACK_DIR="$WORKDIR/pack"
export PROOF_HARNESS_SKIP_PODMAN=1
mkdir -p "$PROOF_WORK_DIR" "$PROOF_OUTPUT_DIR" "$PROOF_PACK_DIR/tasks/hello"
export PROOF_PARAM_TASKS_DIR="tasks"

# --- tasks_dir ---
if (export PROOF_PARAM_TASKS_DIR=".."; proof_require_tasks) 2>/dev/null; then
    fail "tasks_dir=.. must be refused"
fi
export PROOF_PARAM_TASKS_DIR="tasks"
proof_require_tasks || fail "tasks_dir=tasks should work"
pass "tasks_dir relative ok, .. refused"

# --- BYOK evaluate never owner ---
export PROOF_JOB=evaluate
export PROOF_PARAM_MINER_BYOK=OPENROUTER_API_KEY
export PROOF_MINER_ENV_DIR="$WORKDIR/miner-env"
export PROOF_SECRETS_DIR="$WORKDIR/secrets"
export PROOF_PARAM_INFERENCE_KEY_FILE=inference_key
export PROOF_PARAM_INFERENCE_KEY_ENV=OPENROUTER_API_KEY
mkdir -p "$PROOF_MINER_ENV_DIR" "$PROOF_SECRETS_DIR"
printf 'owner-secret-key' > "$PROOF_SECRETS_DIR/inference_key"
chmod 0600 "$PROOF_SECRETS_DIR/inference_key"
unset OPENROUTER_API_KEY || true
if (proof_load_inference_key) 2>/dev/null; then
    fail "evaluate without miner BYOK file must fail (no owner fallback)"
fi
pass "evaluate missing BYOK refuses owner key"

printf 'miner-secret-key' > "$PROOF_MINER_ENV_DIR/OPENROUTER_API_KEY"
chmod 0600 "$PROOF_MINER_ENV_DIR/OPENROUTER_API_KEY"
unset OPENROUTER_API_KEY || true
proof_load_inference_key || fail "evaluate with miner BYOK file should load"
[ "${OPENROUTER_API_KEY:-}" = "miner-secret-key" ] || fail "evaluate loaded owner key instead of miner"
pass "evaluate BYOK prefers miner file"

# --- BYOK baseline without miner file uses owner ---
export PROOF_JOB=baseline
unset OPENROUTER_API_KEY || true
rm -f "$PROOF_MINER_ENV_DIR/OPENROUTER_API_KEY"
proof_load_inference_key || fail "baseline without miner file should use owner key"
[ "${OPENROUTER_API_KEY:-}" = "owner-secret-key" ] || fail "baseline did not load owner key"
pass "baseline without miner BYOK uses owner key"

# --- agent selection ---
FIXTURES="$ADAPTOR/tests/fixtures"
export PROOF_JOB=evaluate
export PROOF_ARTIFACT_DIR="$FIXTURES"
# fixtures has agent/ at fixtures/agent and recipe/agent — prefer top-level agent/
got="$(proof_select_harbor_agent)" || fail "evaluate should select fixtures/agent"
[ "$got" = "agent.agent:MinerAgent" ] || fail "expected agent.agent:MinerAgent, got $got"
# Command substitution is a subshell; source/PYTHONPATH are set on a direct call.
proof_select_harbor_agent >/dev/null
[ "$PROOF_HARBOR_AGENT_SOURCE" = "artifact_dir/agent" ] || fail "source $PROOF_HARBOR_AGENT_SOURCE"
pass "evaluate prefers \$PROOF_ARTIFACT_DIR/agent"

ONLY_RECIPE="$WORKDIR/only-recipe"
mkdir -p "$ONLY_RECIPE/recipe/agent"
cp -a "$FIXTURES/recipe/agent/." "$ONLY_RECIPE/recipe/agent/"
export PROOF_ARTIFACT_DIR="$ONLY_RECIPE"
got="$(proof_select_harbor_agent)" || fail "evaluate should select recipe/agent"
[ "$got" = "agent.agent:RecipeAgent" ] || fail "expected RecipeAgent, got $got"
proof_select_harbor_agent >/dev/null
[ "$PROOF_HARBOR_AGENT_SOURCE" = "artifact_dir/recipe/agent" ] || fail "source $PROOF_HARBOR_AGENT_SOURCE"
pass "evaluate falls through to recipe/agent"

CLASSIC="$WORKDIR/classic"
mkdir -p "$CLASSIC/recipe"
cp "$FIXTURES/recipe/run.sh" "$CLASSIC/recipe/run.sh"
export PROOF_ARTIFACT_DIR="$CLASSIC"
export PROOF_PARAM_HARBOR_AGENT=terminus-2
if (proof_select_harbor_agent) >"$WORKDIR/classic.out" 2>"$WORKDIR/classic.err"; then
    fail "evaluate with only recipe/run.sh must fail closed"
fi
grep -q "recipe/run.sh" "$WORKDIR/classic.err" || fail "error should name recipe/run.sh"
if grep -qx "terminus-2" "$WORKDIR/classic.out"; then
    fail "must not emit terminus-2 for classic recipe"
fi
pass "evaluate + recipe/run.sh fails closed (no topic agent)"

EMPTY_ART="$WORKDIR/empty-art"
mkdir -p "$EMPTY_ART"
export PROOF_ARTIFACT_DIR="$EMPTY_ART"
if (proof_select_harbor_agent) >/dev/null 2>"$WORKDIR/empty.err"; then
    fail "evaluate with empty artefact must not fall back to topic agent"
fi
grep -qi "refusing topic agent fallback\\|no Harbor agent" "$WORKDIR/empty.err" || fail "must explain the scoring gap"
pass "evaluate empty artefact refuses terminus-2 fallback"

unset PROOF_ARTIFACT_DIR
export PROOF_JOB=baseline
export PROOF_PARAM_HARBOR_AGENT=terminus-2
got="$(proof_select_harbor_agent)" || fail "baseline without artefact should use topic agent"
[ "$got" = "terminus-2" ] || fail "expected terminus-2, got $got"
proof_select_harbor_agent >/dev/null
[ "$PROOF_HARBOR_AGENT_SOURCE" = "topic" ] || fail "source should be topic"
pass "baseline without artefact uses topic agent"

# --- fake harbor end-to-end evaluate ---
FAKE_BIN="$WORKDIR/bin"
mkdir -p "$FAKE_BIN"
cat > "$FAKE_BIN/harbor" <<'EOF'
#!/bin/bash
set -euo pipefail
agent=""
path=""
jobs=""
while [ $# -gt 0 ]; do
    case "$1" in
        -a|--agent) agent="$2"; shift 2 ;;
        --path|-p) path="$2"; shift 2 ;;
        --jobs-dir) jobs="$2"; shift 2 ;;
        *) shift ;;
    esac
done
printf '%s\n' "$agent" > "${PROOF_WORK_DIR}/harbor.agent"
printf '%s\n' "$path" > "${PROOF_WORK_DIR}/harbor.path"
job="$jobs/job1/hello__1"
mkdir -p "$job"
cat > "$job/result.json" <<JSON
{"trial_name": "hello__1", "verifier_result": {"rewards": {"reward": 1.0}}}
JSON
EOF
chmod 0755 "$FAKE_BIN/harbor" "$ADAPTOR/run" "$ADAPTOR/inspect" "$ADAPTOR/harness/run-harbor"

export PATH="$FAKE_BIN:$PATH"
export PROOF_JOB=evaluate
export PROOF_ARTIFACT_DIR="$FIXTURES"
export PROOF_MODEL_PIN="moonshotai/kimi-k3"
export PROOF_PARAM_MINER_BYOK=OPENROUTER_API_KEY
printf 'miner-secret-key' > "$PROOF_MINER_ENV_DIR/OPENROUTER_API_KEY"
chmod 0600 "$PROOF_MINER_ENV_DIR/OPENROUTER_API_KEY"
unset OPENROUTER_API_KEY || true
PROOF_OUTPUT_DIR="$WORKDIR/out-eval"
mkdir -p "$PROOF_OUTPUT_DIR"
export PROOF_OUTPUT_DIR
"$ADAPTOR/harness/run-harbor"
[ -f "$PROOF_OUTPUT_DIR/report.json" ] || fail "evaluate must write report.json"
got_agent="$(cat "$PROOF_WORK_DIR/harbor.agent")"
[ "$got_agent" = "agent.agent:MinerAgent" ] || fail "harbor -a was $got_agent (miner artefact ignored)"
grep -q '"primary_value"' "$PROOF_OUTPUT_DIR/report.json" || fail "report.json missing primary_value"
python3 - "$PROOF_OUTPUT_DIR/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
assert r["primary_value"] == 1.0
assert r["evidence"]["agent"] == "agent.agent:MinerAgent"
assert "terminus-2" not in json.dumps(r)
PY
pass "evaluate run-harbor passes miner -a, not terminus-2"

# --- nonzero Harbor exit must not write a successful report ---
cat > "$FAKE_BIN/harbor" <<'EOF'
#!/bin/bash
set -euo pipefail
jobs=""
while [ $# -gt 0 ]; do
    case "$1" in
        --jobs-dir) jobs="$2"; shift 2 ;;
        *) shift ;;
    esac
done
job="$jobs/job1/hello__1"
mkdir -p "$job"
cat > "$job/result.json" <<JSON
{"trial_name": "hello__1", "verifier_result": {"rewards": {"reward": 0.6}}}
JSON
exit 23
EOF
chmod 0755 "$FAKE_BIN/harbor"
PARTIAL_OUT="$WORKDIR/out-partial"
mkdir -p "$PARTIAL_OUT"
export PROOF_OUTPUT_DIR="$PARTIAL_OUT"
export PROOF_JOB=evaluate
export PROOF_ARTIFACT_DIR="$FIXTURES"
if "$ADAPTOR/harness/run-harbor" >"$WORKDIR/partial.out" 2>"$WORKDIR/partial.err"; then
    fail "nonzero harbor exit must fail closed"
fi
[ ! -f "$PARTIAL_OUT/report.json" ] || fail "must not write report.json after harbor exit 23"
grep -qi "harbor exited 23\\|refusing to score" "$WORKDIR/partial.err" || fail "must name the harbor failure"
pass "nonzero harbor exit fails closed (no report)"

# --- import_path naming a module only on inherited PYTHONPATH ---
ESCAPE="$WORKDIR/escape"
mkdir -p "$ESCAPE/artifact/agent" "$ESCAPE/external"
printf 'outside_agent:ExternalAgent\n' > "$ESCAPE/artifact/agent/import_path"
cat > "$ESCAPE/external/outside_agent.py" <<'PY'
from harbor.agents.base import BaseAgent
class ExternalAgent(BaseAgent):
    pass
PY
export PROOF_JOB=evaluate
export PROOF_ARTIFACT_DIR="$ESCAPE/artifact"
export PYTHONPATH="$ESCAPE/external${PYTHONPATH:+:$PYTHONPATH}"
if (proof_select_harbor_agent) >"$WORKDIR/escape.out" 2>"$WORKDIR/escape.err"; then
    fail "import_path outside the artefact must fail before Harbor"
fi
if grep -q "outside_agent:ExternalAgent" "$WORKDIR/escape.out"; then
    fail "must not emit an escaped import path"
fi
pass "import_path outside artefact is refused before Harbor"

echo "all adaptor tests passed"
