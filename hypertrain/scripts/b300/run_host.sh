#!/usr/bin/env bash
# One B300 host run (docs/b300-run.md at the monorepo root): host probe, digest-pinned pull,
# in-container GPU probe, 8-GPU MFU bench with bitwise replay, the miner flow against the test
# challenge, and production rounds with --prod.
# Stops on --duration (default 3h), SIGTERM/SIGINT/SIGHUP or an EC2 Spot interruption notice: it
# stops this run's containers, writes stop.json and uploads the results when --upload is given.
# An unexpected exit takes the same path (reason unexpected-exit).
# Idempotent: a step that passed (steps/<name>.rc = 0) is skipped on re-run unless --force; the
# host probe always runs. Results go to <results-root>/<host>/. Nothing secret is read here: the
# miner config and key directory are bind-mounted by path and never printed.
# shellcheck disable=SC2329  # step functions and on_signal are invoked through step() and trap
set -euo pipefail
# The input checks below are regexes: in en_US.UTF-8 [0-9] and [a-z] match thousands of other code
# points (Arabic-Indic digits, accented letters), and the arithmetic that follows only prints an
# error and goes on.
export LC_ALL=C

DEFAULT_IMAGE=ghcr.io/cortexlm/cortex/hypertrain-gpu@sha256:f4b2885f2dc89478da645b9f5e6f02891cd3309ef0a1f2aa0de04328e11cba0f
HT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
IMDS=${HT_IMDS_URL:-http://169.254.169.254}
POLL=${HT_B300_POLL_SECONDS:-5}
EXPECT_DRIVER=${HT_B300_EXPECT_DRIVER:-595.91.07}
EXPECT_GPUS=${HT_B300_EXPECT_GPUS:-8}
EXPECT_CC=${HT_B300_EXPECT_CC:-10.3}
MIN_EFA=${HT_B300_MIN_EFA:-1}
EFA_SYSFS=${HT_B300_EFA_SYSFS:-/sys/class/infiniband}
MIN_DISK_GB=${HT_B300_MIN_DISK_GB:-100}
STOP_GRACE=${HT_B300_STOP_GRACE:-60}
SMOKE_BUDGET=${HT_B300_SMOKE_BUDGET:-900}
PEAK=${HT_B300_PEAK_FLOPS:-}
PREFIX=${HT_B300_CONTAINER_PREFIX:-ht-b300}
ALL_STEPS="host_probe pull gpu_probe mfu miner_test miner_prod"
# What --upload copies out of the results directory. Everything else there (the lock, the IMDS
# scratch file, anything a miner or an operator left behind) stays on the host.
UPLOAD_INCLUDE=(
  run.log steps.tsv stop.json spot-instance-action.json
  nvidia-smi.csv nvidia-smi.txt efa-devices.txt fi_info.txt df.txt host_probe.json
  pull.log pull-digests.json gpu_probe.json 'mfu-dp*-flash.json'
  mine-smoke-v2.json mine-smoke-v2.stdout 'steps/*.rc' 'miner-prod/round-*.json'
)

usage() {
  cat <<'EOF'
usage: run_host.sh [--image REPO@sha256:<64 hex>] [--results-root DIR] [--host NAME]
                   [--duration 3h|90m|SECONDS] [--steps "host_probe pull ..."] [--force]
                   [--upload s3://bucket/prefix] [--prod]
--prod adds production rounds; it needs HT_B300_PROD_ENDPOINT (master challenge URL),
HT_B300_MINER_CONFIG (miner.toml path) and HT_B300_PROD_ROUNDS (round numbers).
EOF
}

image=$DEFAULT_IMAGE results_root=/results host=$(hostname -s) duration=3h
steps=$ALL_STEPS force=0 prod=0 upload=
while [ $# -gt 0 ]; do
  case $1 in
    --image) image=$2; shift 2 ;;
    --results-root) results_root=$2; shift 2 ;;
    --host) host=$2; shift 2 ;;
    --duration) duration=$2; shift 2 ;;
    --steps) steps=$2; shift 2 ;;
    --upload) upload=$2; shift 2 ;;
    --force) force=1; shift ;;
    --prod) prod=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

die() { echo "run_host: $*" >&2; exit 2; }
((BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1))) ||
  die "bash 5.1 or newer is needed (wait -n -p): $BASH_VERSION"
