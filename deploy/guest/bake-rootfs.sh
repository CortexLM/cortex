#!/usr/bin/env bash
# bake-rootfs.sh — build the Proof RLM / experiment-VM guest rootfs (ext4)
# for the Firecracker topic-VM host, and print the RE-LOCK steps.
#
# What goes in (all operator capability, nothing topic-specific):
#   * a Debian minbase tree (mmdebstrap) with the tools the guest init needs;
#   * proof-vm-guest-agent (built from this repo, static musl preferred) as
#     the vsock agent, deploy/guest/init.sh as /sbin/init;
#   * --with-podman (default on): rootless podman + crun + fuse-overlayfs +
#     slirp4netns/passt + podman-compose + catatonit, an unprivileged run-as
#     user with subuid/subgid ranges, storage on the writable scratch drive
#     (cgroupfs manager: there is no systemd in the guest);
#   * --with-harbor --harbor-version X.Y.Z: the Harbor CLI in a venv under
#     /opt/harbor (pinned version, never floating) — one example of a
#     container-based benchmark harness an adaptor may drive;
#   * --runner <id>=<dir>: operator adaptors copied to /opt/proof/runners/<id>
#     (the id is what a signed topic names in constraints.params;
#     see deploy/guest/runners/README.md for the contract).
#
# What never goes in: a benchmark pack, a task list, a dataset, a model, a
# key. Packs are staged by the KVM host at boot from its pack directory;
# keys come over vsock from PROOF_VM_AGENT_OWNER_KEY_DIR.
#
# Size budget: the baked tree must fit --budget-mib (default 2560 MiB ~ the
# 1.5–2.5 GiB target); the image file is --size-mib (default 3072 MiB, so
# the read-only rootfs has headroom). Per-VM writable disk (>= 32 GiB by
# default) is a separate scratch drive the host creates per boot; image
# pulls and job output never touch this image.
#
# Requires root (chroot + device nodes + mkfs -d ownership): run it in a
# throwaway build VM or container. Tools: mmdebstrap, e2fsprogs (mkfs.ext4
# with -d), tar, sha256sum, du. Network to the Debian mirror (and PyPI with
# --with-harbor).
#
# Never write a digest you did not compute from the file you staged: this
# script prints `sha256sum` of the image it wrote and names the file after
# it; that hex is what goes into PROOF_RLM_VM_IMAGE_DIGEST (or
# PROOF_EXPERIMENT_VM_IMAGE_DIGEST) on the control plane.
set -euo pipefail

usage() {
    cat <<'EOF'
usage: bake-rootfs.sh --guest-agent PATH [options]

  --guest-agent PATH       proof-vm-guest-agent binary to install (required)
  --runner ID=DIR          adaptor directory for runner id ID (repeatable);
                           DIR/run must be executable
  --with-podman | --no-podman   rootless podman + crun + fuse-overlayfs (default: with)
  --with-harbor            install the Harbor CLI (needs --harbor-version)
  --harbor-version X.Y.Z   exact Harbor version to pin (no floating installs)
  --suite NAME             Debian suite (default: trixie)
  --mirror URL             Debian mirror (default: http://deb.debian.org/debian)
  --run-as-uid N           unprivileged uid adaptors run as (default: 1000)
  --resolver IP            nameserver baked into the guest (default: none;
                           must also be on the host egress allowlist :53/udp)
  --allow-plain-http       let the guest fetch http:// artefacts (staging only)
  --size-mib N             ext4 image size (default: 3072)
  --budget-mib N           fail when the baked tree exceeds this (default: 2560)
  --out-dir DIR            where the image lands (default: ./out)
  --check-kernel-config F  grep a kernel .config for what rootless podman needs
  --dry-run                print the plan and exit 0 without building
  -h, --help               this text
EOF
}

GUEST_AGENT=""
RUNNERS=()
WITH_PODMAN=1
WITH_HARBOR=0
HARBOR_VERSION=""
SUITE=trixie
MIRROR=http://deb.debian.org/debian
RUN_AS_UID=1000
RESOLVER=""
ALLOW_PLAIN_HTTP=0
SIZE_MIB=3072
BUDGET_MIB=2560
OUT_DIR=./out
KERNEL_CONFIG=""
DRY_RUN=0

while [ $# -gt 0 ]; do
    case "$1" in
        --guest-agent) GUEST_AGENT="$2"; shift 2 ;;
        --runner) RUNNERS+=("$2"); shift 2 ;;
        --with-podman) WITH_PODMAN=1; shift ;;
        --no-podman) WITH_PODMAN=0; shift ;;
        --with-harbor) WITH_HARBOR=1; shift ;;
        --harbor-version) HARBOR_VERSION="$2"; shift 2 ;;
        --suite) SUITE="$2"; shift 2 ;;
        --mirror) MIRROR="$2"; shift 2 ;;
        --run-as-uid) RUN_AS_UID="$2"; shift 2 ;;
        --resolver) RESOLVER="$2"; shift 2 ;;
        --allow-plain-http) ALLOW_PLAIN_HTTP=1; shift ;;
        --size-mib) SIZE_MIB="$2"; shift 2 ;;
        --budget-mib) BUDGET_MIB="$2"; shift 2 ;;
        --out-dir) OUT_DIR="$2"; shift 2 ;;
        --check-kernel-config) KERNEL_CONFIG="$2"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

