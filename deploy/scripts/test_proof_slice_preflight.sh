#!/usr/bin/env bash
# proof-slice-preflight.sh: the selection a topic names is the selection it gets.
#
# The LIVE Gate 1 failure was a `task_slice` that did not resolve, on a pack
# with no `slices/`, silently scored as *every* task. The guest refuses that
# now; this covers the preflight that reports it in seconds instead of after a
# provision + boot cycle.
#
# No task name, pack, or slice is compiled in: every fixture here is a
# placeholder this test invents.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
PREFLIGHT="$REPO_ROOT/deploy/scripts/proof-slice-preflight.sh"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}
pass() { echo "ok - $*"; }

[ -x "$PREFLIGHT" ] || fail "preflight is not executable at $PREFLIGHT"

work="$(mktemp -d "${TMPDIR:-/tmp}/proof-slice-preflight-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT

make_pack() {
    local root="$1"
    shift
    mkdir -p "$root/tasks"
    for t in "$@"; do
        mkdir -p "$root/tasks/$t"
        printf '[task]\nname = "%s"\n' "$t" >"$root/tasks/$t/task.toml"
    done
}

# --- a pack that defines the slice: resolved=true, exactly the named set ----
with_slices="$work/with-slices"
make_pack "$with_slices" t-one t-two t-three t-four t-five t-extra
mkdir -p "$with_slices/slices"
printf '%s\n' '["t-one","t-two","t-three","t-four","t-five"]' >"$with_slices/slices/five.json"

out="$("$PREFLIGHT" --pack-dir "$with_slices" --task-slice five --expect 5 \
    --keep t-one,t-two,t-three,t-four,t-five 2>&1)" || fail "a defined slice must resolve: $out"
grep -q "resolved=true" <<<"$out" || fail "must report resolved=true: $out"
pass "a pack-defined slice resolves, keeps exactly five, and reports resolved=true"

# --- the LIVE failure: a label on a pack with NO slices must be refused ------
no_slices="$work/no-slices"
make_pack "$no_slices" t-one t-two t-three
if out="$("$PREFLIGHT" --pack-dir "$no_slices" --task-slice five 2>&1)"; then
    fail "a label on a slice-less pack must be refused, got: $out"
fi
grep -q "REFUSED" <<<"$out" || fail "the refusal must say REFUSED: $out"
grep -q "defines no slices" <<<"$out" || fail "the refusal must name the pack's state: $out"
pass "an unresolved slice on a slice-less pack is refused before any run"

# --- a typo against a pack that HAS slices is refused, and names them --------
if out="$("$PREFLIGHT" --pack-dir "$with_slices" --task-slice typo 2>&1)"; then
    fail "a typo'd slice must be refused, got: $out"
fi
grep -q "five.json" <<<"$out" || fail "the refusal must name the slices the pack has: $out"
pass "a typo'd slice is refused and the refusal names the pack's own slices"

# --- explicit tasks: the documented escape, bounded by n_tasks ---------------
out="$("$PREFLIGHT" --pack-dir "$no_slices" --tasks t-one,t-two --n-tasks 2 --expect 2 2>&1)" \
    || fail "an explicit set must resolve even with no slices: $out"
grep -q "t-one,t-two" <<<"$out" || fail "the explicit set must be what is kept: $out"
pass "an explicit params.tasks set resolves on a slice-less pack"

# --- the escape WITH a stale label still set: the label is not read ----------
# The Owner's migration shape: a topic carries an old `task_slice` it cannot
# resolve *and* names the set explicitly. The guest reads `params.tasks` and
# never resolves the label, so the run is correct — but this preflight refused
# it, which blocks the very fix it exists to prove. Assert the guest's own
# answer, both directions, so the two cannot drift apart again.
out="$("$PREFLIGHT" --pack-dir "$no_slices" --task-slice five --tasks t-one,t-two --expect 2 2>&1)" \
    || fail "a stale label beside an explicit set must not be refused: $out"
grep -q "is not read: params.tasks named the set" <<<"$out" \
    || fail "the preflight must say the label is not read: $out"
pass "a stale label beside params.tasks passes (the guest reads the tasks)"
# …and the label alone is still the LIVE refusal: the escape is the tasks, not
# the label.
if out="$("$PREFLIGHT" --pack-dir "$no_slices" --task-slice five 2>&1)"; then
    fail "the label alone must still be refused, got: $out"
fi
pass "the stale label alone is still refused (the escape is the explicit set)"

