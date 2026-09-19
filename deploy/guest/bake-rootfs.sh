#!/usr/bin/env bash
# Build the Python guest stage and publish a measured, reproducible ext4 rootfs.
set -euo pipefail

usage() {
    cat <<'EOF'
usage: bake-rootfs.sh [options]

  --engine docker|podman  OCI engine (default: docker)
  --image sha256:<hex>    convert an existing content-addressed guest image
  --python-image REF      pinned Python base passed to the Docker build
  --size-mib N            ext4 capacity, 1024..32768 (default: 4096)
  --source-date-epoch N   normalized filesystem timestamp (default: 946684800)
  --out-dir DIR           publication directory (default: ./out)
  -h, --help              show this help

Without --image, the script builds deploy/Dockerfile's guest target and reads
its content-addressed image ID. It never writes a deployment pin automatically.
EOF
}

die() {
    echo "bake-rootfs: $*" >&2
    exit 2
}

ENGINE=docker
IMAGE=""
PYTHON_IMAGE=""
SIZE_MIB=4096
SOURCE_DATE_EPOCH=946684800
OUT_DIR=./out

while [ "$#" -gt 0 ]; do
    case "$1" in
        --engine) ENGINE="$2"; shift 2 ;;
        --image) IMAGE="$2"; shift 2 ;;
        --python-image) PYTHON_IMAGE="$2"; shift 2 ;;
        --size-mib) SIZE_MIB="$2"; shift 2 ;;
        --source-date-epoch) SOURCE_DATE_EPOCH="$2"; shift 2 ;;
        --out-dir) OUT_DIR="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done

[ "$ENGINE" = docker ] || [ "$ENGINE" = podman ] || die "unsupported OCI engine"
[[ "$SIZE_MIB" =~ ^[0-9]+$ ]] || die "--size-mib must be an integer"
[ "$SIZE_MIB" -ge 1024 ] && [ "$SIZE_MIB" -le 32768 ] || die "--size-mib is outside 1024..32768"
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || die "--source-date-epoch must be an integer"
[ "$SOURCE_DATE_EPOCH" -ge 1 ] || die "--source-date-epoch must be positive"
[ "$SOURCE_DATE_EPOCH" -le 2147483647 ] || die "--source-date-epoch exceeds the portable ext4 range"
[ "$(id -u)" -eq 0 ] || die "run as root or inside a rootless podman unshare namespace"

ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
for tool in "$ENGINE" python3 fakeroot mkfs.ext4 debugfs tune2fs e2fsck sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done
mkdir -p -- "$OUT_DIR"
OUT_DIR="$(CDPATH= cd -- "$OUT_DIR" && pwd)"
SCRATCH="$(mktemp -d "$OUT_DIR/.guest-rootfs.XXXXXX")"
cleanup() { rm -rf -- "$SCRATCH"; }
trap cleanup EXIT

if [ -z "$IMAGE" ]; then
    build=(
        "$ENGINE" build
        --file "$ROOT/deploy/Dockerfile"
        --target guest
        --iidfile "$SCRATCH/image.iid"
    )
    if [ -n "$PYTHON_IMAGE" ]; then
        build+=(--build-arg "PYTHON_IMAGE=$PYTHON_IMAGE")
    fi
    build+=("$ROOT")
    "${build[@]}"
    IMAGE="$(tr -d '\r\n' < "$SCRATCH/image.iid")"
fi
[[ "$IMAGE" =~ ^([a-z0-9][a-z0-9._:/-]*@)?sha256:[0-9a-f]{64}$ ]] \
    || die "guest image must be content-addressed by sha256"

fakeroot -- python3 "$ROOT/src/cortex/vm/image.py" \
    --engine "$ENGINE" \
    --image "$IMAGE" \
    --output "$SCRATCH/rootfs.ext4" \
    --size-mib "$SIZE_MIB" \
    --source-date-epoch "$SOURCE_DATE_EPOCH" \
    > "$SCRATCH/build.json"

digest_line="$(sha256sum "$SCRATCH/rootfs.ext4")"
DIGEST="${digest_line%% *}"
[[ "$DIGEST" =~ ^[0-9a-f]{64}$ ]] || die "could not measure rootfs"
FINAL="$OUT_DIR/sha256-$DIGEST.ext4"
if [ -e "$FINAL" ] || [ -L "$FINAL" ]; then
    existing_line="$(sha256sum "$FINAL" 2>/dev/null)" || die "existing output is unreadable"
    EXISTING="${existing_line%% *}"
    [ "$EXISTING" = "$DIGEST" ] || die "existing digest-named output has different bytes"
    rm -f -- "$SCRATCH/rootfs.ext4"
else
    chmod 0644 "$SCRATCH/rootfs.ext4"
    mv -- "$SCRATCH/rootfs.ext4" "$FINAL"
fi

printf 'rootfs=%s\ndigest=sha256:%s\nsource_image=%s\nsource_date_epoch=%s\n' \
    "$FINAL" "$DIGEST" "$IMAGE" "$SOURCE_DATE_EPOCH"
