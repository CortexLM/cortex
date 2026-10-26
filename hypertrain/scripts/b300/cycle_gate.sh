#!/usr/bin/env bash
# Gate a GPU step on the Lium validator cycle (docs/b300-run.md, "Lium validator cycle").
# Exit 0 only when both checks are clear; it prints one JSON line per poll on stdout.
#   1. No foreign compute app on the selected GPUs. "Ours" are the processes whose name starts
#      with --ours (default /opt/hypertrain/), so a second job of ours does not block the gate.
#   2. The executor log's latest SSH-key marker is not an `upload_ssh_key` younger than
#      --cycle-max seconds. The validator cycle starts at upload_ssh_key and ends at
#      remove_ssh_key (30 s to 8 min); an upload older than --cycle-max with no remove is
#      reported as stale and does not block.
# It only reads: nvidia-smi and `docker logs` of the executor container. It never touches it.
set -euo pipefail

gpus=
ours=/opt/hypertrain/
timeout=1800
poll=10
cycle_max=720
executor=${HT_GATE_EXECUTOR:-executor-executor-1}

usage() {
  echo "usage: cycle_gate.sh [--gpus 0,1,...] [--ours PREFIX] [--timeout SECONDS] [--poll SECONDS]" \
    "[--cycle-max SECONDS]" >&2
  exit 2
}
while [ "$#" -gt 0 ]; do
  case $1 in
    --gpus) gpus=${2:?}; shift 2 ;;
    --ours) ours=${2:?}; shift 2 ;;
    --timeout) timeout=${2:?}; shift 2 ;;
    --poll) poll=${2:?}; shift 2 ;;
    --cycle-max) cycle_max=${2:?}; shift 2 ;;
    *) usage ;;
  esac
done
for n in "$timeout" "$poll" "$cycle_max"; do [[ $n =~ ^[0-9]+$ ]] || usage; done
[ -z "$gpus" ] || [[ $gpus =~ ^[0-9]+(,[0-9]+)*$ ]] || usage

if docker info >/dev/null 2>&1; then dk() { docker "$@"; }; else dk() { sudo -n docker "$@"; }; fi

# Prints "<busy count> <last marker> <marker age seconds or -1>"; one `nvidia-smi` and one
# `docker logs` call per poll. --id selects GPUs; without it every GPU is checked.
probe() {
  local smi_args=() apps marker now stamp epoch
  [ -z "$gpus" ] || smi_args=(--id="$gpus")
  # Compute apps carry the GPU uuid, not its index, so resolve the selected uuids first.
  local uuids
  uuids=$(nvidia-smi "${smi_args[@]}" --query-gpu=uuid --format=csv,noheader)
  apps=$(nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name,used_memory --format=csv,noheader)
  busy=0
  while IFS=, read -r uuid _ name _; do
    [ -n "${uuid:-}" ] || continue
    uuid=${uuid// /}
    name=${name# }
    grep -qxF "$uuid" <<<"$uuids" || continue
    case $name in "$ours"*) ;; *) busy=$((busy + 1)) ;; esac
  done <<<"$apps"
  marker=$(dk logs -t --since 30m "$executor" 2>&1 | grep -E 'POST /(upload|remove)_ssh_key' | tail -n 1 || true)
  last=none age=-1
  if [ -n "$marker" ]; then
    case $marker in *upload_ssh_key*) last=upload ;; *) last=remove ;; esac
    stamp=${marker%% *}
    epoch=$(date -u -d "${stamp%.*}Z" +%s 2>/dev/null || echo 0)
    now=$(date -u +%s)
    age=$((now - epoch))
  fi
}

deadline=$(($(date +%s) + timeout))
while :; do
  probe
  in_cycle=0 stale=0
  if [ "$last" = upload ]; then
    if [ "$age" -lt "$cycle_max" ]; then in_cycle=1; else stale=1; fi
  fi
  clear=false
  [ "$busy" -eq 0 ] && [ "$in_cycle" -eq 0 ] && clear=true
  printf '{"time":"%s","gpus":"%s","foreign_compute_apps":%s,"last_marker":"%s","marker_age_s":%s,"in_cycle":%s,"stale_upload":%s,"clear":%s}\n' \
    "$(date -u +%FT%TZ)" "${gpus:-all}" "$busy" "$last" "$age" \
    "$([ "$in_cycle" -eq 1 ] && echo true || echo false)" \
    "$([ "$stale" -eq 1 ] && echo true || echo false)" "$clear"
  [ "$clear" = true ] && exit 0
  [ "$(date +%s)" -lt "$deadline" ] || exit 1
  sleep "$poll"
done
