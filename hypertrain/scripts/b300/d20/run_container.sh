#!/usr/bin/env bash
# Run d20/gpu_run_v2.py in one --rm container of the GPU image on ONE GPU of a B300 host, with the
# checkout mounted read-only at /opt/ht (docs/b300-run.md, "Lium validator cycle").
#   1. cycle_gate.sh waits until no foreign compute app is on the host and no validator cycle is
#      open (unless --no-gate), then
#   2. gpu_watch.sh records GPU occupancy beside the run, and
#   3. the container runs `gpu_run_v2.py MODE`, and
#   4. run.json records the window, the exit code and every executor SSH-key marker inside it, so
#      an overlap with a validator cycle is a fact in the evidence and not an assumption.
# Everything is written under OUT_DIR (must not exist yet). The container is named
# ht-s9-<mode>; only that name is ever removed. The Lium executor-* containers are never touched.
# usage: run_container.sh --image REPO@sha256:<64 hex> --gpu N --out DIR [--mode MODE]
#                         [--driver 595.91.07] [--sm-count 148] [--no-gate] [--gate-timeout SECONDS]
# MODE: smoke | multi (dense fixture model) or smoke-kimi | multi-kimi | job-kimi (kimi-k3-pro-tiny;
# HT_D20_KIMI_DTYPE=bf16|fp32 selects its compute dtype, default bf16).
set -euo pipefail

HT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
B300=$HT/scripts/b300
image= gpu= out= mode=multi driver=595.91.07 sm_count=148 gate=1 gate_timeout=1800
executor=${HT_GATE_EXECUTOR:-executor-executor-1}

usage() {
  sed -n '/^# usage:/,/^set -/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//; /^set -/d' >&2
  exit 2
}
while [ "$#" -gt 0 ]; do
  case $1 in
    --image) image=${2:?}; shift 2 ;;
    --gpu) gpu=${2:?}; shift 2 ;;
    --out) out=${2:?}; shift 2 ;;
    --mode) mode=${2:?}; shift 2 ;;
    --driver) driver=${2:?}; shift 2 ;;
    --sm-count) sm_count=${2:?}; shift 2 ;;
    --no-gate) gate=0; shift ;;
    --gate-timeout) gate_timeout=${2:?}; shift 2 ;;
    *) usage ;;
  esac
done
[[ $image =~ ^[a-z0-9./_-]+@sha256:[0-9a-f]{64}$ ]] || usage
[[ $gpu =~ ^[0-9]+$ ]] || usage
[[ $mode =~ ^(smoke|multi|smoke-kimi|multi-kimi|job-kimi)$ ]] || usage
[[ $gate_timeout =~ ^[0-9]+$ ]] || usage
[[ $out == /* ]] || { echo "--out must be an absolute path" >&2; exit 2; }
[ ! -e "$out" ] || { echo "$out already exists; use a fresh --out" >&2; exit 2; }

if docker info >/dev/null 2>&1; then dk() { docker "$@"; }; else dk() { sudo -n docker "$@"; }; fi
name=ht-s9-$mode
mkdir -p "$out"

if [ "$gate" -eq 1 ]; then
  "$B300/cycle_gate.sh" --timeout "$gate_timeout" >"$out/gate.jsonl" || {
    echo "gate did not clear within ${gate_timeout}s; see $out/gate.jsonl" >&2
    exit 3
  }
fi

"$B300/gpu_watch.sh" "$out/gpu-watch" 2 &
watcher=$!
trap 'kill "$watcher" 2>/dev/null || true' EXIT

started=$(date -u +%FT%TZ)
rc=0
dk rm -f "$name" >/dev/null 2>&1 || true
# shellcheck disable=SC2016  # $1 expands in the container shell
dk run --rm --init --name "$name" --gpus "device=$gpu" --ipc=host \
  --ulimit memlock=-1 --ulimit stack=67108864 --user "$(id -u):$(id -g)" -e HOME=/tmp \
  -e PYTHONPATH=/tmp/pyextra:/opt/ht/src -e PYTHONDONTWRITEBYTECODE=1 \
  -e PATH=/opt/ht/scripts/b300/shim:/opt/hypertrain/venv/bin:/usr/local/bin:/usr/bin:/bin \
  -e HYPERTRAIN_MINER_DEVICE=cuda -e "HT_IMAGE_DIGEST=${image#*@}" \
  -e "HT_D20_DRIVER=$driver" -e "HT_D20_SM_COUNT=$sm_count" -e HT_D20_EVENTS=/out/events.jsonl \
  -e MINE_SMOKE_V2_EVIDENCE=/out/mine-smoke-v2.json -e MINE_MULTI_ROUND_V2_EVIDENCE=/out/multi.json \
  -e HT_D20_EVIDENCE=/out/job-kimi.json -e "HT_D20_KIMI_DTYPE=${HT_D20_KIMI_DTYPE:-bf16}" \
  -v "$HT:/opt/ht:ro" -v "$out:/out" -w /opt/ht/scripts --entrypoint bash "$image" -c \
  'uv-pinned pip install --quiet --target /tmp/pyextra pytest==9.1.1 &&
   exec python b300/d20/gpu_run_v2.py "$1"' run "$mode" \
  >"$out/stdout.log" 2>"$out/stderr.log" || rc=$?
ended=$(date -u +%FT%TZ)

markers=$(dk logs -t --since "$started" "$executor" 2>&1 | grep -E 'POST /(upload|remove)_ssh_key' |
  sed -E 's/^([^ ]+) .*POST \/(upload|remove)_ssh_key.*/\1 \2/' || true)
python3 - "$out/run.json" "$image" "$gpu" "$mode" "$started" "$ended" "$rc" "$gate" <<PY
import json, sys
path, image, gpu, mode, started, ended, rc, gate = sys.argv[1:]
markers = [line.split() for line in """$markers""".splitlines() if line.strip()]
json.dump(
    {"image": image, "gpu": int(gpu), "mode": mode, "started": started, "ended": ended,
     "rc": int(rc), "gated": gate == "1",
     "executor_markers_in_window": [{"time": t, "kind": k} for t, k in markers]},
    open(path, "w"), indent=1, sort_keys=True)
PY
echo "run.json: rc=$rc window $started .. $ended (markers in window: $(wc -w <<<"$markers" | awk '{print $1/2}'))"
exit "$rc"