die() { echo "bake-rootfs: $*" >&2; exit 2; }

[ -n "$GUEST_AGENT" ] || die "--guest-agent is required"
[ -f "$GUEST_AGENT" ] && [ -x "$GUEST_AGENT" ] || die "guest agent $GUEST_AGENT is not an executable file"
if [ "$WITH_HARBOR" = 1 ] && [ -z "$HARBOR_VERSION" ]; then
    die "--with-harbor needs --harbor-version X.Y.Z (pin it; never a floating install)"
fi
[[ "$RUN_AS_UID" =~ ^[0-9]+$ ]] && [ "$RUN_AS_UID" -ge 1000 ] || die "--run-as-uid must be an unprivileged uid (>= 1000)"
[[ "$SIZE_MIB" =~ ^[0-9]+$ ]] && [[ "$BUDGET_MIB" =~ ^[0-9]+$ ]] || die "--size-mib / --budget-mib must be integers"
[ "$SIZE_MIB" -gt "$BUDGET_MIB" ] || die "--size-mib ($SIZE_MIB) must exceed --budget-mib ($BUDGET_MIB) so the tree fits"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[ -f "$HERE/init.sh" ] && [ -f "$HERE/agent-loop.sh" ] || die "init.sh / agent-loop.sh missing beside this script"

RUNNER_IDS=()
RUNNER_DIRS=()
for spec in "${RUNNERS[@]+"${RUNNERS[@]}"}"; do
    id="${spec%%=*}"
    dir="${spec#*=}"
    [ "$id" != "$spec" ] && [ -n "$id" ] && [ -n "$dir" ] || die "--runner wants ID=DIR, got $spec"
    [[ "$id" =~ ^[a-z0-9][a-z0-9_-]{1,63}$ ]] || die "runner id $id must match [a-z0-9][a-z0-9_-]{1,63} (it is what a topic names in constraints.params)"
    [ -x "$dir/run" ] || die "runner $id: $dir/run is not executable (the agent refuses an adaptor without run)"
    RUNNER_IDS+=("$id")
    RUNNER_DIRS+=("$dir")
done

