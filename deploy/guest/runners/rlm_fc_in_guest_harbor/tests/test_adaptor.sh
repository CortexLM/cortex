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
mkdir -p "$PROOF_WORK_DIR" "$PROOF_OUTPUT_DIR" "$PROOF_PACK_DIR/tasks/cargo-flight-dispatch"
printf '[agent]\ntimeout_sec = 120\n' > "$PROOF_PACK_DIR/tasks/cargo-flight-dispatch/task.toml"
# x0017 hour-plus and broken-until-fixed names with no timeout must still drop.
for drop in \
    biped biped-contact-dynamics formal-crypto cad cad-model data-anon data-anonymization \
    batched-eval-parity ctr-optimization cumulative-layout-shift distributed-dedup coq-block-bound; do
    mkdir -p "$PROOF_PACK_DIR/tasks/$drop"
    printf '# leftover long or broken task\n' > "$PROOF_PACK_DIR/tasks/$drop/instruction.md"
done
export PROOF_PARAM_TASKS_DIR="tasks"

# --- tasks_dir ---
if (export PROOF_PARAM_TASKS_DIR=".."; proof_require_tasks) 2>/dev/null; then
    fail "tasks_dir=.. must be refused"
fi
export PROOF_PARAM_TASKS_DIR="tasks"
proof_require_tasks || fail "tasks_dir=tasks should work"
pass "tasks_dir relative ok, .. refused"

# --- duration filter drops ≥1h tasks ---
LONG="$PROOF_PACK_DIR/tasks/too-slow"
mkdir -p "$LONG"
printf '[agent]\ntimeout_sec = 7200\n' > "$LONG/task.toml"
proof_require_tasks
proof_filter_tasks || fail "filter should keep the short task"
[ -d "$PROOF_TASKS/cargo-flight-dispatch" ] || fail "allowlisted short task must be kept"
[ ! -d "$PROOF_TASKS/too-slow" ] || fail "≥1h task must be dropped"
[ ! -d "$PROOF_TASKS/biped" ] || fail "n15 biped must be dropped without timeout_sec"
[ ! -d "$PROOF_TASKS/cad-model" ] || fail "n15 cad-model must be dropped"
[ ! -d "$PROOF_TASKS/formal-crypto" ] || fail "n15 formal-crypto must be dropped"
[ ! -d "$PROOF_TASKS/data-anonymization" ] || fail "n15 data-anonymization must be dropped"
[ ! -d "$PROOF_TASKS/batched-eval-parity" ] || fail "broken batched-eval-parity must be dropped"
[ ! -d "$PROOF_TASKS/ctr-optimization" ] || fail "broken ctr-optimization must be dropped"
[ ! -d "$PROOF_TASKS/cumulative-layout-shift" ] || fail "broken cumulative-layout-shift must be dropped"
[ ! -d "$PROOF_TASKS/distributed-dedup" ] || fail "broken distributed-dedup must be dropped"
[ ! -d "$PROOF_TASKS/coq-block-bound" ] || fail "broken coq-block-bound must be dropped"
pass "default pack keeps x0017 allowlist and drops hour-plus plus broken"

# --- agent network rewrite ---
printf '[environment]\nnetwork_mode = "no-network"\n' > "$PROOF_TASKS/cargo-flight-dispatch/task.toml"
proof_enable_agent_network || fail "network rewrite should succeed"
grep -q 'network_mode = "public"' "$PROOF_TASKS/cargo-flight-dispatch/task.toml" || fail "agent network must be public"
if grep -q 'no-network' "$PROOF_TASKS/cargo-flight-dispatch/task.toml"; then
    fail "no-network must not remain on the filtered copy"
fi
pass "agent network rewritten to public (not no-network)"

# --- pytest in verifier / environment images ---
mkdir -p "$PROOF_TASKS/cargo-flight-dispatch/environment"
printf 'FROM python:3.12-slim\nWORKDIR /app\n' > "$PROOF_TASKS/cargo-flight-dispatch/environment/Dockerfile"
printf 'numpy\n' > "$PROOF_TASKS/cargo-flight-dispatch/environment/requirements.txt"
proof_ensure_verifier || fail "ensure_verifier should patch the filtered copy"
grep -q 'pip install --no-cache-dir pytest' "$PROOF_TASKS/cargo-flight-dispatch/environment/Dockerfile" \
    || fail "environment Dockerfile must install pytest"
