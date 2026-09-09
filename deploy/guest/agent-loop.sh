#!/bin/sh
# Keeps proof-vm-guest-agent running inside the guest (baked to
# /usr/local/sbin/proof-agent-loop, exec'd by /sbin/init under catatonit).
# Every flag is the baked layout; nothing here names a topic or a runner.
set -u
: "${PROOF_GUEST_RUN_AS_UID:=1000}" "${PROOF_GUEST_RUN_AS_GID:=1000}" "${PROOF_GUEST_SCRATCH:=/var/lib/proof}"
log() { echo "proof-agent-loop: $*" > /dev/console 2>/dev/null || echo "proof-agent-loop: $*"; }
trap 'log "stopping"; sync; exit 0' TERM INT
while :; do
    # shellcheck disable=SC2086
    /usr/local/bin/proof-vm-guest-agent \
        --run-as-uid "$PROOF_GUEST_RUN_AS_UID" --run-as-gid "$PROOF_GUEST_RUN_AS_GID" \
        --pack-root "$PROOF_GUEST_SCRATCH/packs" --work-root "$PROOF_GUEST_SCRATCH/work" \
        --secrets-dir /run/proof/secrets --runners-dir /opt/proof/runners \
        ${PROOF_GUEST_ALLOW_PLAIN_HTTP:+--allow-plain-http}
    log "guest agent exited ($?); restarting in 1s"
    sleep 1
done
