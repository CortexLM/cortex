#!/usr/bin/env bash
# Contract-2 auditor clients for the s100 TEST challenge: hypertrain-test-auditor-0/1 (slot 0/1 of
# the run's training.auditors) on network hypertrain-test_test. Disposable TEST seeds only, under
# /data/hypertrain-test/auditors; never the production hypertrain-auditor-* containers or
# /data/role-dispatch seeds (scripts/ops/auditors.sh). The replay itself runs in the challenge
# (store.execute_audit_v2), so the server image is enough.
# Run on s100 over stdin: ssh root@s100 'bash -s -- CMD ARGS' < test_auditors.sh
#   check IMAGE SS58_0 SS58_1   each staged seed must derive the pinned auditor of its slot
#   start IMAGE RUN_ID          create and start both containers (refuses if one exists)
#   stop | remove               stop both | stop, then remove both with their anonymous volumes
# Stage a seed without printing it: ssh root@s100 'cat > /dev/null' style, see the runbook.
set -euo pipefail
BASE=/data/hypertrain-test/auditors; NET=hypertrain-test_test; API=http://challenge:8000
NAMES=(hypertrain-test-auditor-0 hypertrain-test-auditor-1)
cmd=${1:?check|start|stop|remove}
image() { [[ $1 =~ ^ghcr\.io/cortexlm/cortex/hypertrain@sha256:[0-9a-f]{64}$ ]] \
  || { echo "bad image ref" >&2; exit 2; }; }
flags() { echo --network "$NET" --user 65532:65532 --read-only --tmpfs /tmp:size=256m \
  --workdir /tmp --cap-drop ALL --security-opt no-new-privileges:true \
  -v "$BASE/auditor-$1":/state:ro --entrypoint sh; }
case $cmd in
check)
  image "${2:?IMAGE}"; want=("${3:?SS58_0}" "${4:?SS58_1}")
  for i in 0 1; do
    stat -c '%a %u %g' "$BASE/auditor-$i/auditor.seed"
    got=$(docker run --rm --network none --user 65532:65532 --read-only --cap-drop ALL \
      --security-opt no-new-privileges:true -v "$BASE/auditor-$i":/state:ro --entrypoint sh "$2" -c \
      'python -I -c "from pathlib import Path; from hypertrain.miner.core import load_keyfile; print(load_keyfile(Path(\"/state/auditor.seed\")).ss58)"')
    [ "$got" = "${want[$i]}" ] && echo "auditor-$i ok ${got:0:8}" \
      || { echo "auditor-$i MISMATCH"; exit 3; }
  done;;
start)
  image "${2:?IMAGE}"; RID=${3:?RUN_ID}; [[ $RID =~ ^[0-9a-f]{64}$ ]] || { echo "bad run id"; exit 2; }
  for n in "${NAMES[@]}"; do
    if docker inspect "$n" >/dev/null 2>&1; then echo "exists: $n"; exit 4; fi; done
  for i in 0 1; do
    docker run -d --name "${NAMES[$i]}" --restart unless-stopped --log-driver json-file \
      --log-opt max-size=20m --log-opt max-file=3 --label io.cortex.challenge.slug=hypertrain \
      --label io.cortex.task=test-auditors $(flags "$i") "$2" -c "while :; do python -m \
hypertrain.auditor --api $API --run-id $RID --contract 2 --keyfile /state/auditor.seed \
|| echo AUDITOR_POLL_EXIT=\$?; sleep 20; done"
  done
  sleep 3; docker ps -a --filter label=io.cortex.task=test-auditors \
    --format '{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Networks}}';;
stop) docker stop "${NAMES[@]}";;
remove) docker stop "${NAMES[@]}" || true; docker rm -v "${NAMES[@]}";;
*) echo "unknown $cmd"; exit 2;;
esac
