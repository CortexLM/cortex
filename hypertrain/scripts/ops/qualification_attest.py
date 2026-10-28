#!/usr/bin/env python3
"""Attest one qualified host: its SSH host key signs a statement about its result.json.

Run on the operator machine, once per host, after the host's output directory was rescued. It
reads the same executor config the CUDA executor uses (the host's address, user, identity file and
pinned known_hosts file), runs two fixed commands over that channel (the host's GPU UUIDs, drivers
and boot id; `ssh-keygen -Y sign` with the host's own key), checks the signature against the key
pinned in known_hosts, and writes attestation.json and attestation.sig beside result.json. Nothing
is staged on the host. A file already there is never replaced: a container can plant one in its
output directory, so delete it and investigate before you retry.

The fingerprint it prints is the one the owner types into `backend_bootstrap.py --host-fingerprint`
after comparing it with the provider console. The signature proves that the holder of the pinned
key agreed to the statement, not where the computation ran.
"""

import argparse
import shutil
import sys
import tempfile
import time
from pathlib import Path

from hypertrain.gpu_ops import cuda_executor, host_attest

REMOTE_SECONDS = 60
MAX_REMOTE_OUTPUT = 64 << 10

p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
p.add_argument("--executor-config", type=Path, required=True)
p.add_argument("--host", required=True, help="label of the host in the executor config")
p.add_argument("--result", type=Path, required=True, help="the host's rescued result.json")
p.add_argument("--host-key-path", default=host_attest.HOST_KEY_PATH, help="on the host")
p.add_argument("--no-sudo", action="store_true", help="the SSH user can read the host key")
a = p.parse_args()

config = cuda_executor.ExecutorConfig.model_validate_json(a.executor_config.read_bytes())
host = next((h for h in config.hosts if h.label == a.host), None)
ssh = shutil.which("ssh")
if host is None or ssh is None:
    sys.exit(f"no host {a.host!r} in {a.executor_config}" if host is None else "ssh not found")
transport = cuda_executor.SshTransport(host, config, ssh)
pin = host_attest.pinned_key(host.known_hosts_file.read_bytes())


def run(command: str, stdin: bytes) -> bytes:
    with tempfile.TemporaryDirectory(prefix="ht-attest-") as scratch:
        files = {name: Path(scratch, name) for name in ("stdin", "stdout", "stderr")}
        files["stdin"].write_bytes(stdin)
        code = transport.run(
            command,
            stdin=files["stdin"],
            stdout=files["stdout"],
            stderr=files["stderr"],
            deadline=time.time() + REMOTE_SECONDS,
            cancel=None,
            max_stdout_bytes=MAX_REMOTE_OUTPUT,
        )
        if code:
            tail = files["stderr"].read_bytes()[-400:].decode("utf-8", "replace")
            sys.exit(f"{command.split()[0]} failed on {a.host} with exit {code}: {tail}")
        return files["stdout"].read_bytes()


raw, signature = host_attest.attest(
    run, a.result, key_path=a.host_key_path, sudo=not a.no_sudo, pin=pin
)
host_attest.write_files(a.result, raw, signature)
print(f"host {a.host}: fingerprint {host_attest.fingerprint(pin)}")
print(f"wrote {a.result.parent / host_attest.ATTESTATION}")
print(f"wrote {a.result.parent / host_attest.SIGNATURE}")
