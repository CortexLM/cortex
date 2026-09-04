#!/bin/sh
# Put a *regular file* at /usr/bin/proof-eval (never a symlink).
set -eu

launcher=""
for candidate in \
    /tmp/proof-eval-launcher \
    /opt/proof-eval/eval/bin/proof-eval \
    /usr/bin/proof-eval
do
    if [ -f "${candidate}" ] && [ ! -L "${candidate}" ]; then
        if head -n 1 "${candidate}" | grep -q '^#!/bin/sh'; then
            launcher=${candidate}
            break
        fi
    fi
done

if [ -n "${launcher}" ] && [ "${launcher}" != /usr/bin/proof-eval ]; then
    install -m 0755 "${launcher}" /usr/bin/proof-eval
elif [ -z "${launcher}" ]; then
    install -m 0755 /opt/proof-eval/eval/bin/proof-eval /usr/bin/proof-eval
else
    chmod 0755 /usr/bin/proof-eval
fi

if [ -L /usr/bin/proof-eval ]; then
    echo "install-cli: /usr/bin/proof-eval must be a regular file, not a symlink" >&2
    ls -l /usr/bin/proof-eval >&2
    exit 1
fi
if [ ! -f /usr/bin/proof-eval ] || [ ! -x /usr/bin/proof-eval ]; then
    echo "install-cli: /usr/bin/proof-eval is not an executable file" >&2
    exit 1
fi

env -i PATH=/usr/bin:/bin /usr/bin/proof-eval --help >/dev/null
env -i PATH=/usr/bin:/bin /usr/bin/proof-eval score --help >/dev/null

echo "install-cli: /usr/bin/proof-eval is a regular file (not a symlink)"
