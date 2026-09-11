#!/bin/bash
# Host n15 wrapper: wait + summarize; restart path logs job_http + DONE.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ADAPTOR="$(cd "$HERE/.." && pwd)"
RUN_N15="$ADAPTOR/host/run-n15"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }

chmod +x "$RUN_N15"

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/n15-wrapper-XXXXXX")"
cleanup() { rm -rf "$WORKDIR"; }
trap cleanup EXIT

# --- fail closed without JOBDIR ---
if out="$("$RUN_N15" 2>&1)"; then
    fail "run-n15 without JOBDIR must fail"
fi
echo "$out" | grep -q 'JOBDIR is required' || fail "missing JOBDIR must say so: $out"
pass "no JOBDIR is fail closed"

# --- restart path waits for nested job.out, logs DONE, writes summary ---
JOBDIR="$WORKDIR/n15-a21f9618-shortpack"
mkdir -p "$JOBDIR/orch"
python3 - <<'PY' "$JOBDIR"
import json, sys
from pathlib import Path
jobdir = Path(sys.argv[1])
payload = {
    "output": {
        "output": "baseline",
        "body": {
            "primary_value": 0.0,
            "evidence": {
                "n_measured": 6,
                "harbor_exit": 0,
                "trials": [{"name": "atrx", "reward": 0.0}],
            },
        },
    }
}
(jobdir / "orch" / "job.out").write_text(json.dumps(payload), encoding="utf-8")
(jobdir / "job_http").write_text("200\n", encoding="utf-8")
PY
export N15_WAIT_SECS=5
out="$("$RUN_N15" --restart "$JOBDIR" 2>&1)" || fail "restart with job.out should succeed: $out"
echo "$out" | grep -q 'N15 RESTART' || fail "restart path must log N15 RESTART: $out"
echo "$out" | grep -q 'job_http=200 DONE' || fail "restart path must log job_http + DONE: $out"
grep -q 'N15 RESTART' "$JOBDIR/run.log" || fail "run.log must contain N15 RESTART"
grep -q 'job_http=200 DONE' "$JOBDIR/run.log" || fail "run.log must contain job_http + DONE"
[ -f "$JOBDIR/custom_value.txt" ] || fail "summarize must write custom_value.txt"
[ -f "$JOBDIR/summary.txt" ] || fail "summarize must write summary.txt"
grep -q 'primary_value=0.0' "$JOBDIR/summary.txt" || fail "summary.txt must have primary_value"
pass "restart path waits, logs job_http DONE, writes summary"

# --- curl.pid is waited even when not our child (orphaned curl) ---
JOB2="$WORKDIR/wait-curl"
mkdir -p "$JOB2"
# A short sleep stands in for Harbor curl; job.out appears after it exits.
sleep 2 &
echo $! >"$JOB2/curl.pid"
(
    # After curl.pid exits, plant job.out so wait_job_out returns.
    while kill -0 "$(cat "$JOB2/curl.pid")" 2>/dev/null; do
        sleep 0.2
    done
    printf '%s\n' '{"output":{"output":"baseline","body":{"primary_value":0.5,"evidence":{"n_measured":1,"trials":[{"name":"a","reward":0.5}]}}}}' >"$JOB2/job.out"
    echo 201 >"$JOB2/job_http"
) &
export N15_WAIT_SECS=15
out="$("$RUN_N15" "$JOB2" 2>&1)" || fail "wait curl.pid should succeed: $out"
echo "$out" | grep -q 'job_http=201 DONE' || fail "must log DONE after curl: $out"
echo "$out" | grep -qv 'N15 RESTART' || fail "non-restart must not log N15 RESTART"
[ -f "$JOB2/custom_value.txt" ] || fail "must summarize after curl"
grep -q '0.5' "$JOB2/custom_value.txt" || fail "custom_value.txt from waited job"
pass "orphaned curl.pid is waited then summarized"

# --- missing job.out fails closed ---
JOB3="$WORKDIR/no-job"
mkdir -p "$JOB3"
export N15_WAIT_SECS=2
if out="$("$RUN_N15" "$JOB3" 2>&1)"; then
    fail "missing job.out must fail: $out"
fi
echo "$out" | grep -q 'job.out never appeared' || fail "must name missing job.out: $out"
[ ! -f "$JOB3/custom_value.txt" ] || fail "must not write custom_value without job.out"
pass "missing job.out is fail closed"

echo "run-n15 tests: all passed"
