#!/usr/bin/env bash
# Two independent contract-2 auditor clients (auditor-0 -> manifest slot 0, auditor-1 -> slot 1).
# CUDA replay runs master-side (store.execute_audit_v2 -> _role_launch "audit" -> owned host).
# Usage: auditors.sh stage | check RUN_ID | once RUN_ID | start RUN_ID | stop | cleanup
set -euo pipefail
IMG=${HT_IMAGE:-ghcr.io/cortexlm/hypertrain@sha256:58e2b42713a9065758796408e7aea4fb3260ccfaa5ad7bad4859fd87320c93d6}
API=${HT_API:-http://cortex-challenge-hypertrain:8000}
NET=${HT_NET:-cortex-challenges}
SRC=${HT_AUDITOR_SEEDS:-/root/.config/cortex-secrets/od-large-e2e}
BASE=/data/role-dispatch/auditors
NAMES=(hypertrain-auditor-0 hypertrain-auditor-1)
cmd=${1:?stage|check|once|start|stop|cleanup}; RID=${2:-}
need_rid() { [[ $RID =~ ^[0-9a-f]{64}$ ]] || { echo "RUN_ID (64 hex) required" >&2; exit 2; }; }
flags() { echo --network "$NET" --read-only --user 65532:65532 --cap-drop ALL --security-opt no-new-privileges \
  --tmpfs /tmp:size=64m -e PYTHONPATH=/data/pydeps -e HOME=/tmp -v /data/pydeps:/data/pydeps:ro \
  -v "$BASE/auditor-$1":/state:ro --entrypoint sh; }
cli() { echo "python -m hypertrain.auditor --api $API --run-id $RID --contract 2 --keyfile /state/auditor.seed"; }
present() { # exact task-owned containers that exist now; docker failure propagates
  local all n; all=$(docker ps -a --format '{{.Names}}') || exit 1
  for n in "${NAMES[@]}"; do grep -qxF "$n" <<<"$all" && echo "$n"; done; return 0; }
remove_owned() {
  local live; live=$(present) || exit 1
  [[ -z $live ]] || docker rm -f $live || exit 1
  live=$(present) || exit 1
  [[ -z $live ]] || { echo "auditor containers still present: $live" >&2; exit 1; }
  echo "auditor containers absent"; }
case $cmd in
  stage) install -d -o root -g root -m 0755 $BASE
         for i in 0 1; do install -d -o 65532 -g 65532 -m 0700 $BASE/auditor-$i
           install -o 65532 -g 65532 -m 0600 $SRC/auditor-$i.seed $BASE/auditor-$i/auditor.seed; done
         stat -c '%a %u:%g %n' $BASE/auditor-*/auditor.seed ;;
  check) need_rid   # GET only: contract 2 and this identity at its exact manifest slot
         for i in 0 1; do docker run --rm $(flags $i) "$IMG" -c "python - <<'EOF'
import hypertrain.trainer, httpx
from pathlib import Path
from hypertrain.miner.core import load_keyfile
k=load_keyfile(Path('/state/auditor.seed')); r=httpx.get('$API/v2/runs/$RID',timeout=30); r.raise_for_status(); j=r.json()
aud=j['manifest_envelope']['body']['training']['auditors']
assert j['contract']==2, j.get('contract')
assert len(aud)>$i and aud[$i]==k.ss58, ('slot $i', k.ss58, aud)
print('READY auditor-$i', k.ss58, 'slot $i')
EOF"; done ;;
  once)  need_rid   # proof: each identity must settle one job with result MATCH; empty queue fails
         for i in 0 1; do out=$(docker run --rm $(flags $i) "$IMG" -c "$(cli) --once"); echo "$out"
           grep -q '"result": "MATCH"' <<<"$out" || { echo "auditor-$i: no MATCH verdict" >&2; exit 1; }
           echo "auditor-$i MATCH"; done ;;
  start) need_rid   # production loop: re-poll every 20s in-container; a failed poll (transient 5xx while the
         # master is busy) is logged with its exit code and retried, never a container restart
         for i in 0 1; do docker run -d --name ${NAMES[$i]} --restart always --label io.cortex.task=auditors \
           --log-opt max-size=20m $(flags $i) "$IMG" -c "while :; do $(cli) || echo AUDITOR_POLL_EXIT=\$?; sleep 20; done"; done ;;
  stop)  remove_owned ;;
  cleanup) remove_owned
         rm -f $BASE/auditor-0/auditor.seed $BASE/auditor-1/auditor.seed
         rmdir $BASE/auditor-0 $BASE/auditor-1 $BASE
         test ! -e $BASE && echo "auditors cleanup: seeds+state absent" ;;
  *) echo "unknown $cmd" >&2; exit 2 ;;
esac