# Kernel features rootless podman in a Firecracker guest depends on. A stock
# microVM kernel config often lacks several; check before you boot.
KERNEL_NEEDS=(CONFIG_USER_NS CONFIG_OVERLAY_FS CONFIG_FUSE_FS CONFIG_VETH CONFIG_TUN CONFIG_CGROUPS CONFIG_MEMCG CONFIG_CGROUP_PIDS CONFIG_SECCOMP CONFIG_SECCOMP_FILTER CONFIG_NETFILTER CONFIG_NF_TABLES CONFIG_VIRTIO_VSOCKETS CONFIG_VIRTIO_BLK CONFIG_VIRTIO_NET CONFIG_EXT4_FS CONFIG_TMPFS CONFIG_DEVTMPFS CONFIG_UNIX CONFIG_INET)
if [ -n "$KERNEL_CONFIG" ]; then
    [ -r "$KERNEL_CONFIG" ] || die "--check-kernel-config $KERNEL_CONFIG is not readable"
    missing=()
    for sym in "${KERNEL_NEEDS[@]}"; do
        grep -Eq "^${sym}=(y|m)$" "$KERNEL_CONFIG" || missing+=("$sym")
    done
    if [ "${#missing[@]}" -gt 0 ]; then
        die "kernel config lacks: ${missing[*]} (rootless podman / vsock / scratch need them)"
    fi
    echo "kernel config: every required symbol present"
fi

BASE_PKGS=(ca-certificates iproute2 iputils-ping procps util-linux e2fsprogs tar gzip curl jq python3 openssl kmod less coreutils findutils grep sed gawk bash)
PODMAN_PKGS=(podman uidmap slirp4netns passt fuse-overlayfs crun netavark aardvark-dns podman-compose catatonit fuse3)
HARBOR_PKGS=(python3-venv python3-pip git)
PKGS=("${BASE_PKGS[@]}")
[ "$WITH_PODMAN" = 1 ] && PKGS+=("${PODMAN_PKGS[@]}")
[ "$WITH_HARBOR" = 1 ] && PKGS+=("${HARBOR_PKGS[@]}")
INCLUDE="$(IFS=,; echo "${PKGS[*]}")"

cat <<EOF
bake plan
  suite/mirror     $SUITE  $MIRROR
  packages         $INCLUDE
  guest agent      $GUEST_AGENT
  init             $HERE/init.sh -> /sbin/init ; $HERE/agent-loop.sh -> /usr/local/sbin/proof-agent-loop
  runners          ${RUNNER_IDS[*]:-(none: every in-guest job fails closed until an adaptor is baked)}
  podman           $([ "$WITH_PODMAN" = 1 ] && echo "rootless (crun, fuse-overlayfs, cgroupfs), run-as uid $RUN_AS_UID" || echo no)
  harbor           $([ "$WITH_HARBOR" = 1 ] && echo "venv /opt/harbor, harbor==$HARBOR_VERSION" || echo no)
  resolver         ${RESOLVER:-(none baked; adaptors must not need DNS or the topic must not need egress)}
  plain http       $([ "$ALLOW_PLAIN_HTTP" = 1 ] && echo "allowed (staging only)" || echo "refused (https only)")
  image            $OUT_DIR/sha256-<hex>.ext4, $SIZE_MIB MiB, tree budget $BUDGET_MIB MiB
EOF
if [ "$DRY_RUN" = 1 ]; then
    echo "dry run: nothing built"
    exit 0
fi

[ "$(id -u)" = 0 ] || die "run as root (chroot, device nodes, mkfs -d ownership)"
for tool in mmdebstrap mkfs.ext4 sha256sum du tar; do
    command -v "$tool" > /dev/null 2>&1 || die "$tool is required"
done

ROOT="$(mktemp -d /tmp/proof-rootfs.XXXXXX)"
cleanup() { rm -rf "$ROOT"; }
trap cleanup EXIT

echo "== debootstrap ($SUITE minbase) =="
mmdebstrap --variant=minbase --include="$INCLUDE" \
    --aptopt='Acquire::Languages "none"' \
    --dpkgopt='path-exclude=/usr/share/doc/*' \
    --dpkgopt='path-exclude=/usr/share/man/*' \
    --dpkgopt='path-exclude=/usr/share/locale/*' \
    --dpkgopt='path-exclude=/usr/share/info/*' \
    "$SUITE" "$ROOT" "$MIRROR"