grep -qx 'pytest' "$PROOF_TASKS/cargo-flight-dispatch/environment/requirements.txt" \
    || fail "environment requirements.txt must list pytest"
pass "verifier/environment images gain pytest (n15 biped+cad hole)"

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
[ "$PROOF_HARNESS_KIND" = "harbor" ] || fail "kind $PROOF_HARNESS_KIND"
pass "evaluate prefers \$PROOF_ARTIFACT_DIR/agent"

PY_ART="$FIXTURES/python_agent"
export PROOF_ARTIFACT_DIR="$PY_ART"
got="$(proof_select_harbor_agent)" || fail "evaluate should select custom Python agent"
[ "$got" = "proof_python_agent:ProofPythonAgent" ] || fail "expected wrapper -a, got $got"
proof_select_harbor_agent >/dev/null
[ "$PROOF_HARNESS_KIND" = "python" ] || fail "kind $PROOF_HARNESS_KIND"
[ "$PROOF_MINER_AGENT_IMPORT" = "agent.agent:Agent" ] || fail "import $PROOF_MINER_AGENT_IMPORT"
pass "evaluate custom Python uses wrapper, not terminus-2"

JSON_ART="$FIXTURES/harness_json"
export PROOF_ARTIFACT_DIR="$JSON_ART"
proof_select_harbor_agent >/dev/null || fail "evaluate should honour harness.json"
[ "$PROOF_HARNESS_KIND" = "python" ] || fail "harness.json kind $PROOF_HARNESS_KIND"
pass "evaluate harness.json selects custom Python"

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
got="$(proof_select_harbor_agent)" || fail "evaluate with recipe/run.sh must accept script harness"
proof_select_harbor_agent >/dev/null
[ "$PROOF_HARNESS_KIND" = "script" ] || fail "classic recipe should be script, got $PROOF_HARNESS_KIND"
[ "$PROOF_HARNESS_ENTRY" = "recipe/run.sh" ] || fail "entry $PROOF_HARNESS_ENTRY"
[ -z "$got" ] || fail "script harness must not emit a Harbor -a, got $got"
if [ "${PROOF_HARBOR_AGENT_ARG:-}" = "terminus-2" ]; then
    fail "must not wrap classic recipe as terminus-2"
fi
pass "evaluate + recipe/run.sh is a script harness (no terminus-2)"

EMPTY_ART="$WORKDIR/empty-art"
mkdir -p "$EMPTY_ART"
export PROOF_ARTIFACT_DIR="$EMPTY_ART"
if (proof_select_harbor_agent) >/dev/null 2>"$WORKDIR/empty.err"; then
    fail "evaluate with empty artefact must not fall back to topic agent"
fi
grep -qi "refusing topic\\|no custom Python\\|no Harbor agent\\|no harness" "$WORKDIR/empty.err" || fail "must explain the scoring gap"
pass "evaluate empty artefact refuses terminus-2 fallback"

unset PROOF_ARTIFACT_DIR
export PROOF_JOB=baseline
export PROOF_PARAM_HARBOR_AGENT=terminus-2
got="$(proof_select_harbor_agent)" || fail "baseline without artefact should use topic agent"
[ "$got" = "terminus-2" ] || fail "expected terminus-2, got $got"
proof_select_harbor_agent >/dev/null
[ "$PROOF_HARBOR_AGENT_SOURCE" = "topic" ] || fail "source should be topic"
[ "$PROOF_HARNESS_KIND" = "builtin" ] || fail "kind $PROOF_HARNESS_KIND"
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
env=""
while [ $# -gt 0 ]; do
    case "$1" in
        -a|--agent) agent="$2"; shift 2 ;;
        --path|-p) path="$2"; shift 2 ;;
        --jobs-dir) jobs="$2"; shift 2 ;;
        --env) env="$2"; shift 2 ;;
        *) shift ;;
    esac
done
printf '%s\n' "$agent" > "${PROOF_WORK_DIR}/harbor.agent"
printf '%s\n' "$path" > "${PROOF_WORK_DIR}/harbor.path"
printf '%s\n' "$env" > "${PROOF_WORK_DIR}/harbor.env"
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
got_env="$(cat "$PROOF_WORK_DIR/harbor.env")"
[ "$got_env" = "docker" ] || fail "harbor --env was $got_env (want docker, not no-network)"
if grep -q 'no-network' "$PROOF_WORK_DIR/harbor.env"; then
    fail "must not pass no-network to Harbor docker env"