# --- a whitespace-only selector is ABSENT, not a selection -------------------
# The guest normalizes a whitespace-only value as absent (`present()` trims,
# then rejects empty), so `--tasks "   "` beside a slice selects the **slice**.
# Reading the raw shell variables instead of the summary made the preflight
# claim "params.tasks named the set" on a run that took the slice branch —
# the wrong explanation for the operator, on the exact shape a re-sign
# produces. Both directions, against the guest's own answer.
out="$("$PREFLIGHT" --pack-dir "$with_slices" --tasks "   " --task-slice five --expect 5 2>&1)" \
    || fail "a whitespace-only --tasks must fall through to the slice: $out"
grep -q "task_slice 'five' resolved through the pack" <<<"$out" \
    || fail "the slice must be reported as the selector, not params.tasks: $out"
pass "a whitespace-only --tasks beside a slice selects the slice (guest semantics)"
out="$("$PREFLIGHT" --pack-dir "$no_slices" --task-slice "   " --tasks t-one,t-two --expect 2 2>&1)" \
    || fail "a whitespace-only --task-slice must not be read as a label: $out"
grep -q "is not read" <<<"$out" \
    || fail "with the tasks naming the set, no label should be reported: $out"
pass "a whitespace-only --task-slice beside params.tasks is not read"

# --- a count that does not match is caught, not silently accepted ------------
if out="$("$PREFLIGHT" --pack-dir "$with_slices" --task-slice five --expect 3 2>&1)"; then
    fail "--expect must fail when the selection is a different size, got: $out"
fi
grep -q "expected 3 tasks selected, got 5" <<<"$out" \
    || fail "the mismatch must say both numbers: $out"
pass "--expect catches a selection of the wrong size"

# --- order matters: the same names in a different order are a mismatch -------
if out="$("$PREFLIGHT" --pack-dir "$with_slices" --task-slice five \
    --keep t-two,t-one,t-three,t-four,t-five 2>&1)"; then
    fail "--keep must compare order, got: $out"
fi
grep -q "order matters" <<<"$out" || fail "the mismatch must say order matters: $out"
pass "--keep compares the order the topic named"

# --- a missing pack is a refusal with a usable message ----------------------
if out="$("$PREFLIGHT" --pack-dir "$work/does-not-exist" 2>&1)"; then
    fail "a missing pack dir must be refused, got: $out"
fi
grep -q "not a directory" <<<"$out" || fail "the message must name the problem: $out"
pass "a missing pack directory is refused"

# --- no argument is a usage refusal, not a silent pass ----------------------
if out="$("$PREFLIGHT" 2>&1)"; then
    fail "a missing --pack-dir must be refused, got: $out"
fi
grep -q -- "--pack-dir is required" <<<"$out" || fail "the message must name the flag: $out"
pass "a missing --pack-dir is a usage refusal"

# --- the pack is never modified ---------------------------------------------
before="$(cd "$with_slices" && find . -type f | sort | xargs sha256sum | sha256sum)"
"$PREFLIGHT" --pack-dir "$with_slices" --task-slice five --expect 5 >/dev/null 2>&1
after="$(cd "$with_slices" && find . -type f | sort | xargs sha256sum | sha256sum)"
[ "$before" = "$after" ] || fail "the preflight must not touch the pack"
pass "the preflight leaves the pack byte-identical"

# ---------------------------------------------------------------------------
# Differential: the preflight must select what the **guest** selects.
#
# Greptile found the first version of this script forwarded only four of the
# eight selector inputs `lib.sh` passes, so it could print a clean PASS for a
# set the guest would never run — exactly the class of failure the script
# exists to catch. The check below is the real thing: it sources the adaptor's
# own `lib.sh`, drives `proof_filter_tasks` the way the guest does, and
# compares the kept set against the preflight's, for every supported selector.
# ---------------------------------------------------------------------------
ADAPTOR="$REPO_ROOT/deploy/guest/runners/rlm_fc_in_guest_harbor"

# A pack with a filter file, a slice, durations and a deny list, so every
# selector has something to bite on. Names are placeholders this test invents.
diff_pack="$work/diff-pack"
mkdir -p "$diff_pack/tasks" "$diff_pack/slices"
for t in d-fast d-slow d-unknown d-denied d-fifth d-sixth; do
    mkdir -p "$diff_pack/tasks/$t"
    case "$t" in
        d-fast) dur=10 ;;
        d-slow) dur=900 ;;
        d-denied) dur=10 ;;
        d-fifth) dur=10 ;;
        d-sixth) dur=10 ;;
        *) dur= ;; # d-unknown declares no duration
    esac
    {
        printf '[task]\nname = "%s"\n' "$t"
        [ -n "$dur" ] && printf 'estimated_duration_s = %s\n' "$dur"
    } >"$diff_pack/tasks/$t/task.toml"
