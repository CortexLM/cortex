"""Phase B: accepted genuine network + the 10 backend/role selectors on one rented 2x RTX 5090.

Reuses scripts/vast_e2e.py (admission, CREATE, ssh, rescue-before-delete, watchdog, absence).
Uploads the repo and the phase-A bundle (scripts/genuine_cuda_ci.py output: seed, rescued
qualification trees, index-d2), restores the qualification trees at their absolute paths, runs
scripts/genuine_cuda_index.py generate on CUDA, then the selectors with -rs; fails on any skip.

  python scripts/genuine_cuda_tests.py --phase-a DIR --key-file KEY --max-usd 4 --max-minutes 90
"""

from __future__ import annotations

import importlib.util
import shlex
import sys
import time
from pathlib import Path
from typing import Any

_spec = importlib.util.spec_from_file_location("vast_e2e", Path(__file__).with_name("vast_e2e.py"))
assert _spec and _spec.loader
vast = importlib.util.module_from_spec(_spec)
sys.modules.setdefault("vast_e2e", vast)
_spec.loader.exec_module(vast)

EVIDENCE = vast.EVIDENCE.parent / "vast-genuine"
DRIVERS = {
    "570.211.01",
    "580.95.05",
    "580.159.03",
    "580.173.02",
    "580.178.04",
    "595.58.03",
    "595.71.05",
    "595.84",
}
SELECTORS = [
    "tests/challenge/test_backend_authority_v2.py::"
    "test_cuda_dispatch_at_admission_audit_and_geometry",
    "tests/challenge/test_backend_authority_v2.py::"
    "test_referee_uses_frozen_backend_before_evidence[cuda]",
    "tests/challenge/test_backend_authority_v2.py::"
    "test_referee_uses_frozen_backend_before_evidence[cpu_anchor]",
    "tests/challenge/test_backend_authority_v2.py::"
    "test_referee_uses_frozen_backend_before_evidence[missing]",
    "tests/challenge/test_backend_authority_v2.py::"
    "test_referee_uses_frozen_backend_before_evidence[mutated]",
    "tests/challenge/test_role_dispatch_consumers_v2.py::"
    "test_accepted_role_factory_called_before_kernel",
    "tests/challenge/test_role_dispatch_consumers_v2.py::"
    "test_real_adapter_audit_canonical_publication",
]
REMOTE = r"""
set -u
cd /workspace/ht
OUT=/workspace/out
mkdir -p "$OUT"
export CUBLAS_WORKSPACE_CONFIG=:4096:8 OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 PYTHONHASHSEED=0
export NVIDIA_TF32_OVERRIDE=0 CUDA_DISABLE_PTX_JIT=1 TMPDIR=/workspace/tmp
export HT_IMAGE_DIGEST=sha256:c16a28dd29df300eca0cfb5c5aeacb8947681708634e3cbc8639b530321fe2e2
mkdir -p "$TMPDIR"
PY=/opt/hypertrain/venv/bin/python
nvidia-smi --query-gpu=name,driver_version --format=csv >"$OUT/gpu.csv" 2>&1
"$PY" -m pip install -q --no-deps -e . >"$OUT/install.log" 2>&1
"$PY" -m pip install -q pytest==9.1.1 >>"$OUT/install.log" 2>&1
fail() {
  rm -rf "$OUT/genuine/generator" "$OUT/genuine/publication-scratch"
  echo "$1" >"$OUT/EXIT"; exit
}
"$PY" scripts/genuine_cuda_tests.py restore phase-a >"$OUT/restore.log" 2>&1 || fail 10
"$PY" scripts/genuine_cuda_index.py generate "$OUT/genuine" --prelim phase-a/genuine/index-d2.json \
  >"$OUT/generate.log" 2>&1 || fail 11
HT_GENUINE_CUDA_INDEX="$OUT/genuine/index.json" "$PY" -m pytest -rs -p no:cacheprovider \
  --basetemp=/workspace/bt --junitxml="$OUT/genuine-junit.xml" __SELECTORS__ \
  >"$OUT/genuine-pytest.log" 2>&1
RC=$?
if grep -q -i "skipped" "$OUT/genuine-pytest.log"; then RC=12; fi
rm -rf "$OUT/genuine/generator" "$OUT/genuine/publication-scratch" "$OUT/genuine/publication-run"
echo $RC >"$OUT/EXIT"
"""


