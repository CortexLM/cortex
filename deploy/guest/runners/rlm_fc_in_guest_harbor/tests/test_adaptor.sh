#!/bin/bash
# Unit tests for lib.sh + run-harbor without a real Harbor install.
set -euo pipefail
ADAPTOR="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=../lib.sh
. "$ADAPTOR/lib.sh"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }

# --- OpenRouter CSV trim (canon miner_byok whitespace) ---
proof_csv_has_openrouter "OPENROUTER_API_KEY" || fail "bare OPENROUTER_API_KEY"
proof_csv_has_openrouter "OTHER_KEY, OPENROUTER_API_KEY" || fail "space after comma must still match"
proof_csv_has_openrouter " OPENROUTER_API_KEY " || fail "padded name must match"
proof_csv_has_openrouter "OTHER_KEY" && fail "OTHER_KEY is not OpenRouter"
proof_csv_has_openrouter "" && fail "empty is not OpenRouter"
pass "proof_csv_has_openrouter trims comma-list names"

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/harbor-adaptor-XXXXXX")"
cleanup() { rm -rf "$WORKDIR"; }
trap cleanup EXIT

export PROOF_WORK_DIR="$WORKDIR/work"
export PROOF_OUTPUT_DIR="$WORKDIR/out"
export PROOF_PACK_DIR="$WORKDIR/pack"
export PROOF_HARNESS_SKIP_PODMAN=1
mkdir -p "$PROOF_WORK_DIR" "$PROOF_OUTPUT_DIR" "$PROOF_PACK_DIR/tasks/task-alpha"
printf '[agent]\ntimeout_sec = 120\n' > "$PROOF_PACK_DIR/tasks/task-alpha/task.toml"
# Neutral fixture names: nothing here is a benchmark task list.
for name in too-slow task-excluded task-topic-excluded; do
    mkdir -p "$PROOF_PACK_DIR/tasks/$name"
    printf '# fixture task\n' > "$PROOF_PACK_DIR/tasks/$name/instruction.md"
done
printf '[task]\nestimated_duration_s = 7200\n' > "$PROOF_PACK_DIR/tasks/too-slow/task.toml"
printf '{"deny": ["task-excluded"], "slices": {"quick": ["task-alpha"]}}\n' > "$PROOF_PACK_DIR/filter.json"
export PROOF_PARAM_TASKS_DIR="tasks"

# --- tasks_dir ---
if (export PROOF_PARAM_TASKS_DIR=".."; proof_require_tasks) 2>/dev/null; then
    fail "tasks_dir=.. must be refused"
fi
export PROOF_PARAM_TASKS_DIR="tasks"
proof_require_tasks || fail "tasks_dir=tasks should work"
pass "tasks_dir relative ok, .. refused"

# --- silent topic: every task minus pack deny / params.task_exclude; no gate ---
export PROOF_PARAM_TASK_EXCLUDE="task-topic-excluded"
proof_require_tasks
proof_filter_tasks || fail "silent topic must select the pack"
[ -d "$PROOF_TASKS/task-alpha" ] || fail "task-alpha must be kept"
[ -d "$PROOF_TASKS/too-slow" ] || fail "no duration gate unless the topic sets one"
[ ! -d "$PROOF_TASKS/task-excluded" ] || fail "pack filter.json deny must drop task-excluded"
[ ! -d "$PROOF_TASKS/task-topic-excluded" ] || fail "params.task_exclude must drop task-topic-excluded"
grep -q '"source": "tasks_dir"' "$PROOF_TASKS/.proof-task-filter.json" || fail "summary must name the source"
pass "silent topic keeps every task minus deny/exclude, no duration gate"

# --- duration gate only from the topic (or the pack's own ceiling) ---
export PROOF_PARAM_MAX_TASK_DURATION_S=3600
proof_require_tasks
proof_filter_tasks || fail "gate should keep the short task"
[ -d "$PROOF_TASKS/task-alpha" ] || fail "task with only a timeout ceiling must be kept (timeouts are not durations)"
[ ! -d "$PROOF_TASKS/too-slow" ] || fail "known duration >= ceiling must be dropped"
unset PROOF_PARAM_MAX_TASK_DURATION_S
pass "max_task_duration_s gates known durations only"

# --- explicit tasks: smoke shape; unknown name fails closed ---
export PROOF_PARAM_TASKS="task-alpha"
proof_require_tasks
proof_filter_tasks || fail "params.tasks=task-alpha must select"
[ "$(ls -1 "$PROOF_TASKS" | grep -vc '^\.')" = "1" ] || fail "params.tasks must keep exactly the named task"
if (export PROOF_PARAM_TASKS="task-alpha,task-missing"; proof_require_tasks; proof_filter_tasks) 2>/dev/null; then
    fail "a named task absent from the pack must fail closed"
fi
if (export PROOF_PARAM_TASKS="task-topic-excluded"; proof_require_tasks; proof_filter_tasks) 2>/dev/null; then
    fail "a task both selected and excluded must fail closed"
fi
unset PROOF_PARAM_TASKS
pass "params.tasks selects exactly the named tasks (single-task smoke) and fails on unknown names"

# --- n_tasks=1 smoke ---
export PROOF_PARAM_N_TASKS=1
proof_require_tasks
proof_filter_tasks || fail "n_tasks=1 must select"
[ "$(ls -1 "$PROOF_TASKS" | grep -vc '^\.')" = "1" ] || fail "n_tasks=1 must keep one task"
unset PROOF_PARAM_N_TASKS
pass "n_tasks=1 keeps the first selected task"

