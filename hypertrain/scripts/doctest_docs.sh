#!/usr/bin/env bash
# Doc-test the guides: run every fenced block tagged `bash doctest` against the local stack.
#   bash scripts/doctest_docs.sh            # docs/operator.md docs/miner.md
#   DOCS="a.md b.md" bash scripts/doctest_docs.sh
# Blocks of one doc run in order in ONE bash process (set -Eeuo pipefail) from the tree root,
# so later blocks see earlier variables. The first failing command is named with its doc,
# block and line, and the script exits nonzero. Blocks tagged `doctest-skip:prod` are listed,
# never run: they need a Cortex master and are checked by read-review against the pinned
# Cortex docs (cortex@d738424). DOCTEST_TIMEOUT (seconds per doc, default 1800).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
docs=${DOCS:-docs/operator.md docs/miner.md}
limit=${DOCTEST_TIMEOUT:-1800}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# extract DOC OUT KIND: write blocks of KIND as a script with block markers, print a summary.
extract() {
  awk -v kind="$3" -v doc="$1" -v out="$2" '
    /^```/ {
      if (inb) { inb = 0; print "__ht_block_end" >> out; next }
      tag = substr($0, 4)
      if (tag == "bash " kind) { inb = 1; n++; first = ""; start = NR
        print "__ht_block " n " " NR >> out; next }
    }
    inb { if (first == "") first = $0; print >> out
      if (!seen[n]++) printf "  %s:%d block %d: %s\n", doc, start, n, first }
    END { if (inb) { print "UNTERMINATED block at " doc ":" start > "/dev/stderr"; exit 3 } }
  ' "$1"
}

total=0
failed=0
for doc in $docs; do
  [ -f "$doc" ] || { echo "FAIL: $doc not found" >&2; exit 2; }
  echo "== $doc: production-only blocks (doctest-skip:prod, read-reviewed, not run)"
  : >"$work/skip"
  extract "$doc" "$work/skip" doctest-skip:prod
  script="$work/$(basename "$doc").sh"
  {
    echo 'set -Eeuo pipefail'
    echo "__ht_doc='$doc'"
    echo '__ht_block() { __ht_n=$1; __ht_line=$2; echo "-- $__ht_doc block $1 (line $2)"; }'
    echo '__ht_block_end() { echo "   EXIT 0"; }'
    echo 'trap '"'"'rc=$?; echo "DOCTEST FAILED: $__ht_doc block ${__ht_n:-?} (line ${__ht_line:-?}) exit $rc: $BASH_COMMAND" >&2; exit $rc'"'"' ERR'
  } >"$script"
  echo "== $doc: doctest blocks"
  extract "$doc" "$script" doctest >"$work/list"
  cat "$work/list"
  n=$(grep -c '^__ht_block ' "$script" || true)
  [ "$n" -gt 0 ] || { echo "FAIL: $doc has no doctest blocks" >&2; exit 2; }
  total=$((total + n))
  echo "== $doc: running $n blocks"
  rc=0
  timeout --kill-after=30 "$limit" bash "$script" || rc=$?
  if [ "$rc" -ne 0 ]; then
    [ "$rc" -eq 124 ] && echo "DOCTEST FAILED: $doc timed out after ${limit}s" >&2
    echo "== $doc: FAIL (exit $rc)"
    failed=$((failed + 1))
  else
    echo "== $doc: PASS"
  fi
done
echo "== summary: $total doctest blocks, $failed failing doc(s)"
[ "$failed" -eq 0 ]