done
printf '%s\n' '["d-fast","d-slow","d-unknown"]' >"$diff_pack/slices/three.json"
cat >"$diff_pack/strict.json" <<'JSON'
{"allow": ["d-fast", "d-slow", "d-unknown", "d-denied"],
 "deny": ["d-denied"],
 "slices": {"two": ["d-fast", "d-slow"]}}
JSON

# The guest's own path: source lib.sh, set the topic params, call the filter.
# Prints the kept names in order, one per line.
guest_select() {
    local env_pairs=("$@")
    local wd="$work/guest-work"
    rm -rf "$wd"
    mkdir -p "$wd"
    (
        export PROOF_TASKS="$diff_pack/tasks"
        export PROOF_WORK_DIR="$wd"
        export PROOF_PACK_DIR="$diff_pack"
        # shellcheck disable=SC2068  # env pairs are intentional here
        export ${env_pairs[@]+"${env_pairs[@]}"}
        # shellcheck source=/dev/null
        source "$ADAPTOR/lib.sh"
        proof_filter_tasks >/dev/null 2>&1 || return 1
        python3 - "$PROOF_TASKS/.proof-task-filter.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as fh:
    doc = json.load(fh)
for row in doc["kept"]:
    print(row["name"])
PY
    )
}

# The preflight's answer for the same topic data, as the same list.
guest_case() {
    local label="$1"
    shift
    local guest pre env_pairs=() flags=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --tasks) env_pairs+=("PROOF_PARAM_TASKS=$2"); flags+=(--tasks "$2"); shift 2 ;;
            --exclude) env_pairs+=("PROOF_PARAM_TASK_EXCLUDE=$2"); flags+=(--exclude "$2"); shift 2 ;;
            --n-tasks) env_pairs+=("PROOF_PARAM_N_TASKS=$2"); flags+=(--n-tasks "$2"); shift 2 ;;
            --task-count) env_pairs+=("PROOF_PARAM_TASK_COUNT=$2"); flags+=(--task-count "$2"); shift 2 ;;
            --task-slice) env_pairs+=("PROOF_TASK_SLICE=$2"); flags+=(--task-slice "$2"); shift 2 ;;
            --filter-rel) env_pairs+=("PROOF_PARAM_TASK_FILTER=$2"); flags+=(--filter-rel "$2"); shift 2 ;;
            --max-duration-s) env_pairs+=("PROOF_PARAM_MAX_TASK_DURATION_S=$2"); flags+=(--max-duration-s "$2"); shift 2 ;;
            --drop-unknown) env_pairs+=("PROOF_PARAM_EXCLUDE_UNKNOWN_DURATION=true"); flags+=(--drop-unknown); shift ;;
            *) fail "guest_case: unknown selector $1" ;;
        esac
    done
    guest="$(guest_select ${env_pairs[@]+"${env_pairs[@]}"} 2>/dev/null)" \
        || fail "$label: the guest's own selection failed unexpectedly"
    pre="$("$PREFLIGHT" --pack-dir "$diff_pack" ${flags[@]+"${flags[@]}"} 2>&1 \
        | sed -n 's/^    kept: //p' | tr ',' '\n')" \
        || fail "$label: the preflight refused where the guest selected"
    [ "$guest" = "$pre" ] \
        || fail "$label: preflight and guest disagree
  guest:
$guest
  preflight:
$pre"
    [ -n "$guest" ] || fail "$label: both selected nothing"
    pass "$label: preflight selects exactly what the guest selects"
}

guest_case "task_slice through the pack" --task-slice three
guest_case "explicit tasks" --tasks d-fast,d-fifth
guest_case "task_exclude" --exclude d-slow
guest_case "n_tasks truncation" --n-tasks 2
guest_case "legacy task_count" --task-count 2
guest_case "n_tasks wins over task_count" --n-tasks 3 --task-count 1
guest_case "named pack filter" --filter-rel strict.json
guest_case "duration ceiling" --max-duration-s 100
guest_case "duration ceiling dropping unknowns" --max-duration-s 100 --drop-unknown

# The exact combination Greptile reproduced: the preflight used to keep three
# tasks where the guest kept one.
guest_case "Greptile's combined case" \
    --task-count 2 --filter-rel strict.json --max-duration-s 100 --drop-unknown

echo "all proof-slice-preflight tests passed"
