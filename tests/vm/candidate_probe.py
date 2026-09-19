"""Run with Linux user namespaces enabled: python -m tests.vm.candidate_probe.

Exercises real bubblewrap/seccomp only; no Firecracker, provider or external service.
"""

import json
import tempfile
from pathlib import Path

from cortex.vm.candidate import _run_isolated
from cortex.vm.models import VmError

PROBE = """
import ctypes,json,os,socket
from pathlib import Path
data=json.load(__import__('sys').stdin)
denied=[]
for name,path in data['private'].items():
    try: Path(path).read_bytes()
    except OSError: denied.append(name)
for name,family in [('inet',socket.AF_INET),('unix',socket.AF_UNIX),('vsock',40)]:
    try: socket.socket(family,socket.SOCK_STREAM)
    except OSError: denied.append(name)
try: Path('/artifact/probe.py').write_text('mutated')
except OSError: denied.append('artifact-write')
try: Path('/workspace/report.json').write_text('{"quality":1.0}')
except OSError: denied.append('report-write')
try: Path('/proc/1/root/workspace').iterdir().__next__()
except OSError: denied.append('proc-escape')
libc=ctypes.CDLL(None,use_errno=True)
if libc.unshare(0x10000000) == -1: denied.append('userns')
Path('/work/result').write_text('scratch works')
print(json.dumps({'uid':os.getuid(),'denied':sorted(denied),'env':dict(os.environ),'echo':data['input']}))
"""


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="candidate-probe-") as temporary:
        root = Path(temporary)
        artifact = root / "artifact"
        artifact.mkdir()
        (artifact / "probe.py").write_text(PROBE)
        private = root / "holdout"
        private.write_text("owner-private-answer")
        result = _run_isolated(
            artifact,
            ["/usr/bin/python3", "/artifact/probe.py"],
            {
                "input": [1, 2, 3],
                "private": {
                    "holdout": str(private),
                    "host-etc": "/etc/passwd",
                    "runtime": "/opt/cortex/bin/python",
                },
            },
            wall_seconds=10,
            memory_mib=256,
        )
        assert result["uid"] == 65534, result
        assert result["echo"] == [1, 2, 3], result
        assert result["denied"] == sorted(
            [
                "holdout",
                "host-etc",
                "runtime",
                "inet",
                "unix",
                "vsock",
                "artifact-write",
                "report-write",
                "proc-escape",
                "userns",
            ]
        ), result
        assert set(result["env"]).issubset(
            {"PATH", "LANG", "HOME", "LC_CTYPE", "PWD", "LD_LIBRARY_PATH"}
        ), result
        assert private.read_text() == "owner-private-answer"
        for source, expected in [
            ("while True: pass", "deadline"),
            ("print('x'*2000000)", "execution failed"),
        ]:
            (artifact / "abuse.py").write_text(source)
            try:
                _run_isolated(
                    artifact,
                    ["/usr/bin/python3", "/artifact/abuse.py"],
                    {},
                    wall_seconds=1,
                    memory_mib=256,
                )
            except VmError as error:
                assert expected in error.reason or (
                    expected == "deadline" and "execution failed" in error.reason
                )
            else:
                raise AssertionError("unbounded candidate was accepted")
        print(
            json.dumps(
                {
                    "isolation": "passed",
                    "denied_surfaces": len(result["denied"]),
                    "resource_abuse": "refused",
                }
            )
        )


if __name__ == "__main__":
    main()
