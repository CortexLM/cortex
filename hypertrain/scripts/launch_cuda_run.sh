#!/usr/bin/env bash
# One-shot CUDA run launch on 8x RTX 5090 (Vast): admit -> deps/seed staging -> CUDA qualification
# -> rescue -> roots -> backend bootstrap -> POST /v2/admin/runs (backend cuda) -> dispatch.json
# -> master reload -> auditors -> relays -> miner host -> round open -> miner loop -> hold.
#
#   scripts/launch_cuda_run.sh --dry-run    # preflight + miner bundle build; no provider call
#   TAG=v1 MINER_SSH="ssh5.vast.ai 12345" setsid nohup scripts/launch_cuda_run.sh --yes-spend \
#     > /data/role-dispatch/launch-v1.log 2>&1 < /dev/null &
#
# Idempotent: each stage writes $STATE/<stage>.done and is skipped on re-run; failures print
# "FAIL <stage>: <reason>" and exit non-zero. The network-v2 supervisor binds the rented hosts to
# THIS process and deletes them when it exits, so admit..rescue cannot resume under a new parent:
# a crash there needs a fresh TAG. After launch the process holds until the cutoff so the hosts
# stay up for master-side CUDA audit replay.
set -euo pipefail
umask 077

TAG=${TAG:-v1}
RD=${RD:-/data/role-dispatch}
E2E=${E2E:-/root/e2e-deploy-wt/hypertrain-e2e-b/.e2e}
OUT=${OUT:-$RD/manifest/out-$TAG}
JOB=${JOB:-$RD/manifest/job-$TAG}
PREP=${PREP:-$RD/prep-$TAG}
RUN=${RUN:-$RD/run-$TAG}
STATE=${STATE:-$RD/launch-$TAG}
MASTER=${MASTER:-cortex-challenge-hypertrain}
PY=${PY:-$RD/runtime/venv/bin/python}
MINER_SRC=${MINER_SRC:-/data/pydeps/hypertrain}
REPO=${REPO:-$(cd "$(dirname "$0")/.." && pwd)}
FIX_BASE=${FIX_BASE:-1a8f91e}
T=${TOOLS:-$REPO/scripts/ops}
MINER_SSH=${MINER_SSH:-}
MINER_KEY=${MINER_KEY:-}
# Signed floor: cutoff >= admitted + staging 1800 s + qualification 9600 s + hard margin 1800 s;
# task8-deps stops staging at cutoff - (qualification + hard margin), so staging needs its own 1800 s.
WINDOW=${WINDOW:-13200}
HOTKEY_SEED=${HOTKEY_SEED:-$RD/keys/uid247-hotkey.seed}
COORD_KEY=${COORD_KEY:-/root/.config/cortex-secrets/s100-master/challenges/hypertrain/coord.key}
PROXY_TOKEN=${PROXY_TOKEN:-/root/.config/cortex-secrets/od-large-e2e-miners/proxy.token}
PUBLIC=${PUBLIC:-185.8.107.43}
W=${W:-0}
# Tighter offsets fail with "no future replay budget": opening builds the start state on CPU.
OFFSETS=${OFFSETS:-150,300,900,1000,1020,1100}
GENESIS=1692803367
PINS="fastapi==0.142.2 uvicorn==0.54.0 httpx==0.28.1 pydantic==2.13.5 py-sr25519-bindings==0.2.4 \
numpy==2.5.3 safetensors==0.8.0 py-ecc==8.0.0 pyarrow==25.0.1 tokenizers==0.23.2"

MODE=${1:-}
case "$MODE" in --dry-run | --yes-spend) ;; *) echo "usage: $0 --dry-run | --yes-spend" >&2; exit 2 ;; esac