# --- task_slice via the pack's slices ---
export PROOF_TASK_SLICE=quick
proof_require_tasks
proof_filter_tasks || fail "pack slice quick must select"
[ -d "$PROOF_TASKS/task-alpha" ] || fail "slice must keep task-alpha"
[ ! -d "$PROOF_TASKS/too-slow" ] || fail "slice must not keep too-slow"
grep -q '"task_slice_resolved": true' "$PROOF_TASKS/.proof-task-filter.json" || fail "summary must record the resolved slice"
if (export PROOF_TASK_SLICE=nope; proof_require_tasks; proof_filter_tasks) 2>/dev/null; then
    fail "a slice the pack does not define must fail closed"
fi
unset PROOF_TASK_SLICE
pass "task_slice resolves through the pack; an unknown label fails closed"

# --- exec timeout export: topic data only ---
unset PROOF_PARAM_EXEC_TIMEOUT_S || true
proof_export_exec_timeout
[ -z "${PROOF_EXEC_TIMEOUT_S:-}" ] || fail "no exec_timeout_s → PROOF_EXEC_TIMEOUT_S must stay unset"
export PROOF_PARAM_EXEC_TIMEOUT_S=900
proof_export_exec_timeout
[ "${PROOF_EXEC_TIMEOUT_S:-}" = "900" ] || fail "exec_timeout_s must export PROOF_EXEC_TIMEOUT_S"
if (export PROOF_PARAM_EXEC_TIMEOUT_S=soon; proof_export_exec_timeout) 2>/dev/null; then
    fail "a non-integer exec_timeout_s must fail closed"
fi
unset PROOF_PARAM_EXEC_TIMEOUT_S PROOF_EXEC_TIMEOUT_S
pass "exec_timeout_s → PROOF_EXEC_TIMEOUT_S only when the topic signs it"

# --- Harbor timeout multipliers: flags only when signed ---
flags=()
proof_harbor_timeout_flags flags
[ "${#flags[@]}" -eq 0 ] || fail "silent topic must pass no timeout flag"
export PROOF_PARAM_TIMEOUT_MULTIPLIER=1.5
export PROOF_PARAM_VERIFIER_TIMEOUT_MULTIPLIER=2
proof_harbor_timeout_flags flags
[ "${flags[*]}" = "--timeout-multiplier 1.5 --verifier-timeout-multiplier 2" ] || fail "timeout flags were: ${flags[*]}"
if (export PROOF_PARAM_AGENT_TIMEOUT_MULTIPLIER=fast; f=(); proof_harbor_timeout_flags f) 2>/dev/null; then
    fail "a non-numeric multiplier must fail closed"
fi
if (export PROOF_PARAM_TIMEOUT_MULTIPLIER=0; f=(); proof_harbor_timeout_flags f) 2>/dev/null; then
    fail "a zero multiplier must fail closed"
fi
unset PROOF_PARAM_TIMEOUT_MULTIPLIER PROOF_PARAM_VERIFIER_TIMEOUT_MULTIPLIER
pass "timeout multipliers reach Harbor only when the topic signs them"

# Leave the filtered set at the single task the later fixtures use.
export PROOF_PARAM_TASKS="task-alpha"
proof_require_tasks
proof_filter_tasks || fail "restore the single-task selection for later adaptor tests"

# --- agent network rewrite ---
printf '[environment]\nnetwork_mode = "no-network"\n' > "$PROOF_TASKS/task-alpha/task.toml"
proof_enable_agent_network || fail "network rewrite should succeed"
grep -q 'network_mode = "public"' "$PROOF_TASKS/task-alpha/task.toml" || fail "agent network must be public"
if grep -q 'no-network' "$PROOF_TASKS/task-alpha/task.toml"; then
    fail "no-network must not remain on the filtered copy"
fi
pass "agent network rewritten to public (not no-network)"

# --- pytest in verifier / environment images ---
mkdir -p "$PROOF_TASKS/task-alpha/environment"
printf 'FROM python:3.12-slim\nWORKDIR /app\n' > "$PROOF_TASKS/task-alpha/environment/Dockerfile"
printf 'numpy\n' > "$PROOF_TASKS/task-alpha/environment/requirements.txt"
proof_ensure_verifier || fail "ensure_verifier should patch the filtered copy"
grep -q 'pip install --no-cache-dir pytest' "$PROOF_TASKS/task-alpha/environment/Dockerfile" \
    || fail "environment Dockerfile must install pytest"
grep -qx 'pytest' "$PROOF_TASKS/task-alpha/environment/requirements.txt" \
    || fail "environment requirements.txt must list pytest"
pass "verifier/environment images gain pytest (verifier image hole)"

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

# --- evaluate with DIR unset stages from exported env (not "DIR is unset") ---
unset PROOF_MINER_ENV_DIR || true
unset OPENROUTER_API_KEY || true
export OPENROUTER_API_KEY="exported-miner-key"
proof_load_inference_key || fail "evaluate should stage BYOK from the exported env var"
[ "${OPENROUTER_API_KEY:-}" = "exported-miner-key" ] || fail "evaluate did not keep the exported key"
[ -n "${PROOF_MINER_ENV_DIR:-}" ] || fail "evaluate must set PROOF_MINER_ENV_DIR after staging"
[ -r "$PROOF_MINER_ENV_DIR/OPENROUTER_API_KEY" ] || fail "evaluate must write the staged file"
pass "evaluate unset DIR stages from exported OPENROUTER_API_KEY"

# --- evaluate with DIR unset and no key fails on missing key, not unset DIR ---
unset PROOF_MINER_ENV_DIR || true
unset OPENROUTER_API_KEY || true
rm -f "$PROOF_SECRETS_DIR/miner/OPENROUTER_API_KEY" "$PROOF_WORK_DIR/miner-env/OPENROUTER_API_KEY"
if err="$(proof_load_inference_key 2>&1)"; then
    fail "evaluate without a key must fail closed"
