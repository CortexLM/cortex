#!/usr/bin/env bash
# Record GPU occupancy beside a timed run so a validator cycle (docs/b300-run.md) is visible in
# the evidence. Every INTERVAL seconds (default 2) it appends to <out-prefix>.apps.csv one row
# per compute app (time, gpu_uuid, pid, process_name, used_memory) and to <out-prefix>.mem.csv
# one row per GPU (time, index, memory.used). Both files are truncated at start, so use a fresh
# prefix. A failed or hung (10 s) nvidia-smi call writes a `<time>,nvidia-smi-failed` row, so a gap in
# the record is never silent. It runs until it is killed and only reads.
# usage: gpu_watch.sh OUT_PREFIX [INTERVAL]
set -uo pipefail

[ "$#" -ge 1 ] && [ "$#" -le 2 ] || { echo "usage: gpu_watch.sh OUT_PREFIX [INTERVAL]" >&2; exit 2; }
prefix=$1
interval=${2:-2}
[[ $interval =~ ^[0-9]+$ ]] && [ "$((10#$interval))" -gt 0 ] || { echo "INTERVAL must be a positive integer" >&2; exit 2; }
interval=$((10#$interval))

echo "time,gpu_uuid,pid,process_name,used_memory" >"$prefix.apps.csv"
echo "time,index,memory_used" >"$prefix.mem.csv"

# sample FILE QUERY_FLAG FIELDS: append one timestamped row per line nvidia-smi prints.
sample() {
  local file=$1 now rows
  now=$(date -u +%FT%TZ)
  if rows=$(timeout 10 nvidia-smi "--query-$2=$3" --format=csv,noheader); then
    [ -z "$rows" ] || sed "s/^/$now,/" <<<"$rows" >>"$file"
  else
    echo "$now,nvidia-smi-failed" >>"$file"
  fi
}

while :; do
  sample "$prefix.apps.csv" compute-apps gpu_uuid,pid,process_name,used_memory
  sample "$prefix.mem.csv" gpu index,memory.used
  sleep "$interval"
done
