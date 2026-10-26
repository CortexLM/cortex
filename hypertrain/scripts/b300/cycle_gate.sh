#!/usr/bin/env bash
# Gate a GPU step on the Lium validator cycle (docs/b300-run.md, "Lium validator cycle").
# Exit 0 only when every check is clear; it prints one JSON line per poll on stdout.
#   1. No foreign compute app on the selected GPUs. A row is ours only when its process NAME starts
#      with --ours (default /opt/hypertrain/): a name match, not an owner check. A row named `[No data]`
#      or a bare `python` (both seen for our own short-lived processes) counts as foreign, so another
#      job of ours can block the gate for a moment.
#   2. The executor log shows no open validator cycle. Markers are the 2xx lines `POST /upload_ssh_key`
#      and `POST /remove_ssh_key` of `docker logs -t --since 30m`. Over the markers younger than
#      --cycle-max seconds an upload opens a cycle and a remove closes one, so nested uploads need as
#      many removes. An unmatched upload older than --cycle-max is reported as `stale_upload` and does not
#      block (cycles lasted 4 to 12.5 minutes on the Hyderabad host). With no marker in the last 30
#      minutes the gate does not clear, because a recreated executor container has an empty log; pass
#      --allow-no-marker on a host without a validator.
# Fail-closed: an unreadable executor log or nvidia-smi is not clear, and so is a marker whose time
# cannot be parsed (it needs GNU date). Exit codes: 0 clear, 1 not clear before --timeout, 2 usage,
# 4 nvidia-smi or the executor log was unreadable at the last poll before --timeout.
# It only reads: nvidia-smi and `docker logs` of the executor container. It never touches it.
set -euo pipefail

gpus=
ours=/opt/hypertrain/
timeout=1800
poll=10
cycle_max=900
allow_none=0
executor=${HT_GATE_EXECUTOR:-executor-executor-1}

usage() {
  echo "usage: cycle_gate.sh [--gpus 0,1,...] [--ours PREFIX] [--timeout SECONDS] [--poll SECONDS]" \
    "[--cycle-max SECONDS] [--allow-no-marker]" >&2
  exit 2
}
while [ "$#" -gt 0 ]; do
  case $1 in
    --gpus) gpus=${2:?}; shift 2 ;;
    --ours) ours=${2:?}; shift 2 ;;
    --timeout) timeout=${2:?}; shift 2 ;;
    --poll) poll=${2:?}; shift 2 ;;
    --cycle-max) cycle_max=${2:?}; shift 2 ;;
    --allow-no-marker) allow_none=1; shift ;;
    *) usage ;;
  esac
done
for n in "$timeout" "$poll" "$cycle_max"; do [[ $n =~ ^[0-9]+$ ]] || usage; done
[ -z "$gpus" ] || [[ $gpus =~ ^[0-9]+(,[0-9]+)*$ ]] || usage
timeout=$((10#$timeout)) poll=$((10#$poll)) cycle_max=$((10#$cycle_max))

if docker info >/dev/null 2>&1; then dk() { docker "$@"; }; else dk() { sudo -n docker "$@"; }; fi

marker_re='^([^[:space:]]+)[[:space:]].*"POST /(upload|remove)_ssh_key HTTP/[0-9.]+" 2[0-9][0-9]'

# Sets busy, depth, last, age, stale, parse_error, smi_ok and log_ok from one nvidia-smi pass and one
# `docker logs` call. depth = uploads not yet matched by a remove among the markers younger than
# cycle_max.
probe() {
  local smi_args=() uuids apps logs line ts kind epoch now uuid name
  busy=0 depth=0 last=none age=-1 stale=false parse_error=false smi_ok=true log_ok=true
  [ -z "$gpus" ] || smi_args=(--id="$gpus")
  # Compute apps carry the GPU uuid, not its index, so resolve the selected uuids first.
  if ! uuids=$(nvidia-smi "${smi_args[@]}" --query-gpu=uuid --format=csv,noheader 2>/dev/null) || [ -z "$uuids" ] ||
    ! apps=$(nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name,used_memory --format=csv,noheader 2>/dev/null); then
    smi_ok=false
  else
    while IFS=, read -r uuid _ name _; do
      [ -n "${uuid:-}" ] || continue
      uuid=${uuid// /}
      name=${name# }
      grep -qxF "$uuid" <<<"$uuids" || continue
      case $name in "$ours"*) ;; *) busy=$((busy + 1)) ;; esac
    done <<<"$apps"
  fi
  if ! logs=$(dk logs -t --since 30m "$executor" 2>&1); then
    log_ok=false
    return
  fi
  now=$(date -u +%s)
  while IFS= read -r line; do
    [[ $line =~ $marker_re ]] || continue
    ts=${BASH_REMATCH[1]} kind=${BASH_REMATCH[2]}
    last=$kind
    if ! epoch=$(date -u -d "${ts%%.*}" +%s 2>/dev/null); then
      parse_error=true
      depth=$((depth + 1))
      continue
    fi
    age=$((now - epoch))
    [ "$age" -lt "$cycle_max" ] || continue
    if [ "$kind" = upload ]; then depth=$((depth + 1)); elif [ "$depth" -gt 0 ]; then depth=$((depth - 1)); fi
  done < <(grep -E 'POST /(upload|remove)_ssh_key' <<<"$logs" || true)
  if [ "$last" = upload ] && [ "$age" -ge "$cycle_max" ]; then stale=true; fi
}

bool() { if [ "$1" = true ] || [ "$1" = 1 ]; then echo true; else echo false; fi; }

deadline=$(($(date +%s) + timeout))
while :; do
  probe
  in_cycle=false
  { [ "$depth" -gt 0 ] || [ "$parse_error" = true ]; } && in_cycle=true
  clear=false
  if [ "$smi_ok" = true ] && [ "$log_ok" = true ] && [ "$busy" -eq 0 ] && [ "$in_cycle" = false ] &&
    { [ "$last" != none ] || [ "$allow_none" -eq 1 ]; }; then
    clear=true
  fi
  printf '{"time":"%s","gpus":"%s","foreign_compute_apps":%s,"last_marker":"%s","marker_age_s":%s,"open_cycles":%s,"in_cycle":%s,"stale_upload":%s,"parse_error":%s,"nvidia_smi_ok":%s,"executor_log_ok":%s,"clear":%s}\n' \
    "$(date -u +%FT%TZ)" "${gpus:-all}" "$busy" "$last" "$age" "$depth" "$in_cycle" \
    "$(bool "$stale")" "$(bool "$parse_error")" "$(bool "$smi_ok")" "$(bool "$log_ok")" "$clear"
  [ "$clear" = true ] && exit 0
  if [ "$(date +%s)" -ge "$deadline" ]; then
    { [ "$smi_ok" = true ] && [ "$log_ok" = true ]; } || exit 4
    exit 1
  fi
  sleep "$poll"
done
