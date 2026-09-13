#!/usr/bin/env bash
# Fail if the tip Harbor runner tree does not emit results.json.
#
# Metal RCA (tip b7d52fa6 / guest pin sha256:0d9329ea… baked before #293):
# in-guest /opt/proof/runners lacked the emit while deploy/guest/runners on
# tip already wrote tbench-harbor-v1. This gate is the tip-tree half — a
# stale guest pin is still fail-closed until rebake; tipping gw/challenge
# alone is not a runner update.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ADAPTOR="${HARBOR_ADAPTOR_DIR:-$ROOT/deploy/guest/runners/rlm_fc_in_guest_harbor}"
SUMMARIZE="$ADAPTOR/harness/summarize.py"
RUN_HARBOR="$ADAPTOR/harness/run-harbor"
LIB_SH="$ADAPTOR/lib.sh"

die() {
    echo "assert-harbor-runner-results-emit: $*" >&2
    exit 1
}

[ -f "$SUMMARIZE" ] || die "missing $SUMMARIZE"
[ -f "$RUN_HARBOR" ] || die "missing $RUN_HARBOR"
[ -f "$LIB_SH" ] || die "missing $LIB_SH"

need() {
    local file="$1" needle="$2"
    grep -Fq -- "$needle" "$file" || die "$file lacks required emit marker: $needle"
}

need "$SUMMARIZE" 'def write_results_next_to_report'
need "$SUMMARIZE" 'CONTRACT_TBENCH = "tbench-harbor-v1"'
need "$SUMMARIZE" 'CONTRACT_HARBOR_TRIALS = "harbor-trials-v1"'
need "$SUMMARIZE" 'results.json first'
need "$SUMMARIZE" 'write_results_next_to_report(out, report, trials, log_tail, secrets)'
need "$SUMMARIZE" 'atomic_write(out, dumped + "\n")'
need "$SUMMARIZE" 'def attach_trial_logs'
need "$SUMMARIZE" 'agent_log'
need "$SUMMARIZE" 'verifier/test-stdout.txt'
need "$SUMMARIZE" '"trial.log"'
need "$LIB_SH" 'proof_require_harbor_results'
need "$RUN_HARBOR" 'proof_require_harbor_results'

# Source order in main(): write results.json, then report.json.
python3 - "$SUMMARIZE" <<'PY' || die "summarize.py main() does not write results.json before report.json"
import sys
from pathlib import Path

src = Path(sys.argv[1]).read_text(encoding="utf-8")
main = src.split("def main(", 1)[-1]
results_at = main.find("write_results_next_to_report(out, report, trials, log_tail, secrets)")
report_at = main.find("atomic_write(out, dumped")
if results_at < 0 or report_at < 0 or results_at >= report_at:
    raise SystemExit(
        f"main() must write results.json before report.json "
        f"(results_at={results_at}, report_at={report_at})"
    )
if "def write_results_next_to_report" not in src:
    raise SystemExit("write_results_next_to_report missing")
print("harbor runner tree: summarize writes results.json before report.json")
PY

echo "harbor runner tree on tip emits results.json (tbench-harbor-v1)"
