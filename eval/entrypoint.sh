#!/bin/sh
# Proof eval image entrypoint.
#
#   serve     keep the pod reachable over SSH so harvest can stage request.json
#   score … | baseline … | selftest | --help
#             run /usr/bin/proof-eval (same binary harvest SSHes into)
set -eu

install_authorized_keys() {
    keys=""
    for name in PUBLIC_KEY SSH_PUBLIC_KEY SSH_PUBLIC_KEYS LIUM_SSH_PUBLIC_KEY; do
        value=$(printenv "$name" 2>/dev/null || true)
        if [ -n "$value" ]; then
            keys="${keys}${value}
"
        fi
    done
    if [ -z "$keys" ]; then
        return 0
    fi
    mkdir -p /root/.ssh
    printf '%s' "$keys" >> /root/.ssh/authorized_keys
    chmod 700 /root/.ssh
    chmod 600 /root/.ssh/authorized_keys
}

serve() {
    install_authorized_keys
    if [ ! -f /etc/ssh/ssh_host_ed25519_key ]; then
        ssh-keygen -A
    fi
    mkdir -p /run/sshd
    /usr/sbin/sshd -D -e \
        -o PermitRootLogin=prohibit-password \
        -o PasswordAuthentication=no \
        -o KbdInteractiveAuthentication=no
}

case "${1:-serve}" in
    serve)
        serve
        ;;
    *)
        exec /usr/bin/proof-eval "$@"
        ;;
esac