echo "== guest agent + init =="
install -m 0755 "$GUEST_AGENT" "$ROOT/usr/local/bin/proof-vm-guest-agent"
install -m 0755 "$HERE/init.sh" "$ROOT/sbin/init"
install -d -m 0755 "$ROOT/usr/local/sbin"
install -m 0755 "$HERE/agent-loop.sh" "$ROOT/usr/local/sbin/proof-agent-loop"
install -d -m 0755 "$ROOT/etc/proof" "$ROOT/opt/proof/runners" "$ROOT/var/lib/proof" "$ROOT/run/proof"
{
    echo "PROOF_GUEST_RUN_AS_UID=$RUN_AS_UID"
    echo "PROOF_GUEST_RUN_AS_GID=$RUN_AS_UID"
    echo "PROOF_GUEST_RESOLVER=$RESOLVER"
    [ "$ALLOW_PLAIN_HTTP" = 1 ] && echo "PROOF_GUEST_ALLOW_PLAIN_HTTP=1"
} > "$ROOT/etc/proof/guest.env"
chmod 0644 "$ROOT/etc/proof/guest.env"
echo proof-guest > "$ROOT/etc/hostname"
printf '127.0.0.1 localhost\n127.0.1.1 proof-guest\n' > "$ROOT/etc/hosts"
if [ -n "$RESOLVER" ]; then echo "nameserver $RESOLVER" > "$ROOT/etc/resolv.conf"; else : > "$ROOT/etc/resolv.conf"; fi

echo "== run-as user uid $RUN_AS_UID =="
chroot "$ROOT" /usr/sbin/groupadd -g "$RUN_AS_UID" runner
chroot "$ROOT" /usr/sbin/useradd -u "$RUN_AS_UID" -g "$RUN_AS_UID" -d "/home/uid$RUN_AS_UID" -M -s /bin/bash runner
install -d -m 0755 -o "$RUN_AS_UID" -g "$RUN_AS_UID" "$ROOT/home/uid$RUN_AS_UID"
echo "runner:100000:65536" > "$ROOT/etc/subuid"
echo "runner:100000:65536" > "$ROOT/etc/subgid"

if [ "$WITH_PODMAN" = 1 ]; then
    echo "== rootless podman config =="
    install -d -m 0755 "$ROOT/etc/containers"
    cat > "$ROOT/etc/containers/containers.conf" <<'EOF'
# Proof guest: no systemd, cgroup v2 mounted by /sbin/init, rootless by default.
[engine]
cgroup_manager = "cgroupfs"
events_logger = "file"
runtime = "crun"
[network]
network_backend = "netavark"
default_rootless_network_cmd = "pasta"
EOF
    cat > "$ROOT/etc/containers/storage.conf" <<'EOF'
# Rootless store lives on the per-VM writable disk (init bind-mounts the
# run-as home onto /var/lib/proof/home and links .local/share/containers).
[storage]
driver = "overlay"
runroot = "/run/containers/storage"
graphroot = "/var/lib/containers/storage"
[storage.options.overlay]
mount_program = "/usr/bin/fuse-overlayfs"
EOF
    # The docker CLI name many harnesses call: podman's compatibility shim.
    if [ ! -e "$ROOT/usr/bin/docker" ]; then
        printf '#!/bin/sh\nexec podman "$@"\n' > "$ROOT/usr/bin/docker"
        chmod 0755 "$ROOT/usr/bin/docker"
    fi
    if [ ! -e "$ROOT/usr/bin/docker-compose" ] && [ -x "$ROOT/usr/bin/podman-compose" ]; then
        ln -s podman-compose "$ROOT/usr/bin/docker-compose"
    fi
fi

if [ "$WITH_HARBOR" = 1 ]; then
    echo "== harbor==$HARBOR_VERSION (venv /opt/harbor) =="
    chroot "$ROOT" /usr/bin/python3 -m venv /opt/harbor
    chroot "$ROOT" /opt/harbor/bin/pip install --no-cache-dir "harbor==$HARBOR_VERSION"
    ln -sf /opt/harbor/bin/harbor "$ROOT/usr/local/bin/harbor"
    chroot "$ROOT" /usr/local/bin/harbor --version
    # The example adaptor drives these flags; refuse a Harbor that lacks one
    # instead of discovering it inside a paid run.
    help="$(chroot "$ROOT" /usr/local/bin/harbor run --help 2>&1 || true)"
    for flag in --path --agent --model --n-concurrent --jobs-dir --job-name; do
        grep -q -- "$flag" <<< "$help" || die "harbor $HARBOR_VERSION: 'harbor run --help' shows no $flag; update the adaptor before baking"
    done
    rm -rf "$ROOT/root/.cache/pip"
