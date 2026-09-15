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

echo "all proof-slice-preflight tests passed"
