#!/bin/sh
# Proof guest init (PID 1) for the RLM / experiment microVM image.
#
# Baked to /sbin/init by deploy/guest/bake-rootfs.sh. Firecracker boots the
# kernel with `console=ttyS0 reboot=k panic=1 pci=off [ip=...]` (see
# crates/proof-fc-host/src/jail.rs), the rootfs read-only on /dev/vda and a
# fresh, host-formatted ext4 scratch drive on /dev/vdb. This script mounts
# the kernel filesystems, puts everything writable on the scratch drive
# (packs, per-job work, the rootless container store, the run-as user's
# home), gives the guest agent a tmpfs for owner key material, and then
# keeps the agent running. It stays PID 1 and reaps children.
#
# Nothing here knows what a topic runs: the agent execs operator adaptors
# from /opt/proof/runners; see crates/proof-vm-guest.
set -u

log() { echo "proof-init: $*" > /dev/console 2>/dev/null || echo "proof-init: $*"; }

# Baked defaults (deploy/guest/bake-rootfs.sh writes /etc/proof/guest.env).
PROOF_GUEST_RUN_AS_UID=1000
PROOF_GUEST_RUN_AS_GID=1000
PROOF_GUEST_RESOLVER=""
if [ -r /etc/proof/guest.env ]; then
    # shellcheck disable=SC1091
    . /etc/proof/guest.env
fi
export PROOF_GUEST_RUN_AS_UID PROOF_GUEST_RUN_AS_GID

mount -t proc -o nosuid,nodev,noexec proc /proc 2>/dev/null || true
mount -t sysfs -o nosuid,nodev,noexec sys /sys 2>/dev/null || true
mount -t devtmpfs -o nosuid dev /dev 2>/dev/null || true
mkdir -p /dev/pts /dev/shm /run /tmp
mount -t devpts -o nosuid,noexec,gid=5,mode=620 devpts /dev/pts 2>/dev/null || true
mount -t tmpfs -o nosuid,nodev tmpfs /dev/shm 2>/dev/null || true
mount -t tmpfs -o nosuid,nodev,mode=755 tmpfs /run 2>/dev/null || true
mount -t tmpfs -o nosuid,nodev tmpfs /tmp 2>/dev/null || true
# cgroup v2 only: rootless podman with cgroup_manager=cgroupfs wants it here.
mount -t cgroup2 -o nsdelegate none /sys/fs/cgroup 2>/dev/null || true
# Rootless container runtimes: user namespaces, unprivileged ping, fuse.
[ -w /proc/sys/user/max_user_namespaces ] && echo 65536 > /proc/sys/user/max_user_namespaces
[ -w /proc/sys/net/ipv4/ping_group_range ] && echo "0 2147483647" > /proc/sys/net/ipv4/ping_group_range
[ -w /proc/sys/kernel/unprivileged_userns_clone ] && echo 1 > /proc/sys/kernel/unprivileged_userns_clone
modprobe fuse 2>/dev/null || true
modprobe tun 2>/dev/null || true

hostname proof-guest 2>/dev/null || true
ip link set lo up 2>/dev/null || true
# eth0 is configured by the kernel from the ip= boot argument on topic /
# experiment VMs; a sister-style guest has no NIC and skips this silently.
ip link set eth0 up 2>/dev/null || true
if [ -n "$PROOF_GUEST_RESOLVER" ]; then
    echo "nameserver $PROOF_GUEST_RESOLVER" > /run/resolv.conf
    mount --bind /run/resolv.conf /etc/resolv.conf 2>/dev/null || true
fi

# Writable disk: everything a run writes lands here, never on the rootfs.
SCRATCH=/var/lib/proof
mkdir -p "$SCRATCH"
if [ -b /dev/vdb ]; then
    if ! mount -o noatime /dev/vdb "$SCRATCH" 2>/dev/null; then
        log "scratch drive not mountable as ext4; formatting"
        mkfs.ext4 -q -F /dev/vdb && mount -o noatime /dev/vdb "$SCRATCH"
    fi
