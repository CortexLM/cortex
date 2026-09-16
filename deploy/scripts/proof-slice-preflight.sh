#!/usr/bin/env bash
# Preflight a topic's task selection against the pack it pins — before a re-sign.
#
# Why this exists: LIVE Gate 1 failed because `task_slice=tb4-first-5` did not
# resolve against a pack with no `slices/`, and the guest silently scored every
# task instead of five. The run overran its wall clock and measured no baseline.
# The guest now refuses that case (filter_tasks.py), which is correct but means
# a bad selection costs a full provision+boot cycle to discover. This runs the
# **same** selection the guest runs, on the host, in seconds.
#
# It is deliberately the guest's own code path: it execs the adaptor's
# `harness/filter_tasks.py` with the same arguments `lib.sh` passes, so a PASS
# here is the guest's own answer, not a re-implementation that could drift.
#
# Usage:
#   deploy/scripts/proof-slice-preflight.sh \
#     --pack-dir /var/lib/proof-vm/packs/<pack> \
#     --task-slice tb4-first-5
#
#   # or the explicit-set form the Owner may prefer:
#   deploy/scripts/proof-slice-preflight.sh \
#     --pack-dir /var/lib/proof-vm/packs/<pack> \
#     --tasks bun-sourcemap-leak,foodstuff-beta-activity,cad-model,cargo-flight-dispatch,formal-crypto \
#     --n-tasks 5
#
#   # or the pack's own filter.json / slices/ with no topic data:
#   deploy/scripts/proof-slice-preflight.sh --pack-dir /var/lib/proof-vm/packs/<pack> --expect 5
#
# Every selector the guest supports is forwarded with the same precedence
# `lib.sh` uses: `tasks` -> `task_slice` -> the pack filter's `allow` -> every
# task; then `exclude` / pack `deny`, a duration gate, `n_tasks`. A preflight
# that silently dropped one of them would report a set the guest never runs.
#
# Read-only: it writes only to a temporary directory it removes, and never
# touches the pack, the database, or any host service.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"

ADAPTOR_DIR="${PROOF_SLICE_PREFLIGHT_ADAPTOR:-$REPO_ROOT/deploy/guest/runners/rlm_fc_in_guest_harbor}"
FILTER_TASKS="$ADAPTOR_DIR/harness/filter_tasks.py"

pack_dir=""
tasks_rel="tasks"
task_slice=""
tasks=""
exclude=""
n_tasks=""
task_count=""
filter_rel=""
max_duration_s=""
drop_unknown=""
expect=""
keep=""

die() {
    echo "proof-slice-preflight: $*" >&2
    exit 2
}
pass() { echo "ok - $*"; }
note() { echo "    $*"; }

usage() {
    sed -n '2,35p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --pack-dir) pack_dir="${2:-}"; shift 2 ;;
        --tasks-rel) tasks_rel="${2:-}"; shift 2 ;;
        --task-slice) task_slice="${2:-}"; shift 2 ;;
        --tasks) tasks="${2:-}"; shift 2 ;;
        --exclude) exclude="${2:-}"; shift 2 ;;
        --n-tasks) n_tasks="${2:-}"; shift 2 ;;
        --task-count) task_count="${2:-}"; shift 2 ;;
        --filter-rel) filter_rel="${2:-}"; shift 2 ;;
        --max-duration-s) max_duration_s="${2:-}"; shift 2 ;;
        # Bare flag, matching lib.sh: the guest emits it only when the topic's
        # `exclude_unknown_duration` normalises to exactly `true`.
        --drop-unknown) drop_unknown="true"; shift ;;
        --expect) expect="${2:-}"; shift 2 ;;
        --keep) keep="${2:-}"; shift 2 ;;
        --adaptor-dir) ADAPTOR_DIR="${2:-}"; FILTER_TASKS="$ADAPTOR_DIR/harness/filter_tasks.py"; shift 2 ;;
        -h | --help) usage ;;
        *) die "unknown argument $1 (try --help)" ;;
    esac
done

[ -n "$pack_dir" ] || die "--pack-dir is required (the pack the topic pins)"
[ -d "$pack_dir" ] || die "--pack-dir $pack_dir is not a directory"
[ -f "$FILTER_TASKS" ] || die "the adaptor's filter_tasks.py is missing at $FILTER_TASKS"

tasks_dir="$pack_dir/$tasks_rel"
[ -d "$tasks_dir" ] || die "no tasks directory $tasks_dir (check --tasks-rel)"

work="$(mktemp -d "${TMPDIR:-/tmp}/proof-slice-preflight.XXXXXX")"
trap 'rm -rf "$work"' EXIT
dest="$work/selected"