fi
echo "$err" | grep -q "PROOF_MINER_ENV_DIR is unset" && fail "must not fail because DIR was unset: $err"
echo "$err" | grep -q "miner did not supply OPENROUTER_API_KEY" || fail "must name the missing key: $err"
pass "evaluate missing key fails closed after staging (not unset DIR)"

# Restore the dir later tests write into.
export PROOF_MINER_ENV_DIR="$WORKDIR/miner-env"
mkdir -p "$PROOF_MINER_ENV_DIR"

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
model=""
while [ $# -gt 0 ]; do
    case "$1" in
        -a|--agent) agent="$2"; shift 2 ;;
        --path|-p) path="$2"; shift 2 ;;
        --jobs-dir) jobs="$2"; shift 2 ;;
        --env) env="$2"; shift 2 ;;
        -m|--model) model="$2"; shift 2 ;;
        *) shift ;;
    esac
done
printf '%s\n' "$agent" > "${PROOF_WORK_DIR}/harbor.agent"
printf '%s\n' "$path" > "${PROOF_WORK_DIR}/harbor.path"
printf '%s\n' "$env" > "${PROOF_WORK_DIR}/harbor.env"
printf '%s\n' "$model" > "${PROOF_WORK_DIR}/harbor.model"
job="$jobs/job1/task-alpha__1"
mkdir -p "$job/verifier"
cat > "$job/result.json" <<JSON
{"trial_name": "task-alpha__1", "verifier_result": {"rewards": {"reward": 1.0}}}
JSON
printf '1.0\n' > "$job/verifier/reward.txt"
EOF
chmod 0755 "$FAKE_BIN/harbor" "$ADAPTOR/run" "$ADAPTOR/inspect" "$ADAPTOR/harness/run-harbor"

export PATH="$FAKE_BIN:$PATH"
export PROOF_JOB=evaluate
export PROOF_ARTIFACT_DIR="$FIXTURES"
export PROOF_MODEL_PIN="moonshotai/kimi-k3"
export PROOF_PARAM_MODEL="openrouter/moonshotai/kimi-k3"
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
got_model="$(cat "$PROOF_WORK_DIR/harbor.model")"
[ "$got_model" = "openrouter/moonshotai/kimi-k3" ] || fail "harbor -m stripped OpenRouter prefix: $got_model"
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
pass "evaluate run-harbor passes miner -a and full OpenRouter -m"

# OpenRouter + vendor/model pin only (no params.model) fails closed.
unset PROOF_PARAM_MODEL || true
export PROOF_MODEL_PIN="moonshotai/kimi-k3"
export PROOF_PARAM_MINER_BYOK=OPENROUTER_API_KEY
if "$ADAPTOR/harness/run-harbor" 2>"$WORKDIR/openrouter-pin.err"; then
    fail "OpenRouter with vendor/model pin must fail closed"
fi
grep -q "provider prefix" "$WORKDIR/openrouter-pin.err" \
    || fail "must name provider prefix: $(cat "$WORKDIR/openrouter-pin.err")"
export PROOF_PARAM_MODEL="openrouter/moonshotai/kimi-k3"
pass "OpenRouter Harbor -m without provider prefix fails closed"

# miner_byok of another name must not skip the prefix guard when
# inference_key_env is OPENROUTER_API_KEY.
unset PROOF_PARAM_MODEL || true
export PROOF_MODEL_PIN="moonshotai/kimi-k3"
export PROOF_PARAM_MINER_BYOK=OTHER_KEY
export PROOF_PARAM_INFERENCE_KEY_ENV=OPENROUTER_API_KEY
printf 'other-key' > "$PROOF_MINER_ENV_DIR/OTHER_KEY"
chmod 0600 "$PROOF_MINER_ENV_DIR/OTHER_KEY"
if "$ADAPTOR/harness/run-harbor" 2>"$WORKDIR/openrouter-env.err"; then
    fail "OpenRouter inference_key_env with vendor/model pin must fail closed"
fi
grep -q "provider prefix" "$WORKDIR/openrouter-env.err" \
    || fail "must name provider prefix when only inference_key_env is OpenRouter: $(cat "$WORKDIR/openrouter-env.err")"
export PROOF_PARAM_MODEL="openrouter/moonshotai/kimi-k3"
export PROOF_PARAM_MINER_BYOK=OPENROUTER_API_KEY
pass "OpenRouter prefix guard inspects inference_key_env even when miner_byok is other"

# --- persist work helper (retain-on-fail durability) ---
proof_persist_work || fail "proof_persist_work must succeed on a writable work dir"
pass "proof_persist_work flushes a real work dir"

# --- harbor log tail helper ---
TAIL_LOG="$PROOF_WORK_DIR/harbor.run.log"
: > "$TAIL_LOG"
i=0
while [ "$i" -lt 100 ]; do
    echo "line-$i" >> "$TAIL_LOG"
    i=$((i + 1))
done
tail_out="$(proof_harbor_log_tail "$TAIL_LOG")"
echo "$tail_out" | grep -qx "line-99" || fail "log tail must include the last line"
echo "$tail_out" | grep -qx "line-0" && fail "log tail must drop lines older than 80"
echo "$tail_out" | grep -qx "line-19" && fail "log tail of 80 from 100 must start at line-20"
echo "$tail_out" | grep -qx "line-20" || fail "log tail of 80 from 100 must include line-20"
pass "proof_harbor_log_tail keeps the last 80 lines"