log() { echo "$(date -u +%FT%TZ) $*"; }
die() { log "FAIL $STAGE: $*"; exit 1; }
done_() { : > "$STATE/$STAGE.done"; log "STAGE_OK $STAGE"; }
stage() {
  STAGE=$1
  if [ -e "$STATE/$STAGE.done" ]; then log "STAGE_SKIP $STAGE"; return 1; fi
  log "STAGE $STAGE"
}
as_runtime() { setpriv --reuid=65532 --regid=65532 --clear-groups "$@"; }
J() { "$PY" -c "import json,sys;d=json.load(open(sys.argv[1]));print(eval(sys.argv[2],{'d':d}))" "$@"; }
beacon_round() { echo $((($(date +%s) - GENESIS) / 3 + 1)); }
mexec() { docker exec -i "$MASTER" "$@"; }
healthy() { mexec python -c 'import httpx;assert httpx.get("http://127.0.0.1:8000/health").status_code==200' 2> /dev/null; }
rescued() { "$PY" -c "import json,sys;sys.exit(0 if any(r.get('kind')=='network_rescued' and r.get('role')==sys.argv[2] for r in map(json.loads,open(sys.argv[1]))) else 1)" "$RUN/journal.jsonl" "$1"; }
mssh() {
  ssh -i "$MINER_KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=20 \
    -o StrictHostKeyChecking="${MINER_HOSTKEY_POLICY:-accept-new}" -o UserKnownHostsFile="${MINER_KNOWN_HOSTS:-$STATE/miner_known_hosts}" \
    -p "${MINER_SSH#* }" "root@${MINER_SSH%% *}" "$@"
}

export PYTHONHOME=$RD/runtime/local CUDA_VISIBLE_DEVICES='' HOME=$RD
# Controller tree: the frozen snapshot admission used (same launcher/orchestrate as every stage).
HT_TREE=${HT_TREE:-$("$PY" -c 'import json,sys;print(json.load(open(sys.argv[1]))["snapshot"])' "$PREP/freeze/freeze.json")/tree}
[ -f "$HT_TREE/experiments/gpu_network_v2/orchestrate.py" ] || { echo "frozen tree missing: $HT_TREE" >&2; exit 1; }
export HT_TREE
export PYTHONPATH=$HT_TREE/src:$HT_TREE/scripts:$HT_TREE:/data/pydeps:$RD/runtime/venv/lib/python3.12/site-packages
mkdir -p "$STATE"

STAGE=preflight
for f in "$T"/{prepare_record.py,task8-deps.py,hf_seed.py,rescue_parallel.py,rescue_dedup.py} \
  "$T"/{task8-materialize-roots.py,prepare_runtime.py} \
  "$T"/{backend_bootstrap.py,live_operator.py,auditors.sh} \
  "$E2E/beacon/pusher.py" "$E2E/relays/build_relay_configs.py" "$E2E/relays/deploy_relay.sh" \
  "$OUT/manifest-envelope.json" "$OUT/manifest.json" "$OUT/profile.json" "$JOB/job.json" \
  "$PREP/base/private/review-base.json" "$PREP/base/private/lifecycle-config.json" \
  "$PREP/context/join.json" "$PREP/context/seed_context.json" "$PREP/evidence/evidence.json" \
  "$RD/keys/vast.key" "$RD/keys/owner.seed" "$HOTKEY_SEED" "$PROXY_TOKEN" "$COORD_KEY" "$RD/dispatch.json"; do
  [ -e "$f" ] || die "missing $f"
done
# materialize refuses existing roots: fail here, before any spend, not an hour into the run.
[ -e "$STATE/roots.done" ] || for r in h0 h1; do
  [ ! -e "/data/hypertrain-$r" ] || die "/data/hypertrain-$r exists: archive it after checking no run reads it"
done
RID=$(J "$OUT/manifest-envelope.json" "d['run_id']")
[ "$(J "$JOB/job.json" "d['run_id']")" = "$RID" ] || die "job-$TAG binds another run than out-$TAG"
[ "$(J "$OUT/profile.json" "(d['admission']['gpu_name'],d['admission']['gpus_per_host'],d['admission']['sm_count'])")" \
  = "('RTX 5090', 8, 170)" ] || die "profile is not 8x RTX 5090 / 170 SM"
[ "$(J "$OUT/manifest.json" "d['training']['reference_spec']['layout']['n_gpus']")" = 8 ] \
  || die "manifest layout is not 8 GPUs"
IMAGE=$(J "$OUT/manifest.json" "d['training']['reference_spec']['image_digest']")
ALLOW=" $(J "$OUT/manifest.json" "' '.join(d['training']['reference_spec']['driver_allowlist'])") "
BASE_URL=$(J "$PREP/base/private/lifecycle-config.json" "d['base_url']")
HOTKEY=$(as_runtime "$PY" -c 'import pathlib,sys;from hypertrain.miner.core import load_keyfile;print(load_keyfile(pathlib.Path(sys.argv[1])).ss58)' "$HOTKEY_SEED") \
  || die "cannot load miner hotkey seed $HOTKEY_SEED"
[ "$BASE_URL" = https://console.vast.ai ] || die "lifecycle base_url is $BASE_URL, expected Vast"
JD=$(J "$JOB/job.json" "d['deadline']")
healthy || die "master $MASTER unhealthy"
ss -ltn | grep -q ':18990 ' || die "miner proxy :18990 not listening"
rc=0
mexec python - "$RID" << 'EOF' || rc=$?
import pathlib, sqlite3, sys
p = pathlib.Path("/data/runs") / sys.argv[1] / "challenge.db"
row = p.exists() and sqlite3.connect(f"file:{p}?mode=ro", uri=True).execute(
    "SELECT data FROM records_v2 WHERE kind='execution-backend' AND id='run'").fetchone()
sys.exit(0 if not row else 3 if '"cuda"' in row[0] else 4)
EOF
case $rc in 0) BACKEND=unregistered ;; 3) BACKEND=cuda ;; 4) die "run ${RID:0:8} is frozen on cpu: use a new TAG" ;; *) die "backend probe rc=$rc" ;; esac