# Same arguments, same precedence, as `proof_filter_tasks` in lib.sh. Every
# selector the guest supports is here: one omitted would let this report a set
# the guest never runs.
args=(
    --tasks-dir "$tasks_dir"
    --dest-dir "$dest"
    --pack-dir "$pack_dir"
)
[ -n "$tasks" ] && args+=(--tasks "$tasks")
[ -n "$exclude" ] && args+=(--exclude "$exclude")
[ -n "$n_tasks" ] && args+=(--n-tasks "$n_tasks")
# `task_count` is a legacy alias for `n_tasks`; `n_tasks` wins when both are set.
[ -z "$n_tasks" ] && [ -n "$task_count" ] && args+=(--task-count "$task_count")
[ -n "$task_slice" ] && args+=(--task-slice "$task_slice")
[ -n "$filter_rel" ] && args+=(--filter-rel "$filter_rel")
[ -n "$max_duration_s" ] && args+=(--max-duration-s "$max_duration_s")
[ -n "$drop_unknown" ] && args+=(--drop-unknown)

echo "proof-slice-preflight"
echo "  pack_dir          $pack_dir"
echo "  tasks_dir         $tasks_dir"
echo "  task_slice        ${task_slice:-<none>}"
echo "  tasks             ${tasks:-<none>}"
echo "  exclude           ${exclude:-<none>}"
echo "  n_tasks           ${n_tasks:-<none>}"
echo "  task_count        ${task_count:-<none>}"
echo "  filter_rel        ${filter_rel:-<none>}"
echo "  max_duration_s    ${max_duration_s:-<none>}"
echo "  drop_unknown      ${drop_unknown:-<none>}"
echo "  adaptor           $FILTER_TASKS"
echo

# The guest's own selection, run the way lib.sh runs it.
if ! python3 "$FILTER_TASKS" "${args[@]}"; then
    echo >&2
    echo "proof-slice-preflight: REFUSED — the guest would refuse this run too." >&2
    echo "  A topic that names a slice the pack does not define is not scored on a" >&2
    echo "  different set; the run stops here instead of overrunning its wall clock." >&2
    echo "  Fix the pack (add slices/<label>.json) or name the set with params.tasks." >&2
    exit 1
fi

summary="$dest/.proof-task-filter.json"
[ -f "$summary" ] || die "the selection wrote no $summary"

read_summary() {
    python3 - "$summary" "$1" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as fh:
    doc = json.load(fh)
value = doc.get(sys.argv[2], "")
# JSON booleans render as Python `True` / `False`; the shell compares against
# the JSON spelling, so normalise here rather than at every call site.
print("true" if value is True else "false" if value is False else value)
PY
}

resolved="$(read_summary task_slice_resolved)"
kept="$(read_summary n_kept)"
source="$(read_summary source)"
names="$(python3 - "$summary" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as fh:
    doc = json.load(fh)
print(",".join(row["name"] for row in doc.get("kept", [])))
PY
)"

echo
echo "  source            $source"
echo "  task_slice_resolved $resolved"
echo "  n_kept            $kept"
note "kept: $names"
echo

# A slice that was set but did not resolve is the LIVE Gate 1 failure. The guest
# refuses it now — **unless** the topic also named the set explicitly, in which
# case `params.tasks` is the documented escape and the label is not read at all
# (`filter_tasks.select_base` returns before resolving it). Refusing here would
# block the very fix this preflight is meant to prove, so the assertion follows
# the guest: a label that did not resolve is a refusal only when it was the
# selector.
if [ -n "$task_slice" ] && [ -z "$tasks" ]; then
    case "$resolved" in
        true) pass "task_slice '$task_slice' resolved through the pack (resolved=true)" ;;
        *) die "task_slice '$task_slice' did not resolve (resolved=$resolved) — the guest would refuse this run" ;;
    esac
elif [ -n "$task_slice" ] && [ -n "$tasks" ]; then
    pass "task_slice '$task_slice' is not read: params.tasks named the set (source=$source)"
fi

if [ -n "$expect" ]; then
    [ "$kept" = "$expect" ] \
        || die "expected $expect tasks selected, got $kept"
    pass "exactly $expect task(s) selected"
fi

if [ -n "$keep" ]; then
    IFS=',' read -r -a want <<<"$keep"
    IFS=',' read -r -a got <<<"$names"
    [ "${#want[@]}" -eq "${#got[@]}" ] \
        || die "expected ${#want[@]} task(s) ($keep), got ${#got[@]} ($names)"
    for i in "${!want[@]}"; do
        [ "${want[$i]}" = "${got[$i]}" ] \
            || die "task $((i + 1)): expected '${want[$i]}', got '${got[$i]}' (order matters)"
    done
    pass "the selected set is exactly: $keep"
fi

echo
pass "selection is what the guest will run"