set +e
(proof_die_harbor 7 "$TAIL_LOG") 2>"$WORKDIR/die.err"
die_rc=$?
set -e
[ "$die_rc" -eq 2 ] || fail "proof_die_harbor exit $die_rc want 2"
grep -q "harbor exited 7" "$WORKDIR/die.err" || fail "must name harbor exit"
grep -q "line-99" "$WORKDIR/die.err" || fail "proof_die_harbor must print log tail"
pass "proof_die_harbor prints harbor.run.log tail on stderr"

# --- nonzero Harbor exit still scores already-measured trials ---
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
job="$jobs/job1/task-alpha__1"
mkdir -p "$job/verifier"
cat > "$job/result.json" <<JSON
{"trial_name": "task-alpha__1", "verifier_result": {"rewards": {"reward": 0.6}}}
JSON
printf '0.6\n' > "$job/verifier/reward.txt"
echo "harbor: timeout after deadline"
exit 23
EOF
chmod 0755 "$FAKE_BIN/harbor"
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
PARTIAL_OUT="$WORKDIR/out-partial"
mkdir -p "$PARTIAL_OUT"
export PROOF_OUTPUT_DIR="$PARTIAL_OUT"
export PROOF_JOB=evaluate
export PROOF_ARTIFACT_DIR="$FIXTURES"
"$ADAPTOR/harness/run-harbor" >"$WORKDIR/partial.out" 2>"$WORKDIR/partial.err" \
    || fail "nonzero harbor with measured trials must still summarize"
python3 - "$PARTIAL_OUT/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
assert r["primary_value"] == 0.6, r
assert r["evidence"]["n_measured"] == 1
assert r["evidence"]["harbor_exit"] == 23
assert r["evidence"]["harbor_incomplete"] is True
PY
grep -qi "harbor exited 23" "$WORKDIR/partial.err" || fail "must name the harbor exit"
grep -qi "scored already-measured" "$WORKDIR/partial.err" || fail "must say measured trials scored"
pass "nonzero harbor exit scores already-measured trials"

# --- unfinished Harbor snapshot + reward.txt-only is fail-closed ---
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
job="$jobs/job1"
mkdir -p "$job/task-alpha__1/verifier"
cat > "$job/result.json" <<JSON
{"finished_at": null, "n_running": 1, "n_completed": 1}
JSON
printf '1.0\n' > "$job/task-alpha__1/verifier/reward.txt"
exit 143
EOF
chmod 0755 "$FAKE_BIN/harbor"
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
TXT_OUT="$WORKDIR/out-reward-txt"
mkdir -p "$TXT_OUT"
export PROOF_OUTPUT_DIR="$TXT_OUT"
if "$ADAPTOR/harness/run-harbor" >"$WORKDIR/txt-only.out" 2>"$WORKDIR/txt-only.err"; then
    fail "reward.txt-only / unfinished snapshot must fail closed"
fi
[ ! -f "$TXT_OUT/report.json" ] || fail "must not publish from unfinished Harbor snapshot"
pass "incomplete Harbor (stale snapshot / reward.txt-only) fails closed"

# --- nonzero Harbor with zero measured trials stays fail-closed ---
cat > "$FAKE_BIN/harbor" <<'EOF'
#!/bin/bash
echo "TypeError: Can't instantiate abstract class ProofPythonAgent without an implementation for abstract method 'setup'"
exit 23
EOF
chmod 0755 "$FAKE_BIN/harbor"
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
EMPTY_OUT="$WORKDIR/out-empty-nonzero"
mkdir -p "$EMPTY_OUT"
export PROOF_OUTPUT_DIR="$EMPTY_OUT"
if "$ADAPTOR/harness/run-harbor" >"$WORKDIR/empty-nz.out" 2>"$WORKDIR/empty-nz.err"; then
    fail "nonzero harbor with no trials must fail closed"
fi
[ ! -f "$EMPTY_OUT/report.json" ] || fail "must not write report.json when n_measured=0"
grep -qi "harbor exited 23" "$WORKDIR/empty-nz.err" || fail "must name the harbor failure"
grep -qi "Can't instantiate abstract class ProofPythonAgent" "$WORKDIR/empty-nz.err" \
    || fail "503 stderr must carry harbor.run.log tail (setup TypeError)"
grep -qi "harbor.run.log" "$WORKDIR/empty-nz.err" || fail "must label the log tail"
pass "nonzero harbor with n_measured=0 fails closed and emits log tail"

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

# --- script harness refuses miner-authored report.json (no Harbor jobs) ---
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
    fail "miner-authored report.json with n_measured=0 must fail closed"
fi
[ ! -f "$SELF_OUT/report.json" ] || fail "must not keep miner-authored report.json"
grep -qi "miner-authored\\|no measured Harbor trials" "$WORKDIR/self.err" \
    || fail "must name the self-report refusal or empty measurement"
pass "script harness refuses miner-authored primary_value when n_measured=0"

# --- script harness score comes only from Harbor verifier trials ---
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
SCRIPT_JOBS="$WORKDIR/script-jobs"
mkdir -p "$SCRIPT_JOBS/recipe"
cat > "$SCRIPT_JOBS/recipe/run.sh" <<'EOF'
#!/bin/bash
job="$PROOF_WORK_DIR/harbor-jobs/job1/task-alpha__1"
mkdir -p "$job/verifier"
cat > "$job/result.json" <<JSON
{"trial_name": "task-alpha__1", "verifier_result": {"rewards": {"reward": 0.5}}}
JSON
printf '0.5\n' > "$job/verifier/reward.txt"
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

