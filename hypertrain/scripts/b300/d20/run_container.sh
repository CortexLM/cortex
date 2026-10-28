#!/usr/bin/env bash
# Run d20/gpu_run_v2.py in one --rm container of the GPU image on ONE GPU of a B300 host, with the
# checkout mounted read-only at /opt/ht (docs/b300-run.md, "Lium validator cycle").
#   1. cycle_gate.sh waits until no foreign compute app is on the host and no validator cycle is
#      open (unless --no-gate), then
#   2. gpu_watch.sh records GPU occupancy beside the run, and
#   3. the container runs `gpu_run_v2.py MODE`, and
#   4. run.json records the window, the exit code, the last gate line, whether the executor log
#      could be read, the last SSH-key marker before the window and every marker inside it.
# Everything is written under OUT_DIR (must not exist yet). The container is named
# ht-s9-<mode>-gpu<N>; only that name is ever removed, also on SIGINT/SIGTERM/SIGHUP (exit code 143;
# a shell ignores SIGINT in a job it starts with `&` and nohup does not change that, so start it in
# the foreground or send SIGTERM). SIGKILL cannot be trapped: the container then runs to its end and
# `--rm` removes it, no run.json is written, the watcher ends by itself (gpu_watch.sh PARENT_PID) and
# the next run of the same mode and GPU removes a leftover container of that name first. The Lium
# executor-* containers are never touched.
# usage: run_container.sh --image REPO@sha256:<64 hex> --gpu N --out DIR [--mode MODE]
#                         [--driver 595.91.07] [--sm-count 148] [--no-gate] [--gate-timeout SECONDS]
# MODE: smoke | multi (dense fixture model) or smoke-kimi | multi-kimi | job-kimi (kimi-k3-pro-tiny;
# HT_D20_KIMI_DTYPE=bf16|fp32 selects its compute dtype, default bf16).
# Every manifest the harness builds pins its environment: CPU class avx2 and the GPU class of
# --sm-count (148: sm_103, 170: sm_120; the harness refuses any other count at start).
# HT_D20_ATTN_BACKEND=math|sdpa-flash is the HT_ATTN_BACKEND of that pin, default math.
# Exit codes: the container's, 2 usage, 3 the gate did not clear, 143 interrupted.
set -euo pipefail

HT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
B300=$HT/scripts/b300
image= gpu= out= mode=multi driver=595.91.07 sm_count=148 gate=1 gate_timeout=1800
executor=${HT_GATE_EXECUTOR:-executor-executor-1}

usage() {
  sed -n '/^# usage:/,/^# Exit codes:/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
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
[[ $sm_count =~ ^[0-9]+$ ]] || usage
[[ $driver =~ ^[0-9][0-9.]*$ ]] || usage
[[ $out == /* ]] || { echo "--out must be an absolute path" >&2; exit 2; }
[ ! -e "$out" ] || { echo "$out already exists; use a fresh --out" >&2; exit 2; }
gpu=$((10#$gpu)) gate_timeout=$((10#$gate_timeout)) sm_count=$((10#$sm_count))

if docker info >/dev/null 2>&1; then dk() { docker "$@"; }; else dk() { sudo -n docker "$@"; }; fi
name=ht-s9-$mode-gpu$gpu
mkdir -p "$out"

if [ "$gate" -eq 1 ]; then
  gate_rc=0
  "$B300/cycle_gate.sh" --timeout "$gate_timeout" >"$out/gate.jsonl" || gate_rc=$?
  if [ "$gate_rc" -ne 0 ]; then
    echo "gate did not clear within ${gate_timeout}s (rc $gate_rc); see $out/gate.jsonl" >&2
    exit 3
  fi
fi

"$B300/gpu_watch.sh" "$out/gpu-watch" 2 "$$" &
watcher=$!
interrupted=0
cleanup() { kill "$watcher" 2>/dev/null || true; }
on_signal() { interrupted=1; dk rm -f "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap on_signal INT TERM HUP

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
  -e "HT_D20_ATTN_BACKEND=${HT_D20_ATTN_BACKEND:-math}" \
  -v "$HT:/opt/ht:ro" -v "$out:/out" -w /opt/ht/scripts --entrypoint bash "$image" -c \
  'uv-pinned pip install --quiet --target /tmp/pyextra pytest==9.1.1 &&
   exec python b300/d20/gpu_run_v2.py "$1"' run "$mode" \
  >"$out/stdout.log" 2>"$out/stderr.log" &
dockerpid=$!
wait "$dockerpid" || rc=$?
if [ "$interrupted" -eq 1 ]; then
  # The trap removed the container; collect the docker client's own exit status and report 143.
  wait "$dockerpid" 2>/dev/null || true
  rc=143
fi
ended=$(date -u +%FT%TZ)

# The executor log from 30 minutes before the window, so a cycle that opened before it is seen.
since=$(date -u -d "@$(($(date -u -d "$started" +%s) - 1800))" +%FT%TZ)
log_ok=true
logs=$(dk logs -t --since "$since" "$executor" 2>&1) || log_ok=false
{ grep -E 'POST /(upload|remove)_ssh_key HTTP/[0-9.]+" 2[0-9][0-9]' <<<"$logs" || true; } |
  sed -E 's/^([^ ]+) .*POST \/(upload|remove)_ssh_key.*/\1 \2/' >"$out/executor-markers.txt"
python3 - "$out" "$image" "$gpu" "$mode" "$started" "$ended" "$rc" "$gate" "$log_ok" "$interrupted" <<'PY'
import json
import re
import sys
from datetime import datetime, timedelta
from pathlib import Path

out, image, gpu, mode, started, ended, rc, gate, log_ok, interrupted = sys.argv[1:]


def at(stamp: str) -> datetime:
    stamp = re.sub(r"(\.\d{6})\d*Z$", r"\1Z", stamp)
    return datetime.fromisoformat(stamp.replace("Z", "+00:00"))


# `ended` is truncated to the second, so a marker in that second still belongs to the window.
begin, end = at(started), at(ended) + timedelta(seconds=1)
markers = [line.split() for line in (Path(out) / "executor-markers.txt").read_text().splitlines()]
before = [{"time": t, "kind": k} for t, k in markers if at(t) < begin]
inside = [{"time": t, "kind": k} for t, k in markers if begin <= at(t) <= end]
gate_lines = Path(out, "gate.jsonl")
gate_last = json.loads(gate_lines.read_text().splitlines()[-1]) if gate == "1" else None
json.dump(
    {
        "image": image,
        "gpu": int(gpu),
        "mode": mode,
        "started": started,
        "ended": ended,
        "rc": int(rc),
        "interrupted": interrupted == "1",
        "gated": gate == "1",
        "gate_last": gate_last,
        "executor_log_ok": log_ok == "true",
        "last_marker_before_window": before[-1] if before else None,
        "executor_markers_in_window": inside,
    },
    open(Path(out) / "run.json", "w"),
    indent=1,
    sort_keys=True,
)
PY
echo "run.json: rc=$rc window $started .. $ended"
exit "$rc"
