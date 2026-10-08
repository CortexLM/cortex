#!/usr/bin/env bash
# Mirror of the Cortex supervisor label check (cortex@d738424 supervisor.py verify_labels) plus
# the contract-v1 user/port rules. Exit 0 = conformant; 1 = refused (reasons on stderr).
#   scripts/verify_image.sh IMAGE [SLUG] [SOURCE]      ENGINE=docker|podman (default podman)
set -euo pipefail
image=${1:?usage: verify_image.sh IMAGE [SLUG] [SOURCE]}
slug=${2:-hypertrain}
source=${3:-https://github.com/CortexLM/hypertrain}
engine=${ENGINE:-podman}
config=$("$engine" image inspect "$image" --format '{{json .Config}}')
fail=0
check() { # name actual expected
  if [ "$2" != "$3" ]; then echo "REFUSED: $1 is '$2', expected '$3'" >&2; fail=1; fi
}
label() { jq -r --arg k "$1" '(.Labels // {})[$k] // ""' <<<"$config"; }
check io.cortex.challenge.slug "$(label io.cortex.challenge.slug)" "$slug"
check io.cortex.challenge.contract "$(label io.cortex.challenge.contract)" "1"
src=$(label org.opencontainers.image.source)
check org.opencontainers.image.source "${src%/}" "$source"
check User "$(jq -r '.User // ""' <<<"$config")" "65532:65532"
check ExposedPorts "$(jq -r '(.ExposedPorts // {}) | has("8000/tcp")' <<<"$config")" "true"
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "OK: $image labels slug=$slug contract=1 source=$source user=65532:65532 port=8000"