# --- self-report is ignored; Harbor trials still score ---
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
SCRIPT_BOTH="$WORKDIR/script-both"
mkdir -p "$SCRIPT_BOTH/recipe"
cat > "$SCRIPT_BOTH/recipe/run.sh" <<'EOF'
#!/bin/bash
job="$PROOF_WORK_DIR/harbor-jobs/job1/task-alpha__1"
mkdir -p "$job/verifier"
cat > "$job/result.json" <<JSON
{"trial_name": "task-alpha__1", "verifier_result": {"rewards": {"reward": 1.0}}}
JSON
printf '1.0\n' > "$job/verifier/reward.txt"
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
"$ADAPTOR/harness/run-harbor" >"$WORKDIR/both.out" 2>"$WORKDIR/both.err" \
    || fail "Harbor trials must still score after discarding miner report.json"
python3 - "$BOTH_OUT/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
assert r["primary_value"] == 1.0, r
assert r["primary_value"] != 999999.25
assert r["evidence"]["n_measured"] == 1
PY
grep -qi "discarded miner-authored" "$WORKDIR/both.err" \
    || fail "must log that miner report.json was discarded"
pass "script self-report is ignored; Harbor verifier reward is the score"

# --- x0020: Harbor finished; miner postamble TypeError must not discard trials ---
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
SCRIPT_POSTAMBLE="$WORKDIR/script-postamble"
mkdir -p "$SCRIPT_POSTAMBLE/recipe"
cat > "$SCRIPT_POSTAMBLE/recipe/run.sh" <<'EOF'
#!/bin/bash
# Harbor finished; postamble then raises (retained x0020).
i=1
while [ "$i" -le 8 ]; do
    job="$PROOF_WORK_DIR/harbor-jobs/job1/task-alpha__${i}"
    mkdir -p "$job/verifier"
    if [ "$i" -eq 1 ]; then
        reward=1.0
    else
        reward=0.0
    fi
    cat > "$job/result.json" <<JSON
{"trial_name": "task-alpha__${i}", "verifier_result": {"rewards": {"reward": ${reward}}}}
JSON
    printf '%s\n' "$reward" > "$job/verifier/reward.txt"
    i=$((i + 1))
done
python3 -c 'raise TypeError("postamble")'
EOF
chmod 0755 "$SCRIPT_POSTAMBLE/recipe/run.sh"
export PROOF_ARTIFACT_DIR="$SCRIPT_POSTAMBLE"
POST_OUT="$WORKDIR/out-script-postamble"
mkdir -p "$POST_OUT"
export PROOF_OUTPUT_DIR="$POST_OUT"
"$ADAPTOR/harness/run-harbor" >"$WORKDIR/postamble.out" 2>"$WORKDIR/postamble.err" \
    || fail "postamble TypeError must not discard already-measured Harbor trials"
python3 - "$POST_OUT/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
assert r["evidence"]["n_measured"] == 8, r
assert abs(r["primary_value"] - 0.125) < 1e-9, r
assert r["evidence"]["harbor_exit"] != 0
PY
grep -qi "scored already-measured" "$WORKDIR/postamble.err" \
    || fail "must say measured trials scored after script exit"
pass "x0020 postamble TypeError still scores n_measured=8 (mean 0.125)"

# --- script harness evaluates only filtered PROOF_TASKS ---
# Reset a recording harbor so --path rewrite is visible.
cat > "$FAKE_BIN/harbor" <<'EOF'
#!/bin/bash
set -euo pipefail
path=""
while [ $# -gt 0 ]; do
    case "$1" in
        --path|-p) path="$2"; shift 2 ;;
        *) shift ;;
    esac
done
printf '%s\n' "$path" > "${PROOF_WORK_DIR}/script.harbor-path"
# Miner asked for the pack path; wrapper must have rewritten it.
ls "$path" > "${PROOF_WORK_DIR}/script.harbor-ls" || true
exit 0
EOF
chmod 0755 "$FAKE_BIN/harbor"
SCRIPT_FILTER="$WORKDIR/script-filter"
mkdir -p "$SCRIPT_FILTER/recipe"
cat > "$SCRIPT_FILTER/recipe/run.sh" <<'EOF'
#!/bin/bash
set -euo pipefail
ls "$PROOF_PACK_DIR/$PROOF_PARAM_TASKS_DIR" > "$PROOF_WORK_DIR/script.pack-tasks"
ls "$PROOF_TASKS" > "$PROOF_WORK_DIR/script.proof-tasks"
if [ -L "$PROOF_PACK_DIR/$PROOF_PARAM_TASKS_DIR" ]; then
    echo SYMLINK > "$PROOF_WORK_DIR/script.tasks-link"
else
    echo COPY > "$PROOF_WORK_DIR/script.tasks-link"
fi
printf '%s\n' "${PROOF_HARBOR_REAL-UNSET}" > "$PROOF_WORK_DIR/script.harbor-real"
ls -1 "$PROOF_PACK_DIR" > "$PROOF_WORK_DIR/script.pack-root"
# Bypass: miners historically pass the unfiltered pack tasks dir.
harbor run --path "$PROOF_PACK_DIR/$PROOF_PARAM_TASKS_DIR" --jobs-dir "$PROOF_WORK_DIR/harbor-jobs" -a x --env docker --yes
# Write a complete filtered trial so summarize succeeds.
job="$PROOF_WORK_DIR/harbor-jobs/job1/task-alpha__1"
mkdir -p "$job/verifier"
cat > "$job/result.json" <<JSON
{"trial_name": "task-alpha__1", "verifier_result": {"rewards": {"reward": 0.25}}}
JSON
printf '0.25\n' > "$job/verifier/reward.txt"
exit 0
EOF
chmod 0755 "$SCRIPT_FILTER/recipe/run.sh"
# Pack-view is only the filtered tasks dir (no original-pack siblings).
mkdir -p "$PROOF_PACK_DIR/unfiltered-original"
printf 'unfiltered\n' > "$PROOF_PACK_DIR/unfiltered-original/hint"
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
export PROOF_ARTIFACT_DIR="$SCRIPT_FILTER"
FILTER_OUT="$WORKDIR/out-script-filter"
mkdir -p "$FILTER_OUT"
export PROOF_OUTPUT_DIR="$FILTER_OUT"
"$ADAPTOR/harness/run-harbor" || fail "script harness with filtered tasks must summarize"
# Filtered set from this fixture (params.tasks): task-alpha only.
grep -qx "task-alpha" "$PROOF_WORK_DIR/script.proof-tasks" \
    || fail "PROOF_TASKS must contain the selected task"