# Miner code = what the master runs + this repo's transport fixes (Content-Length PUT, 5xx retry,
# parallel part fetch); built locally so --dry-run proves the patch applies.
BUNDLE=$STATE/bundle
rm -rf "$BUNDLE" && mkdir -p "$BUNDLE" && cp -a "$MINER_SRC" "$BUNDLE/hypertrain"
find "$BUNDLE" -name __pycache__ -prune -exec rm -rf {} +
# Diff FIX_BASE against the working tree (not HEAD): uncommitted miner changes ship too.
# The object upload is a PUT in this tree and a signed POST once the body-signed upload lands; either
# call site is the retry. Both the "patch or not" decision and the final check ask this one function.
has_upload_retry() { grep -q -e '_retrying("POST object' -e '_retrying("PUT object' "$BUNDLE/hypertrain/miner/core.py"; }
if ! has_upload_retry; then
  git -C "$REPO" diff "$FIX_BASE" -- src/hypertrain/miner/core.py src/hypertrain/data/store.py \
    | patch -p3 --forward --quiet -d "$BUNDLE/hypertrain" || die "transport fixes do not apply onto $MINER_SRC"
fi
has_upload_retry || die "bundle lacks the upload retry fix"
grep -q 'def _parts' "$BUNDLE/hypertrain/data/store.py" || die "bundle lacks the parallel part fetch"
grep -q '^UPLOAD_WORKERS = ' "$BUNDLE/hypertrain/miner/core.py" || die "bundle lacks the parallel part PUT"
grep -q 'def _local_input' "$BUNDLE/hypertrain/miner/core.py" || die "bundle lacks the theta-only input reuse"
# store.py has no master-side divergence: the bundle copy must be the working tree's bytes.
cmp -s "$REPO/src/hypertrain/data/store.py" "$BUNDLE/hypertrain/data/store.py" \
  || die "bundle data/store.py differs from the working tree"