# Operator text is matched against a regex before it reaches arithmetic or a command line: bash
# reads 08 as an invalid octal number and evaluates $(...) inside $(( )), so a number is never
# computed from an unchecked string and every conversion is forced to base 10 (10#).
# aws joins its --exclude/--include patterns to the source directory without escaping it, so a
# [1], ? or * in the results root defeats the upload allowlist (everything would be uploaded); a
# colon is a docker bind-mount separator; a relative root is a docker volume name.
[[ $results_root =~ ^/[A-Za-z0-9._/+@=,-]*$ ]] ||
  die "--results-root must be an absolute path of letters, digits and ._/+@=,-: $results_root"
[[ $image =~ ^[a-z0-9][a-z0-9./_-]*@sha256:[0-9a-f]{64}$ ]] || die "image must be REPO@sha256:<64 hex>: $image"
[[ $host =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || die "bad host name: $host"
[[ $duration =~ ^([0-9]{1,6})([hm]?)$ ]] || die "bad --duration (3h, 90m or seconds): $duration"
case ${BASH_REMATCH[2]} in
  h) unit=3600 ;;
  m) unit=60 ;;
  *) unit=1 ;;
esac
seconds=$((10#${BASH_REMATCH[1]} * unit))
[ "$seconds" -gt 0 ] || die "bad --duration (not positive): $duration"
# The poll interval is 0.001 to 59.999 s: it must stay well inside the 2-minute Spot notice.
[[ $POLL =~ ^([0-9]|[1-5][0-9])(\.[0-9]{1,3})?$ && $POLL =~ [1-9] ]] ||
  die "HT_B300_POLL_SECONDS must be 0.001 to 59.999 seconds: $POLL"
[[ $STOP_GRACE =~ ^[0-9]{1,4}$ ]] || die "HT_B300_STOP_GRACE must be whole seconds, at most 9999: $STOP_GRACE"
STOP_GRACE=$((10#$STOP_GRACE))
[[ $EXPECT_GPUS =~ ^[1-9][0-9]?$ ]] || die "HT_B300_EXPECT_GPUS must be 1 to 99: $EXPECT_GPUS"
[[ $PREFIX =~ ^ht-[a-z0-9][a-z0-9_.-]*$ ]] || die "HT_B300_CONTAINER_PREFIX must start with ht-: $PREFIX"
for s in $steps; do [[ " $ALL_STEPS " == *" $s "* ]] || die "unknown step: $s"; done
out=$results_root/$host
if [ "$prod" = 1 ]; then
  [ -n "${HT_B300_PROD_ENDPOINT:-}" ] || die "--prod needs HT_B300_PROD_ENDPOINT"
  [ -f "${HT_B300_MINER_CONFIG:-}" ] || die "--prod needs HT_B300_MINER_CONFIG (a miner.toml path)"
  [[ ${HT_B300_PROD_ROUNDS:-} =~ ^[0-9]+( [0-9]+)*$ ]] || die "--prod needs HT_B300_PROD_ROUNDS"
  # The results directory is mounted at /out and uploaded: the miner directory (miner.toml, key
  # file, workdir) must be neither inside it nor above it.
  miner_dir=$(realpath -m -- "$(dirname -- "$HT_B300_MINER_CONFIG")")
  out_real=$(realpath -m -- "$out")
  case $miner_dir/ in "$out_real"/*) die "HT_B300_MINER_CONFIG is inside the results directory $out" ;; esac
  case $out_real/ in "$miner_dir"/*) die "HT_B300_MINER_CONFIG is above the results directory $out" ;; esac
else
  steps=${steps//miner_prod/}
fi
[ -z "$upload" ] || [[ $upload =~ ^s3://[a-z0-9.-]+(/[A-Za-z0-9._/-]*)?$ ]] || die "bad --upload: $upload"

mkdir -p "$out/steps"
exec > >(tee -a "$out/run.log") 2>&1
exec 9>"$out/.lock"
flock -n 9 || die "another run_host.sh holds $out/.lock"
# State of an earlier run that this run would otherwise report as its own.
rm -f -- "$out/.stop-reason" "$out/.instance-action" "$out/spot-instance-action.json" 2>/dev/null || true
# The ubuntu user of the AWS hosts is not in the docker group: fall back to passwordless sudo.
if [ -n "${DOCKER:-}" ]; then
  docker_cmd=("$DOCKER")
elif docker info >/dev/null 2>&1; then
  docker_cmd=(docker)
elif sudo -n docker info >/dev/null 2>&1; then
  docker_cmd=(sudo -n docker)
else
  die "docker is not usable, directly or through sudo -n"
fi
dk() { "${docker_cmd[@]}" "$@"; }
run_id=$(date -u +%Y%m%dT%H%M%SZ)-$$
deadline=$(($(date +%s) + seconds))
log() { echo "[$(date -u +%H:%M:%S)] $*"; }
# The checkout is bind-mounted into every container: its scripts, and for miner_test its hypertrain
# package, run in place of the image's. Record which commit that is and whether the tree was
# modified (untracked files show in the mount too). A modified or non-git tree is code nobody can
# name: a step that passed with it is never skipped.
rev=$(git -C "$HT" rev-parse HEAD 2>/dev/null) || rev=unknown
dirty=unknown
if [ "$rev" != unknown ] && tree=$(git -C "$HT" status --porcelain -- . 2>/dev/null); then
  if [ -z "$tree" ]; then dirty=0; else dirty=1; fi
fi
case $dirty in
  0) code=$rev ;;
  1) code=$rev+dirty ;;
  *) code=unknown ;;
esac
log "run $run_id host=$host image=$image steps=[$steps] duration=${seconds}s prod=$prod rev=$rev dirty=$dirty"

# --- auto-stop: the deadline or a Spot notice sends SIGTERM to this script --------------------
spot_notice() {
  local session code
  session=$(curl -fsS -m 2 -X PUT "$IMDS/latest/api/token" \
    -H 'X-aws-ec2-metadata-token-ttl-seconds: 300' 2>/dev/null) || session=
  code=$(curl -sS -m 2 -o "$out/.instance-action" -w '%{http_code}' \
    ${session:+-H "X-aws-ec2-metadata-token: $session"} \
    "$IMDS/latest/meta-data/spot/instance-action" 2>/dev/null) || code=000
  [ "$code" = 200 ] && mv "$out/.instance-action" "$out/spot-instance-action.json"
}
watchdog() {
  while sleep "$POLL"; do
    # Left behind by a SIGKILLed script, it would write into a results directory that a newer
    # run may own and TERM a pid that may have been reused.
    kill -0 "$1" 2>/dev/null || return
    if [ "$(date +%s)" -ge "$deadline" ]; then
      echo duration >"$out/.stop-reason" || true
    elif spot_notice; then
      echo spot-interruption >"$out/.stop-reason" || true
    else
      continue
    fi
    kill -TERM "$1"
    return
  done
}
# Its output goes nowhere: a sleep that outlives the script must not hold the caller's pipe.
watchdog $$ 9>&- >/dev/null 2>&1 &
watchdog_pid=$!
log "watchdog pid $watchdog_pid"
bg_pid= # the docker client of the step that is running, killed by finish

upload_results() {
  local filters=(--exclude '*') f
  [ -n "$upload" ] || return 0
  for f in "${UPLOAD_INCLUDE[@]}"; do filters+=(--include "$f"); done
  if ! command -v aws >/dev/null; then
    log "upload skipped: aws CLI not found"
  elif aws s3 cp --recursive --only-show-errors "${filters[@]}" "$out" "${upload%/}/$host/"; then
    log "uploaded to ${upload%/}/$host/"
  else
    log "upload FAILED"
  fi
}
finish() {
  local reason=$1 rc=$2 ids
  # Last thing the script does, whatever called it: no later signal or EXIT trap may interrupt it
  # and no failing command (a run.log that cannot be written, a docker that does not answer) may
  # skip the next one. A group signal (systemd stop, SSM cancel) also kills tee: log straight to
  # the file from here.
  trap '' TERM INT HUP PIPE
  trap - EXIT
  set +e
  exec >>"$out/run.log" 2>&1
  kill "$watchdog_pid" 2>/dev/null || true
  ids=$(dk ps -q --filter "label=io.cortex.b300-run=$run_id" 2>/dev/null) ||
    log "could not list this run's containers: docker ps failed"
  if [ -n "$ids" ]; then
    log "stopping containers ($reason): $(echo "$ids" | tr '\n' ' ')"
    # shellcheck disable=SC2086  # one container id per word
    dk stop --time "$STOP_GRACE" $ids >/dev/null || log "docker stop failed"
  fi
  if [ -n "$bg_pid" ]; then # a docker pull has no container to stop
    pkill -TERM -P "$bg_pid" 2>/dev/null || true
    kill -TERM "$bg_pid" 2>/dev/null || true
  fi
  # A file that cannot be read goes to "errors" in stop.json; it never stops the file or the upload.
  python3 - "$out" "$reason" "$rc" "$run_id" "$image" "$rev" "$dirty" <<'PY' ||
import json, sys
from pathlib import Path
out, reason, rc, run_id, image, rev, dirty = Path(sys.argv[1]), *sys.argv[2:]
errors, steps, rows, notice = [], {}, [], None
for p in sorted((out / "steps").glob("*.rc")):
    try:
        steps[p.stem] = int(p.read_text().split()[0])
    except (OSError, ValueError, IndexError) as e:
        errors.append(f"steps/{p.name}: {type(e).__name__}")
try:
    rows = [r.split("\t") for r in (out / "steps.tsv").read_text().splitlines() if r]
except FileNotFoundError:
    pass
except (OSError, ValueError) as e:
    errors.append(f"steps.tsv: {type(e).__name__}")
try:
    text = (out / "spot-instance-action.json").read_text()
    try:
        notice = json.loads(text)
    except ValueError as e:
        errors.append(f"spot-instance-action.json: {type(e).__name__}")
        notice = {"unparsed": text[:200]}
except FileNotFoundError:
    pass
except (OSError, ValueError) as e:
    errors.append(f"spot-instance-action.json: {type(e).__name__}")
stop = {
    "run_id": run_id, "reason": reason, "rc": int(rc), "image": image,
    "git_rev": rev, "git_dirty": {"0": False, "1": True}.get(dirty),
    "steps": steps, "ran": [r[0] for r in rows if r[3:4] == [run_id]],
    "spot_notice": notice, "errors": errors,
}
(out / "stop.json").write_text(json.dumps(stop, indent=1, sort_keys=True))
print("STOP", json.dumps({k: stop[k] for k in ("reason", "rc", "steps")}, sort_keys=True))
PY
    log "stop.json was not written"
  upload_results
  exit "$rc"
}
on_signal() {
  local reason
  reason=$(cat "$out/.stop-reason" 2>/dev/null || echo signal)
  rm -f "$out/.stop-reason" 2>/dev/null || true
  finish "$reason" 143
}
# $? is 0 when bash runs this for a fatal signal (SIGPIPE from a dead tee, SIGUSR1...): a stop.json
# that says unexpected-exit must not say rc 0.
on_exit() {
  local rc=$?
  finish unexpected-exit "$((rc == 0 ? 1 : rc))"
}
trap on_signal TERM INT HUP
trap on_exit EXIT

# --- step runner -------------------------------------------------------------------------------
# What a step writes under $out, except the files its own shell redirection truncates when it
# starts and the per-round files of miner_prod (removed round by round). It is removed before the
# step runs, together with the step's rc: a step that dies before it writes must not leave the file
# of an earlier run (a gpu_probe.json with ok:true) beside, or an old pass without its file.
step_evidence() {
  case $1 in
    host_probe) echo nvidia-smi.txt efa-devices.txt fi_info.txt df.txt host_probe.json ;;
    pull) echo pull-digests.json ;;
    gpu_probe) echo gpu_probe.json ;;
    mfu) echo "mfu-dp$EXPECT_GPUS-flash.json" ;;
    miner_test) echo mine-smoke-v2.json ;;
  esac
}
step() {
  local name=$1 t0 rc=0 f files
  shift
  [[ " $steps " == *" $name "* ]] || return 0
  if [ "$force" = 0 ] && [ "$name" != host_probe ] && [ "$dirty" = 0 ] &&
    [ "$(cat "$out/steps/$name.rc" 2>/dev/null)" = "0 $image $code" ]; then
    log "skip $name: passed in an earlier run with this image and commit"
    return 0
  fi
  # Without the watchdog neither --duration nor a Spot notice can stop the run. (A watchdog that
  # already sent its TERM is not seen here: the trap runs before the next command.)
  if ! kill -0 "$watchdog_pid" 2>/dev/null; then
    log "the watchdog died: nothing would stop this run at --duration or a Spot notice"
    finish watchdog-died 9
  fi
  read -ra files <<<"$(step_evidence "$name")"
  for f in "${files[@]}" "steps/$name.rc"; do rm -f -- "$out/$f"; done
  log "step $name"
  t0=$(date +%s)
  "$@" || rc=$?
  echo "$rc $image $code" >"$out/steps/$name.rc"
  printf '%s\t%s\t%s\t%s\n' "$name" "$rc" "$(($(date +%s) - t0))" "$run_id" >>"$out/steps.tsv"
  log "step $name rc=$rc"
  return "$rc"
}

# docker runs in the background: a signal then interrupts `wait` instead of waiting for docker.
# Container names are global to the host while the flock covers one results directory, so this
# script never removes a container by name: a name already in use makes `docker run` fail, and
# only `finish` stops containers, by this run's label.
# Waits for the docker client in bg_pid and returns its status. The watchdog is waited on with it:
# if it dies, neither --duration nor a Spot notice can stop the step, so stop the run now rather
# than at the next step. (A watchdog that has sent its TERM never gets here: the trap runs first.)
wait_client() {
  local done_pid='' rc=0
  wait -n -p done_pid "$bg_pid" "$watchdog_pid" || rc=$?
  if [ "$done_pid" = "$watchdog_pid" ]; then
    log "the watchdog died during a step: nothing would stop this run at --duration or a Spot notice"
    finish watchdog-died 9
  fi
  bg_pid=
  return "$rc"
}
container() {
  local name=$PREFIX-$1 rc=0
  shift
  dk run --rm --init --name "$name" --label "io.cortex.b300-run=$run_id" --gpus all \
    --ipc=host --ulimit memlock=-1 --ulimit stack=67108864 \
    -v "$HT:/opt/ht:ro" -v "$out:/out" -w /opt/ht/scripts "$@" 9>&- &
  bg_pid=$!
  wait_client || rc=$?
  return "$rc"
}
PY=/opt/hypertrain/venv/bin/python

# --- steps -------------------------------------------------------------------------------------
host_probe() {
  local docker_root
  nvidia-smi --query-gpu=index,name,driver_version,compute_cap,memory.total,pci.bus_id \
    --format=csv,noheader >"$out/nvidia-smi.csv" || return
  nvidia-smi >"$out/nvidia-smi.txt" 2>&1 || true
  { ls -1 "$EFA_SYSFS" 2>/dev/null || true; } >"$out/efa-devices.txt"
  if command -v fi_info >/dev/null; then fi_info -p efa -t FI_EP_RDM >"$out/fi_info.txt" 2>&1 || true; fi
  docker_root=$(dk info --format '{{.DockerRootDir}}' 2>/dev/null) || docker_root=/var/lib/docker
  df -P -B1G "$results_root" "$docker_root" >"$out/df.txt" 2>/dev/null || df -P -B1G "$results_root" >"$out/df.txt"
  python3 "$HT/scripts/b300/host_check.py" --out "$out/host_probe.json" --nvidia-smi "$out/nvidia-smi.csv" \
    --efa "$out/efa-devices.txt" --df "$out/df.txt" --driver "$EXPECT_DRIVER" --gpus "$EXPECT_GPUS" \
    --cc "$EXPECT_CC" --min-efa "$MIN_EFA" --min-disk-gb "$MIN_DISK_GB"
}
pull() {
  local got
  dk pull "$image" >"$out/pull.log" 2>&1 9>&- &
  bg_pid=$!
  wait_client || { tail -n 20 "$out/pull.log"; return 1; }
  got=$(dk image inspect --format '{{json .RepoDigests}}' "$image") || return
  echo "$got" >"$out/pull-digests.json"
  [[ $got == *"${image#*@}"* ]] || { log "pulled digests $got lack ${image#*@}"; return 1; }
}
gpu_probe() {
  container gpu_probe --network none --entrypoint "$PY" "$image" b300/gpu_probe.py \
    --out /out/gpu_probe.json --expect-gpus "$EXPECT_GPUS" --expect-cc "$EXPECT_CC"
}
mfu() {
  # sdpa-flash is qualified for sm_120 only (trainer/attention.py QUALIFIED_BACKENDS), so on
  # sm_103 select_backend returns math; HT_ATTN_BACKEND forces the flash path, as on the 5090.
  local ev=mfu-dp$EXPECT_GPUS-flash.json
  # Default network: torchrun --standalone resolves the container hostname for its store. Rank
  # output is teed into run.log; a PET_LOG_DIR under /out would be a root-only 0700 directory.
  container mfu -e HT_ATTN_BACKEND=sdpa-flash -e PET_TEE=3 --entrypoint "$PY" "$image" \
    -m torch.distributed.run --standalone \
    --nproc-per-node "$EXPECT_GPUS" run_mfu_dist.py --profile "mfu-dense-1b-dp$EXPECT_GPUS" \
    --device cuda --reduction zero2o --act-ckpt --warmup 1 ${PEAK:+--measured-peak "$PEAK"} \
    --out "/out/$ev" || return
  python3 "$HT/scripts/check_gpu_evidence.py" --kind mfu "$out/$ev" || return
  python3 - "$out/$ev" <<'PY'
import json, sys
e = json.load(open(sys.argv[1]))
print(e["attention_backend"], e["attention_paths"], e["tokens_per_s"], e.get("gpu_name"))
assert e["attention_backend"] == "sdpa-flash" and e["replay_match"] is True, e["attention_backend"]
assert e["attention_paths"] == ["flash"], e["attention_paths"]
PY
}
miner_test() {
  # The test challenge is the one of scripts/mine_smoke_v2.py: the real challenge app and a TLS
  # relay on 127.0.0.1 with the test ledger, driven by join-v2 -> probe-v2 -> proof-v2 -> run-v2
  # through the real CLI (CPU device). It loads tests/ from the mounted checkout and imports
  # pytest, which the image lacks, so pytest goes to a throwaway target inside the container.
  # Its evidence file is mode 0600: run as the invoking user so the host can read and upload it.
  # shellcheck disable=SC2016  # $1 expands in the container shell
  container miner_test --user "$(id -u):$(id -g)" -e HOME=/tmp \
    -e PYTHONPATH=/tmp/pyextra:/opt/ht/src -e PYTHONDONTWRITEBYTECODE=1 \
    -e PATH=/opt/ht/scripts/b300/shim:/opt/hypertrain/venv/bin:/usr/local/bin:/usr/bin:/bin \
    -e MINE_SMOKE_V2_EVIDENCE=/out/mine-smoke-v2.json --entrypoint bash "$image" -c \
    'uv-pinned pip install --quiet --target /tmp/pyextra pytest==9.1.1 &&
     exec timeout -k 5 "$1" python mine_smoke_v2.py' smoke "$SMOKE_BUDGET" \
    >"$out/mine-smoke-v2.stdout" || return
  python3 - "$out/mine-smoke-v2.json" <<'PY'
import json, sys
e = json.load(open(sys.argv[1]))
print(e["result"], [(s.get("name"), s.get("rc")) for s in e.get("stages", [])])
assert e["result"] == "PASS", e.get("failed")
PY
}
miner_prod() {
  # Admission (join-v2 --prepare, coldkey cosign, escrow) is the operator's: no owner key here.
  # The miner config directory (miner.toml, keyfile, workdir) is mounted at the same path; the
  # keyfile must belong to the invoking uid (miner/core.py load_keyfile), hence --user.
  local cfg=$HT_B300_MINER_CONFIG dir w summary
  dir=$(cd "$(dirname "$cfg")" && pwd)
  mkdir -p "$out/miner-prod"
  for w in $HT_B300_PROD_ROUNDS; do
    [ "$(date +%s)" -lt "$deadline" ] || { log "deadline reached before round $w"; return 1; }
    rm -f -- "$out/miner-prod/round-$w.json" "$out/miner-prod/round-$w-commitments.json"
    container "miner-prod-$w" --user "$(id -u):$(id -g)" -e HOME=/tmp \
      -e HYPERTRAIN_MINER_API="$HT_B300_PROD_ENDPOINT" -e HT_IMAGE_DIGEST="${image#*@}" \
      -v "$dir:$dir" "$image" run-v2 --config "$dir/$(basename "$cfg")" --round "$w" \
      >"$out/miner-prod/round-$w.json" || return
    grep -q '"status": *"UPLOADED"' "$out/miner-prod/round-$w.json" || return
    summary=$(find "$dir" -path "*/$w/published/rank-0/summary.json" -print -quit)
    if [ -n "$summary" ]; then
      python3 -c 'import json, sys; c = json.load(open(sys.argv[1]))["commitments"]
c.pop("leaves", None); print(json.dumps(c, indent=1, sort_keys=True))' "$summary" \
        >"$out/miner-prod/round-$w-commitments.json"
    fi
  done
}

step host_probe host_probe || finish host-probe-failed 3
step pull pull || finish pull-failed 4
step gpu_probe gpu_probe || finish gpu-probe-failed 5
step mfu mfu || finish mfu-failed 6
step miner_test miner_test || finish miner-test-failed 7
step miner_prod miner_prod || finish miner-prod-failed 8
finish completed 0