if grep -qx "too-slow" "$PROOF_WORK_DIR/script.proof-tasks" \
    || grep -qx "task-excluded" "$PROOF_WORK_DIR/script.proof-tasks"; then
    fail "PROOF_TASKS must not include filtered-out pack tasks"
fi
# Pack view: $PROOF_PACK_DIR/tasks is the same filtered tree.
grep -qx "task-alpha" "$PROOF_WORK_DIR/script.pack-tasks" \
    || fail "pack-view tasks must be the filtered set"
if grep -qx "too-slow" "$PROOF_WORK_DIR/script.pack-tasks" \
    || grep -qx "task-excluded" "$PROOF_WORK_DIR/script.pack-tasks"; then
    fail "script must not see unfiltered \$PROOF_PACK_DIR/tasks"
fi
got_path="$(cat "$PROOF_WORK_DIR/script.harbor-path")"
echo "$got_path" | grep -q "tasks-filtered" \
    || fail "harbor --path was $got_path (want tasks-filtered, not the unfiltered pack)"
if echo "$got_path" | grep -q "/pack/tasks$"; then
    fail "harbor wrapper must not pass the unfiltered pack tasks dir"
fi
grep -qx "COPY" "$PROOF_WORK_DIR/script.tasks-link" \
    || fail "pack-view tasks must be a materialized copy, not a symlink to the original pack"
grep -qx "UNSET" "$PROOF_WORK_DIR/script.harbor-real" \
    || fail "PROOF_HARBOR_REAL must not be exported to the miner script"
grep -qx "tasks" "$PROOF_WORK_DIR/script.pack-root" \
    || fail "pack-view must expose the filtered tasks dir"
if grep -qx "unfiltered-original" "$PROOF_WORK_DIR/script.pack-root"; then
    fail "pack-view must not copy original-pack siblings (Harbor --path recover)"
fi
python3 - "$FILTER_OUT/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
assert r["primary_value"] == 0.25, r
assert r["evidence"]["harness_kind"] == "script"
PY
pass "script harness is bound to filtered PROOF_TASKS (pack view + harbor --path wrap)"

# --- script timeout with reward.txt-only is fail-closed ---
SCRIPT_TIMEOUT="$WORKDIR/script-timeout"
mkdir -p "$SCRIPT_TIMEOUT/recipe"
cat > "$SCRIPT_TIMEOUT/recipe/run.sh" <<'EOF'
#!/bin/bash
job="$PROOF_WORK_DIR/harbor-jobs/job1/task-alpha__1"
mkdir -p "$job/verifier"
printf '0.4\n' > "$job/verifier/reward.txt"
exit 143
EOF
chmod 0755 "$SCRIPT_TIMEOUT/recipe/run.sh"
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
export PROOF_ARTIFACT_DIR="$SCRIPT_TIMEOUT"
TO_OUT="$WORKDIR/out-script-timeout"
mkdir -p "$TO_OUT"
export PROOF_OUTPUT_DIR="$TO_OUT"
if "$ADAPTOR/harness/run-harbor" >"$WORKDIR/timeout.out" 2>"$WORKDIR/timeout.err"; then
    fail "script timeout with reward.txt-only must fail closed"
fi
[ ! -f "$TO_OUT/report.json" ] || fail "must not publish miner-writable reward.txt-only"
pass "script harness timeout with reward.txt-only fails closed"

# --- excluded trial names cannot be scored even if the miner writes them ---
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
SCRIPT_BYPASS="$WORKDIR/script-bypass"
mkdir -p "$SCRIPT_BYPASS/recipe"
cat > "$SCRIPT_BYPASS/recipe/run.sh" <<'EOF'
#!/bin/bash
kept="$PROOF_WORK_DIR/harbor-jobs/job1/task-alpha__1"
drop="$PROOF_WORK_DIR/harbor-jobs/job1/task-excluded__1"
mkdir -p "$kept/verifier" "$drop/verifier"
cat > "$kept/result.json" <<JSON
{"trial_name": "task-alpha__1", "verifier_result": {"rewards": {"reward": 0.5}}}
JSON
printf '0.5\n' > "$kept/verifier/reward.txt"
cat > "$drop/result.json" <<JSON
{"trial_name": "task-excluded__1", "verifier_result": {"rewards": {"reward": 0.99}}}
JSON
printf '0.99\n' > "$drop/verifier/reward.txt"
exit 0
EOF
chmod 0755 "$SCRIPT_BYPASS/recipe/run.sh"
export PROOF_ARTIFACT_DIR="$SCRIPT_BYPASS"
BY_OUT="$WORKDIR/out-script-bypass"
mkdir -p "$BY_OUT"
export PROOF_OUTPUT_DIR="$BY_OUT"
"$ADAPTOR/harness/run-harbor" || fail "filtered-name scoring must still succeed"
python3 - "$BY_OUT/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
assert r["primary_value"] == 0.5, r
assert r["evidence"]["n_measured"] == 1, r
assert all("task-excluded" not in t["name"] for t in r["evidence"]["trials"]), r
PY
pass "summarize drops excluded task names even if the miner wrote them"