fi
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

# --- script harness refuses miner-authored report.json ---
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
SCRIPT_ART="$WORKDIR/script-self-score"
mkdir -p "$SCRIPT_ART/recipe"
cat > "$SCRIPT_ART/recipe/run.sh" <<'EOF'
#!/bin/bash
cat > "$PROOF_OUTPUT_DIR/report.json" <<JSON
{"primary_value": 999999.25, "claim_holds": true}
JSON
exit 0
EOF
chmod 0755 "$SCRIPT_ART/recipe/run.sh"
export PROOF_JOB=evaluate
export PROOF_ARTIFACT_DIR="$SCRIPT_ART"
SELF_OUT="$WORKDIR/out-self-score"
mkdir -p "$SELF_OUT"
export PROOF_OUTPUT_DIR="$SELF_OUT"
if "$ADAPTOR/harness/run-harbor" >"$WORKDIR/self.out" 2>"$WORKDIR/self.err"; then
    fail "miner-authored report.json must fail closed"
fi
[ ! -f "$SELF_OUT/report.json" ] || fail "must not keep miner-authored report.json"
grep -qi "miner-authored\\|refusing miner-authored primary_value" "$WORKDIR/self.err" \
    || fail "must name the self-report refusal"
pass "script harness refuses miner-authored primary_value"

# --- script harness score comes only from Harbor verifier trials ---
SCRIPT_JOBS="$WORKDIR/script-jobs"
mkdir -p "$SCRIPT_JOBS/recipe"
cat > "$SCRIPT_JOBS/recipe/run.sh" <<'EOF'
#!/bin/bash
job="$PROOF_WORK_DIR/harbor-jobs/job1/hello__1"
mkdir -p "$job"
cat > "$job/result.json" <<JSON
{"trial_name": "hello__1", "verifier_result": {"rewards": {"reward": 0.5}}}
JSON
exit 0
EOF
chmod 0755 "$SCRIPT_JOBS/recipe/run.sh"
export PROOF_ARTIFACT_DIR="$SCRIPT_JOBS"
JOBS_OUT="$WORKDIR/out-script-jobs"
mkdir -p "$JOBS_OUT"
export PROOF_OUTPUT_DIR="$JOBS_OUT"
"$ADAPTOR/harness/run-harbor" || fail "script that writes Harbor jobs must summarize"
python3 - "$JOBS_OUT/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
assert r["primary_value"] == 0.5, r
assert r["evidence"]["harness_kind"] == "script"
PY
pass "script harness primary_value is Harbor verifier reward"

# --- script that self-reports AND writes jobs still fails closed ---
SCRIPT_BOTH="$WORKDIR/script-both"
mkdir -p "$SCRIPT_BOTH/recipe"
cat > "$SCRIPT_BOTH/recipe/run.sh" <<'EOF'
#!/bin/bash
job="$PROOF_WORK_DIR/harbor-jobs/job1/hello__1"
mkdir -p "$job"
cat > "$job/result.json" <<JSON
{"trial_name": "hello__1", "verifier_result": {"rewards": {"reward": 1.0}}}
JSON
cat > "$PROOF_OUTPUT_DIR/report.json" <<JSON
{"primary_value": 999999.25, "claim_holds": true}
JSON
exit 0
EOF
chmod 0755 "$SCRIPT_BOTH/recipe/run.sh"
export PROOF_ARTIFACT_DIR="$SCRIPT_BOTH"
BOTH_OUT="$WORKDIR/out-script-both"
mkdir -p "$BOTH_OUT"
export PROOF_OUTPUT_DIR="$BOTH_OUT"
if "$ADAPTOR/harness/run-harbor" >"$WORKDIR/both.out" 2>"$WORKDIR/both.err"; then
    fail "self-report must fail even when Harbor jobs exist"
fi
[ ! -f "$BOTH_OUT/report.json" ] || fail "must not keep report.json after self-report"
pass "script self-report fails closed even with Harbor jobs"

echo "all adaptor tests passed"