else
    log "no /dev/vdb scratch drive; using a tmpfs (runs will be memory-bound)"
    mount -t tmpfs tmpfs "$SCRATCH"
fi
mkdir -p "$SCRATCH/packs" "$SCRATCH/work" "$SCRATCH/home" "$SCRATCH/containers/storage" "$SCRATCH/docker"
chown "$PROOF_GUEST_RUN_AS_UID:$PROOF_GUEST_RUN_AS_GID" "$SCRATCH/work" "$SCRATCH/home" "$SCRATCH/containers" "$SCRATCH/containers/storage"
chmod 755 "$SCRATCH/packs"
# The run-as user's home (and its rootless container store) live on scratch.
mkdir -p "/home/uid$PROOF_GUEST_RUN_AS_UID"
mount --bind "$SCRATCH/home" "/home/uid$PROOF_GUEST_RUN_AS_UID" 2>/dev/null || true
mkdir -p "/home/uid$PROOF_GUEST_RUN_AS_UID/.local/share"
ln -sfn "$SCRATCH/containers" "/home/uid$PROOF_GUEST_RUN_AS_UID/.local/share/containers"
chown -R "$PROOF_GUEST_RUN_AS_UID:$PROOF_GUEST_RUN_AS_GID" "/home/uid$PROOF_GUEST_RUN_AS_UID/.local"
# XDG_RUNTIME_DIR for the rootless runtime's sockets and state, and the
# podman runroot under it. Together with $SCRATCH/containers/storage these
# are the two paths /etc/containers/storage.conf names (bake-rootfs.sh):
# both exist, both are owned by the run-as user, before any adaptor runs.
mkdir -p "/run/user/$PROOF_GUEST_RUN_AS_UID/containers"
chown -R "$PROOF_GUEST_RUN_AS_UID:$PROOF_GUEST_RUN_AS_GID" "/run/user/$PROOF_GUEST_RUN_AS_UID"
chmod 700 "/run/user/$PROOF_GUEST_RUN_AS_UID"
# Owner key material: tmpfs, handed to the run-as user by the agent.
mkdir -p /run/proof/secrets
chmod 700 /run/proof/secrets

# Rootful container engine from an operator overlay: store on scratch (the
# rootfs is read-only) and start the daemon so adaptors can talk to the
# socket. Single-tenant guest: the run-as user needs the socket. If no
# engine is baked, this is a no-op and the adaptor may start a rootless
# fallback.
if command -v dockerd >/dev/null 2>&1; then
    mkdir -p /var/lib/docker /run/docker
    mount --bind "$SCRATCH/docker" /var/lib/docker 2>/dev/null || true
    dockerd \
        --data-root /var/lib/docker \
        --exec-root /run/docker \
        --pidfile /run/docker.pid \
        --iptables=true \
        >"$SCRATCH/dockerd.log" 2>&1 &
    i=0
    while [ "$i" -lt 50 ]; do
        if [ -S /var/run/docker.sock ]; then
            chmod 666 /var/run/docker.sock 2>/dev/null || true
            break
        fi
        if [ -S /run/docker.sock ]; then
            chmod 666 /run/docker.sock 2>/dev/null || true
            break
        fi
        i=$((i + 1))
        sleep 0.1
    done
    if [ ! -S /var/run/docker.sock ] && [ ! -S /run/docker.sock ]; then
        log "container engine did not create a socket; adaptors that need one will fail closed"
    fi
fi

export PROOF_GUEST_SCRATCH="$SCRATCH"
LOOP=/usr/local/sbin/proof-agent-loop
log "starting proof-vm-guest-agent (vsock :5000)"
# catatonit (shipped with podman) is a real PID 1: it reaps every orphan a
# container run leaves behind and forwards signals to the loop.
if command -v catatonit > /dev/null 2>&1; then
    exec catatonit -g -- "$LOOP"
fi
log "catatonit missing; running the agent loop as PID 1 (orphans are not reaped)"
exec "$LOOP"