# --- partial filtered set (one of two allowlisted tasks) is fail-closed ---
TWO_PACK="$WORKDIR/two-pack"
mkdir -p "$TWO_PACK/tasks/task-alpha" "$TWO_PACK/tasks/task-beta"
printf '[agent]\ntimeout_sec = 120\n' > "$TWO_PACK/tasks/task-alpha/task.toml"
printf '[agent]\ntimeout_sec = 120\n' > "$TWO_PACK/tasks/task-beta/task.toml"
export PROOF_PACK_DIR="$TWO_PACK"
unset PROOF_TASKS PROOF_PARAM_TASKS || true
proof_require_tasks
proof_filter_tasks || fail "two-task pack should filter"
[ -d "$PROOF_TASKS/task-alpha" ] || fail "two-pack must keep task-alpha"
[ -d "$PROOF_TASKS/task-beta" ] || fail "two-pack must keep task-beta"
SCRIPT_PARTIAL="$WORKDIR/script-partial"
mkdir -p "$SCRIPT_PARTIAL/recipe"
cat > "$SCRIPT_PARTIAL/recipe/run.sh" <<'EOF'
#!/bin/bash
# Favourable score on one filtered task; skip the rest (P1 partial mean).
job="$PROOF_WORK_DIR/harbor-jobs/job1/task-alpha__1"
mkdir -p "$job/verifier"
cat > "$job/result.json" <<JSON
{"trial_name": "task-alpha__1", "verifier_result": {"rewards": {"reward": 1.0}}}
JSON
printf '1.0\n' > "$job/verifier/reward.txt"
exit 143
EOF
chmod 0755 "$SCRIPT_PARTIAL/recipe/run.sh"
rm -rf "$PROOF_WORK_DIR/harbor-jobs"
export PROOF_ARTIFACT_DIR="$SCRIPT_PARTIAL"
PARTIAL_SET_OUT="$WORKDIR/out-script-partial-set"
mkdir -p "$PARTIAL_SET_OUT"
export PROOF_OUTPUT_DIR="$PARTIAL_SET_OUT"
if "$ADAPTOR/harness/run-harbor" >"$WORKDIR/partial-set.out" 2>"$WORKDIR/partial-set.err"; then
    fail "subset of filtered tasks must fail closed"
fi
[ ! -f "$PARTIAL_SET_OUT/report.json" ] || fail "must not publish a partial filtered-set mean"
grep -qi "incomplete vs filtered\\|no complete Harbor trials" "$WORKDIR/partial-set.err" \
    || fail "must name incomplete filtered set: $(cat "$WORKDIR/partial-set.err")"
pass "partial filtered task set fails closed (no subset mean)"

# --- agent_exception_policy: a harness crash is topic policy, never a 503 by default flip ---
# Fake Harbor: one measured trial (0.0) + one trial whose agent phase raised.
cat > "$FAKE_BIN/harbor" <<'EOF'
#!/bin/bash
set -euo pipefail
jobs=""
while [ $# -gt 0 ]; do
    case "$1" in
        --jobs-dir) jobs="$2"; shift 2 ;;
        --timeout-multiplier) printf '%s\n' "$2" > "${PROOF_WORK_DIR}/harbor.timeout-multiplier"; shift 2 ;;
        --agent-timeout-multiplier) printf '%s\n' "$2" > "${PROOF_WORK_DIR}/harbor.agent-timeout-multiplier"; shift 2 ;;
        *) shift ;;
    esac
done
printf '%s\n' "${PROOF_EXEC_TIMEOUT_S-UNSET}" > "${PROOF_WORK_DIR}/harbor.exec-timeout"
ok="$jobs/job1/task-alpha__1"
mkdir -p "$ok/verifier"
cat > "$ok/result.json" <<JSON
{"trial_name": "task-alpha__1", "verifier_result": {"rewards": {"reward": 0.0}}}
JSON
printf '0.0\n' > "$ok/verifier/reward.txt"
crashed="$jobs/job1/task-beta__1"
mkdir -p "$crashed"
cat > "$crashed/result.json" <<JSON
{"trial_name": "task-beta__1", "exception_info": {"exception_type": "RuntimeError", "exception_message": "Command timed out after 120 seconds", "exception_traceback": "…", "occurred_at": "2026-09-11T00:00:00Z"}, "agent_execution": {"started_at": "2026-09-11T00:00:00Z", "finished_at": "2026-09-11T00:02:00Z"}, "verifier": null}
JSON
exit 0
EOF
chmod 0755 "$FAKE_BIN/harbor"
export PROOF_PACK_DIR="$TWO_PACK"
export PROOF_JOB=evaluate
export PROOF_ARTIFACT_DIR="$FIXTURES"
export PROOF_MODEL_PIN="vendor/model"
export PROOF_PARAM_MODEL="openrouter/vendor/model"
export PROOF_PARAM_MINER_BYOK=OPENROUTER_API_KEY
unset PROOF_PARAM_TASKS PROOF_PARAM_AGENT_EXCEPTION_POLICY PROOF_PARAM_EXEC_TIMEOUT_S || true
rm -rf "$PROOF_WORK_DIR/harbor-jobs" "$PROOF_WORK_DIR"/harbor.timeout-multiplier "$PROOF_WORK_DIR"/harbor.agent-timeout-multiplier
POLICY_FAIL_OUT="$WORKDIR/out-policy-fail"
mkdir -p "$POLICY_FAIL_OUT"
export PROOF_OUTPUT_DIR="$POLICY_FAIL_OUT"
if "$ADAPTOR/harness/run-harbor" >"$WORKDIR/policy-fail.out" 2>"$WORKDIR/policy-fail.err"; then
    fail "default agent_exception_policy=fail must refuse an uncovered crashed task"