log "PREFLIGHT_OK run=$RID image=$IMAGE job_deadline=$JD backend=$BACKEND miner=${MINER_SSH:-unset}"
if [ "$MODE" = --dry-run ]; then log "DRY_RUN_DONE no provider call, nothing spent"; exit 0; fi
# Runtime uid 65532 and the master container (sees /data only) cannot read a /root checkout:
# run a frozen copy of the tools from /data.
if [ "${T#"$RD"/}" = "$T" ]; then
  TS=$RD/ops-$TAG
  install -d -o 65532 -g 65532 -m 0700 "$TS"
  for f in "$T"/*.py "$T"/*.sh; do install -o 65532 -g 65532 -m 0700 "$f" "$TS/"; done
  T=$TS
fi

if [ ! -e "$STATE/window" ]; then
  A=$(date +%s); CUT=$((A + WINDOW)); [ "$CUT" -le "$JD" ] || CUT=$JD
  echo "$A $CUT" > "$STATE/window"
fi
read -r A CUT < "$STATE/window"
# Abort = kill -TERM "$(cat $STATE/launcher.pid)": the supervisor tracks this pid (INT/HUP are ignored).
echo $$ > "$STATE/launcher.pid"

STAGE=beacon
# The master clock only advances on POST /v1/admin/beacon; the pusher dies on ReadTimeout while
# the master is busy building start states, so it runs in a restart loop until the cutoff.
if ! { [ -s "$STATE/beacon.pid" ] && kill -0 "$(cat "$STATE/beacon.pid")" 2> /dev/null; }; then
  setsid bash -c "while [ \$(date +%s) -lt $CUT ]; do docker exec -i $MASTER python - --cutoff $CUT \
    < '$E2E/beacon/pusher.py' >> '$STATE/beacon.log' 2>&1; sleep 3; done" < /dev/null > /dev/null 2>&1 &
  echo $! > "$STATE/beacon.pid"
fi
log "STAGE_OK beacon pid=$(cat "$STATE/beacon.pid") cutoff=$CUT"

CTX=(--file "join_h0=$PREP/context/join.json" --file "join_h1=$PREP/context/join.json"
  --file "seed_context_h0=$PREP/context/seed_context.json"
  --file "seed_context_h1=$PREP/context/seed_context.json"
  --file "evidence=$PREP/evidence/evidence.json")
COMMON=(--base "$PREP/base/private/review-base.json" --parent-pid "$$" --owner-seed "$RD/keys/owner.seed"
  --run-id "$RID" --provider-key-file "$RD/keys/vast.key" --run-dir "$RUN"
  --source-message "Owner order 2026-10-14: 8x RTX 5090 Vast CUDA run, admission to mining authorized"
  --live --cutoff-unix "$CUT" --admitted-unix "$A")
LIVE=$PREP/live-$TAG
if stage admit; then
  [ ! -e "$RUN/journal.jsonl" ] || die "$RUN already holds a transaction of another parent: use a new TAG"
  [ $((A + 1800 + 9600 + 1800)) -le "$CUT" ] || die "window too short A=$A CUT=$CUT (job deadline $JD)"
  install -d -o 65532 -g 65532 -m 0700 "$RUN" "$LIVE"
  as_runtime "$PY" "$T/prepare_record.py" "${COMMON[@]}" --action admit --beacon "$(beacon_round)" \
    --output "$LIVE/review-admit.json" "${CTX[@]}" > "$STATE/admit.out" 2>&1 || die "see $STATE/admit.out"
  done_
fi
if stage deps; then
  as_runtime "$PY" "$T/task8-deps.py" "$RUN" "$PREP/base/private/review-base.json" > "$STATE/deps.out" 2>&1 \
    || die "see $STATE/deps.out"
  done_
fi
if stage qualify; then
  as_runtime "$PY" "$T/prepare_record.py" "${COMMON[@]}" --action qualification --beacon "$(beacon_round)" \
    --output "$LIVE/review-qual.json" "${CTX[@]}" > "$STATE/qual.out" 2>&1 || die "see $STATE/qual.out"
  done_
fi
if stage rescue; then
  pids=()
  for r in h0 h1; do
    rescued "$r" && continue
    port=18560; [ "$r" = h1 ] && port=18600  # 16 workers bind base..base+15 each
    as_runtime "$PY" "$T/rescue_parallel.py" --run-dir "$RUN" --role "$r" --via range --workers 16 \
      --public-ip "$PUBLIC" --base-port "$port" > "$STATE/rescue-$r.out" 2>&1 &
    pids+=($!)
  done
  for p in "${pids[@]}"; do wait "$p" || die "rescue failed, see $STATE/rescue-h*.out"; done
  done_
fi
if stage roots; then
  as_runtime "$PY" "$T/task8-materialize-roots.py" "$RUN" "$PREP/base/private/review-base.json" h0 h1 \
    > "$STATE/materialize.out" 2>&1 || true
  [ "$(grep -c '^MATERIALIZED ' "$STATE/materialize.out")" = 2 ] || die "see $STATE/materialize.out"
  done_
fi
if stage bootstrap; then
  RES=()
  for r in h0 h1; do RES+=("$(J "/data/hypertrain-$r/qualification.json" "d['output']")/result.json"); done
  mexec python -B "$T/backend_bootstrap.py" --manifest-envelope "$OUT/manifest-envelope.json" \
    --owner-seed "$RD/keys/owner.seed" \
    --config /data/hypertrain-h0/qualification.json /data/hypertrain-h1/qualification.json \
    --result "${RES[@]}" > "$STATE/bootstrap.out" 2>&1 || die "see $STATE/bootstrap.out"
  done_
fi
if stage register; then
  for p in "$OUT"/policy-*.json; do
    mexec python -c 'import pathlib,sys;from hypertrain.data.store import LocalFSStore;LocalFSStore(pathlib.Path("/data/objects")).put(sys.stdin.buffer.read())' \
      < "$p" || die "policy object $p"
  done
  mexec python -c '
import os, sys, httpx
tok = open(os.environ["CHALLENGE_ADMIN_TOKEN_FILE"]).read().strip()
r = httpx.post("http://127.0.0.1:8000/v2/admin/runs", content=sys.stdin.buffer.read(), timeout=300,
               headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json"})
print("POST_STATUS", r.status_code, r.text[:400])
sys.exit(0 if r.status_code in (200, 201) or (r.status_code == 409 and "already exists" in r.text) else 1)
' < "$OUT/manifest-envelope.json" > "$STATE/register.out" 2>&1 || die "see $STATE/register.out"
  mexec python -c '
import sqlite3, sys
db = sqlite3.connect(f"file:/data/runs/{sys.argv[1]}/challenge.db?mode=ro", uri=True)
row = db.execute("SELECT data FROM records_v2 WHERE kind=\"execution-backend\" AND id=\"run\"").fetchone()
sys.exit(0 if row and "\"cuda\"" in row[0] else 1)
' "$RID" || die "registered run is not frozen on the CUDA backend"
  done_
fi
if stage dispatch; then
  "$PY" - "$RD/dispatch.json" "$RUN" "$RID" "$HOTKEY" << 'EOF' || die "dispatch.json rewrite"
import json, os, sys
path, run, rid, hotkey = sys.argv[1:]
cfg = json.load(open(path))
cfg.update(run_dir=run, run_id=rid, roles={hotkey: "h0"})
tmp = path + ".tmp"
with open(tmp, "w") as f:
    json.dump(cfg, f, separators=(",", ":"))
os.chown(tmp, 65532, 65532)
os.chmod(tmp, 0o600)
os.replace(tmp, path)
EOF
  # create_app loads HYPERTRAIN_ROLE_DISPATCH once; the new binding needs a process restart.
  docker restart "$MASTER" > /dev/null
  for _ in $(seq 90); do healthy && break; sleep 2; done
  healthy || die "master not healthy after reload"
  done_
fi
if stage auditors; then
  { "$T/auditors.sh" stop && "$T/auditors.sh" start "$RID" && "$T/auditors.sh" check "$RID"; } \
    > "$STATE/auditors.out" 2>&1 || die "see $STATE/auditors.out"
  done_
fi
if stage relays; then
  R=$STATE/relays
  mkdir -p "$R/prep"
  cp "$OUT/manifest.json" "$OUT/policy-relay_registry_hash.json" "$R/prep/"
  if [ ! -e "$R/out/relay-de.json" ]; then
    "$PY" "$E2E/relays/build_relay_configs.py" --run-id "$RID" --prep "$R/prep" --coord-key "$COORD_KEY" \
      --master-url https://gateway.cortex.foundation/challenge/hypertrain --out-dir "$R/out" \
      > "$R/build.out" 2>&1 || die "see $R/build.out"
  fi
  for pair in de:185.191.119.141 us:185.8.107.22 sg:88.216.191.197; do
    region=${pair%%:*} ip=${pair#*:} cfg=$R/out/relay-$region.json
    rs=(ssh -i /root/.ssh/ascii_box_ed25519 -o BatchMode=yes -o ConnectTimeout=15 -o IdentitiesOnly=yes "root@$ip")
    if [ "$("${rs[@]}" sha256sum /etc/hypertrain/relay/relay.json | cut -c1-64)" = "$(sha256sum "$cfg" | cut -c1-64)" ] \
      && "${rs[@]}" docker ps -q --filter 'name=^hypertrain-relay$' | grep -q .; then continue; fi
    "${rs[@]}" docker rm -f hypertrain-relay > /dev/null 2>&1 || true
    bash "$E2E/relays/deploy_relay.sh" "$region" "$ip" "$cfg" >> "$R/deploy.out" 2>&1 \
      || die "relay $region, see $R/deploy.out"
  done
  done_
fi
if stage miner; then
  if [ -z "$MINER_SSH" ]; then  # default: mine on qualified host h1 (8x5090, staged, allowlisted)
    MINER_SSH=$("$PY" -c "import json,sys;r=[x for x in map(json.loads,open(sys.argv[1])) if x.get('kind')=='instance_ready' and x.get('role')=='h1'][-1];print(r['ssh_host'],r['ssh_port'])" "$RUN/journal.jsonl") \
      || die "no h1 instance_ready in $RUN/journal.jsonl"
    MINER_KEY=${MINER_KEY:-$RUN/id_ed25519}
    MINER_HOSTKEY_POLICY=yes MINER_KNOWN_HOSTS=$RUN/known_hosts-h1  # pinned at admission
  fi
  MINER_KEY=${MINER_KEY:-$RD/vast-m5/id_ed25519}
  echo "$MINER_SSH" > "$STATE/miner_ssh"
  mssh "nvidia-smi --query-gpu=name,driver_version --format=csv,noheader" > "$STATE/miner-gpus.txt" \
    || die "miner ssh to $MINER_SSH"
  [ "$(grep -c 'RTX 5090' "$STATE/miner-gpus.txt")" = 8 ] \
    || die "miner host is not 8x RTX 5090: $(sort -u "$STATE/miner-gpus.txt" | tr '\n' ';')"
  DRV=$(head -1 "$STATE/miner-gpus.txt" | awk -F', ' '{print $2}')
  case "$ALLOW" in *" $DRV "*) ;; *) die "miner driver $DRV not in allowlist$ALLOW" ;; esac
  tar -C "$BUNDLE" -czf - hypertrain \
    | mssh 'rm -rf /opt/ht && mkdir -p /opt/ht /opt/miner/work && tar -C /opt/ht -xzf -' || die "bundle upload"
  mssh 'install -m 0600 /dev/stdin /opt/miner/hotkey.seed' < "$HOTKEY_SEED" || die "hotkey upload"
  API="http://$PUBLIC:18990/$(cat "$PROXY_TOKEN")"
  mssh 'cat > /opt/miner/miner.toml' << EOF || die "miner.toml upload"
api = "$API"
keyfile = "/opt/miner/hotkey.seed"
workdir = "/opt/miner/work"
state_source = "$API/v2/runs/$RID/objects"
image_digest = "$IMAGE"
run_id = "$RID"
owner_hotkey = "$(J "$OUT/manifest-envelope.json" "d['signer']")"
device = "cuda"
EOF
  # island_worker refuses to train unless HT_IMAGE_DIGEST names the manifest image; run-v2 exits
  # "authenticated assignment not ready" until d_assign, hence the retry loop up to the cutoff.
  mssh 'cat > /opt/miner/loop.sh' << EOF || die "loop.sh upload"
cd /opt/ht
export PYTHONPATH=/opt/ht CUBLAS_WORKSPACE_CONFIG=:4096:8 CUDA_DISABLE_PTX_JIT=1 HT_IMAGE_DIGEST=$IMAGE
rc=1
while [ \$(date +%s) -lt $CUT ]; do
  /venv/main/bin/python -m hypertrain.miner.cli run-v2 --config /opt/miner/miner.toml --round $W >> /opt/miner/run$W.log 2>&1; rc=\$?
  echo "LOOP rc=\$rc \$(date -u +%T)" >> /opt/miner/run$W.log
  [ \$rc -eq 0 ] && break
  sleep 10
done
echo "LOOP_END rc=\$rc" >> /opt/miner/run$W.log
EOF
  mssh "cd /opt/ht && { PYTHONPATH=/opt/ht /venv/main/bin/python -c 'import hypertrain.miner.cli' 2> /dev/null \
    || /venv/main/bin/python -m pip install -q $PINS; }" > "$STATE/miner-pip.out" 2>&1 || die "see $STATE/miner-pip.out"
  mssh "cd /opt/ht && PYTHONPATH=/opt/ht /venv/main/bin/python -c 'import torch, hypertrain.miner.cli; \
assert torch.__version__ == \"2.14.0+cu130\" and torch.cuda.device_count() == 8, torch.__version__'" \
    || die "miner host needs torch 2.14.0+cu130 in /venv/main with 8 visible GPUs"
  done_
fi
if stage open; then
  if ! mexec python -c 'import httpx,sys;sys.exit(httpx.get(f"http://127.0.0.1:8000/v2/runs/{sys.argv[1]}/rounds/{sys.argv[2]}").status_code!=200)' \
    "$RID" "$W"; then
    mexec sh -c "python -B $T/live_operator.py --master http://127.0.0.1:8000 --run-id $RID \
      --state-dir /data/runs/$RID --coord-key \$HYPERTRAIN_COORD_KEY_FILE \
      --admin-token-file \$CHALLENGE_ADMIN_TOKEN_FILE open --w $W --hotkey $HOTKEY --offsets $OFFSETS" \
      > "$STATE/open.out" 2>&1 || die "see $STATE/open.out"
  fi
  done_
fi
if stage minerstart; then
  [ -n "$MINER_SSH" ] || MINER_SSH=$(cat "$STATE/miner_ssh")
  MINER_KEY=${MINER_KEY:-$RUN/id_ed25519}
  # [/] keeps pgrep from matching the remote shell whose own argv contains the pattern.
  mssh "pgrep -f '[/]opt/miner/loop.sh' > /dev/null || { setsid nohup bash /opt/miner/loop.sh > /dev/null 2>&1 < /dev/null & }" \
    || die "miner loop start"
  mssh "sleep 2; pgrep -f '[/]opt/miner/loop.sh' > /dev/null" || die "miner loop is not running on $MINER_SSH"
  done_
fi

log "LAUNCHED run=$RID round=$W miner=$MINER_SSH; holding the host lease until cutoff $CUT"
n=$(date +%s)
if [ "$CUT" -gt "$n" ]; then sleep $((CUT - n)); fi
log PARENT_RELEASED
