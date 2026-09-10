#!/bin/bash
# Run adaptor unit tests (no Harbor, no podman, no Firecracker).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
python3 "$HERE/test_resolve_agent.py"
python3 "$HERE/test_summarize.py"
python3 "$HERE/test_inspect_scan.py"
bash "$HERE/test_adaptor.sh"
echo "rlm_fc_in_guest_harbor tests: all passed"