fi
[ ! -f "$POLICY_FAIL_OUT/report.json" ] || fail "fail policy must not publish"
grep -qi "incomplete vs filtered\\|no complete Harbor trials" "$WORKDIR/policy-fail.err" \
    || fail "must name the incomplete set: $(cat "$WORKDIR/policy-fail.err")"
grep -qx "UNSET" "$PROOF_WORK_DIR/harbor.exec-timeout" || fail "PROOF_EXEC_TIMEOUT_S must stay unset for a silent topic"
[ ! -f "$PROOF_WORK_DIR/harbor.timeout-multiplier" ] || fail "no timeout multiplier flag for a silent topic"
pass "agent_exception_policy=fail (default) keeps the fail-closed behaviour"

rm -rf "$PROOF_WORK_DIR/harbor-jobs"
export PROOF_PARAM_AGENT_EXCEPTION_POLICY=zero
export PROOF_PARAM_EXEC_TIMEOUT_S=900
export PROOF_PARAM_TIMEOUT_MULTIPLIER=2
export PROOF_PARAM_AGENT_TIMEOUT_MULTIPLIER=1.5
POLICY_ZERO_OUT="$WORKDIR/out-policy-zero"
mkdir -p "$POLICY_ZERO_OUT"
export PROOF_OUTPUT_DIR="$POLICY_ZERO_OUT"
"$ADAPTOR/harness/run-harbor" >"$WORKDIR/policy-zero.out" 2>"$WORKDIR/policy-zero.err" \
    || fail "agent_exception_policy=zero must score the crashed task as 0: $(cat "$WORKDIR/policy-zero.err")"
python3 - "$POLICY_ZERO_OUT/report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
ev = r["evidence"]
assert r["primary_value"] == 0.0, r
assert ev["n_scored"] == 2 and ev["n_measured"] == 1 and ev["n_agent_exceptions"] == 1, ev
assert ev["agent_exception_policy"] == "zero"
assert ev["agent_exception_trials"][0]["exception_type"] == "RuntimeError"
assert "120 seconds" in ev["agent_exception_trials"][0]["exception_message"]
PY
grep -qx "900" "$PROOF_WORK_DIR/harbor.exec-timeout" || fail "PROOF_EXEC_TIMEOUT_S must carry exec_timeout_s to the harness"
grep -qx "2" "$PROOF_WORK_DIR/harbor.timeout-multiplier" || fail "timeout_multiplier must reach Harbor"
grep -qx "1.5" "$PROOF_WORK_DIR/harbor.agent-timeout-multiplier" || fail "agent_timeout_multiplier must reach Harbor"
pass "agent_exception_policy=zero scores a harness crash as 0 with evidence; timeouts are topic data"

if (export PROOF_PARAM_AGENT_EXCEPTION_POLICY=zer0; "$ADAPTOR/harness/run-harbor") >/dev/null 2>"$WORKDIR/policy-typo.err"; then
    fail "a malformed agent_exception_policy must fail closed"
fi
grep -q "agent_exception_policy" "$WORKDIR/policy-typo.err" || fail "must name the knob"
unset PROOF_PARAM_AGENT_EXCEPTION_POLICY PROOF_PARAM_EXEC_TIMEOUT_S PROOF_PARAM_TIMEOUT_MULTIPLIER PROOF_PARAM_AGENT_TIMEOUT_MULTIPLIER
pass "malformed agent_exception_policy fails closed before Harbor"

# --- network / verifier knobs are topic data with today's defaults ---
proof_require_tasks
proof_filter_tasks
printf '[environment]\nnetwork_mode = "no-network"\n' > "$PROOF_TASKS/task-alpha/task.toml"
export PROOF_PARAM_TASK_NETWORK_MODE=keep
proof_enable_agent_network || fail "task_network_mode=keep must succeed"
grep -q 'no-network' "$PROOF_TASKS/task-alpha/task.toml" || fail "keep must leave the pack's network settings"
unset PROOF_PARAM_TASK_NETWORK_MODE
proof_enable_agent_network || fail "default network mode must rewrite"
grep -q 'network_mode = "public"' "$PROOF_TASKS/task-alpha/task.toml" || fail "default must be public"
if (export PROOF_PARAM_TASK_NETWORK_MODE=bridge; proof_enable_agent_network) 2>/dev/null; then
    fail "an unknown task_network_mode must fail closed"
fi
mkdir -p "$PROOF_TASKS/task-alpha/environment"
printf 'FROM python:3.12-slim\n' > "$PROOF_TASKS/task-alpha/environment/Dockerfile"
export PROOF_PARAM_ENSURE_VERIFIER_PYTEST=false
proof_ensure_verifier || fail "ensure_verifier_pytest=false must succeed"
if grep -q 'pytest' "$PROOF_TASKS/task-alpha/environment/Dockerfile"; then
    fail "ensure_verifier_pytest=false must leave the image untouched"
fi
unset PROOF_PARAM_ENSURE_VERIFIER_PYTEST
proof_ensure_verifier || fail "default ensure_verifier must patch"
grep -q 'pytest' "$PROOF_TASKS/task-alpha/environment/Dockerfile" || fail "default must inject pytest"
pass "task_network_mode / ensure_verifier_pytest are topic knobs with today's defaults"

echo "all adaptor tests passed"