fi

for i in "${!RUNNER_IDS[@]}"; do
    id="${RUNNER_IDS[$i]}"
    dir="${RUNNER_DIRS[$i]}"
    echo "== runner $id <- $dir =="
    install -d -m 0755 "$ROOT/opt/proof/runners/$id"
    cp -a "$dir/." "$ROOT/opt/proof/runners/$id/"
    chmod 0755 "$ROOT/opt/proof/runners/$id/run"
    for entry in inspect propose_rules; do
        [ -f "$ROOT/opt/proof/runners/$id/$entry" ] && chmod 0755 "$ROOT/opt/proof/runners/$id/$entry"
    done
done

echo "== trim =="
rm -rf "$ROOT/var/cache/apt/archives/"*.deb "$ROOT/var/lib/apt/lists/"* "$ROOT/tmp/"* "$ROOT/usr/share/doc" "$ROOT/usr/share/man" "$ROOT/usr/share/info"
: > "$ROOT/etc/machine-id"

echo "== size budget =="
TREE_MIB="$(du -sm "$ROOT" | cut -f1)"
echo "baked tree: $TREE_MIB MiB (budget $BUDGET_MIB MiB, image $SIZE_MIB MiB)"
if [ "$TREE_MIB" -gt "$BUDGET_MIB" ]; then
    die "tree exceeds the budget: drop --with-harbor / packages, or raise --budget-mib and --size-mib deliberately"
fi

echo "== image =="
mkdir -p "$OUT_DIR"
IMG="$OUT_DIR/rootfs.ext4"
rm -f "$IMG"
truncate -s "${SIZE_MIB}M" "$IMG"
mkfs.ext4 -q -F -L proof-guest -d "$ROOT" "$IMG"
HEX="$(sha256sum "$IMG" | cut -d' ' -f1)"
FINAL="$OUT_DIR/sha256-$HEX.ext4"
mv "$IMG" "$FINAL"
{
    echo "image=$FINAL"
    echo "digest=sha256:$HEX"
    echo "tree_mib=$TREE_MIB"
    echo "size_mib=$SIZE_MIB"
    echo "suite=$SUITE"
    echo "podman=$WITH_PODMAN"
    echo "harbor=${HARBOR_VERSION:-none}"
    echo "runners=${RUNNER_IDS[*]:-none}"
    echo "guest_agent_sha256=$(sha256sum "$GUEST_AGENT" | cut -d' ' -f1)"
} > "$OUT_DIR/bake-manifest.txt"

cat <<EOF

baked: $FINAL
digest: sha256:$HEX   (computed from the file above; the only source of this value)
manifest: $OUT_DIR/bake-manifest.txt

RE-LOCK (metal):
  1. KVM host:  install -m 0644 $FINAL /var/lib/proof-vm/images/
                sha256sum /var/lib/proof-vm/images/sha256-$HEX.ext4   # must print $HEX
  2. Packs:     for every topic this host serves, stage the pack tar the topic pins
                under /var/lib/proof-vm/packs/ as sha256-<hex>.tar (or the topic's
                experiment_pack_path); sha256sum it — that hex is the topic's
                constraints.params.experiment_pack_digest. Never type a digest by hand.
  3. CP env:    PROOF_RLM_VM_IMAGE_DIGEST=sha256:$HEX  (or PROOF_EXPERIMENT_VM_IMAGE_DIGEST
                when topic VMs keep another image); restart proof-challenge.
  4. Verify:    deploy/scripts/proof-vm-wire-check.sh all && boot-probe; then one
                experiment run per docs/runbooks/proof-vm-orchestrator.md § Experiment VMs.
Running VMs keep the old image until torn down.
EOF