def restore(phase_a: Path) -> int:
    import json
    import shutil
    import tarfile

    from hypertrain.gpu_ops.journal import fsha

    runtime = phase_a / "runtime"
    cfg = json.loads((runtime / "config.json").read_bytes())
    admit = json.loads((phase_a / "admit/admit.json").read_bytes())["record"]["files"]
    for role in ("h0", "h1"):
        root = Path(cfg["remote_root"].format(role=role))
        if root.exists():
            shutil.rmtree(root)
        root.mkdir(parents=True)
        with tarfile.open(phase_a / "bundle.tar") as tar:
            tar.extractall(root, filter="data")
        shutil.copyfile(phase_a / "source-profile.json", root / "profile.json")
        shutil.copyfile(phase_a / "registry.json", root / "registry.json")
        shutil.copyfile(runtime / f"cli-receipt-{role}.json", root / "receipts.json")
        shutil.copyfile(runtime / f"cli-qualification-{role}.json", root / "qualification.json")
        archive = Path(json.loads((phase_a / f"rescue-{role}.json").read_bytes())["tar"])
        with tarfile.open(phase_a / "rescue" / role / archive.name) as tar:
            tar.extractall(root, filter="data")
        assert fsha(root / "profile.json") == admit["profile"]["sha256"]
    return 0


class GenuineHarness(vast.Harness):
    label_prefix = "hypertrain-genuine"
    upload_paths = [
        "src",
        "tests",
        "scripts",
        "experiments",
        "docker",
        "pyproject.toml",
        "uv.lock",
        "README.md",
        "LICENSE",
    ]
    rsync_excludes = ["/experiments/results", "/data"]
    phase_a: Path

    def offer_query(self) -> dict[str, Any]:
        return {**vast.offer_query(), "inet_down": {"gte": 3000}, "reliability": {"gt": 0.95}}

    def eligible(self, o: Any) -> bool:
        relaxed = {
            **o,
            "reliability": "0.99",
        }
        return (
            vast.eligible(relaxed)
            and (vast.dec(o.get("reliability")) or 0) > vast.Decimal("0.95")
            and o.get("driver_version") in DRIVERS
            and o.get("machine_id") != 116630
            and (vast.dec(o.get("inet_down")) or 0) >= 3000
        )

    def upload(self) -> int:
        rc = super().upload()
        if rc:
            return rc
        ssh = shlex.join(self.ssh_argv(*self.ssh)[:-1])
        argv = [
            "rsync",
            "-az",
            "-e",
            ssh,
            f"{self.phase_a}/",
            f"root@{self.ssh[0]}:/workspace/ht/phase-a/",
        ]
        return vast.subprocess.run(argv, capture_output=True, timeout=900, check=False).returncode  # noqa: S603

    def run_remote(self) -> int:
        script = REMOTE.replace("__SELECTORS__", " ".join(shlex.quote(s) for s in SELECTORS))
        launch = f"setsid nohup bash -c {shlex.quote(script)} >/dev/null 2>&1 < /dev/null &"
        if self.sh(launch, timeout=120) != 0:
            raise RuntimeError("remote launch failed")
        status = self.out / "exit.txt"
        while time.time() < self.deadline and not self.cancel.is_set():
            self.api.sleep(30.0)
            self.sh("cat /workspace/out/EXIT 2>/dev/null", timeout=60, stdout_path=status)
            text = status.read_text().strip() if status.exists() else ""
            if text:
                self.api.log("genuine_exit", rc=text)
                return int(text)
        return 124


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv[:1] == ["restore"]:
        return restore(Path(argv[1]).resolve())
    i = argv.index("--phase-a")
    phase_a = Path(argv[i + 1]).resolve()
    del argv[i : i + 2]
    if "--evidence-dir" not in argv:
        argv += ["--evidence-dir", str(EVIDENCE)]

    class Bound(GenuineHarness):
        pass

    Bound.phase_a = phase_a
    return vast.main(argv, harness=Bound)


if __name__ == "__main__":
    sys.exit(main())
