"""Pin bodies for tests of ``reference_spec.env.pinned`` (ADR 0011, part 3).

The presets are measured per compute dtype on kimi MICRO (docs/protocol.md section 7): fp32 roots
depend on ``ATEN_CPU_CAPABILITY`` and ``MKL_CBWR``, bf16 roots on ``ONEDNN_MAX_CPU_ISA``.
Production code reads only the manifest, so the presets live here and in the docs.
"""

from __future__ import annotations

import os
import platform
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

CPU_FP32: dict[str, Any] = {
    "present": {
        "ATEN_CPU_CAPABILITY": "avx2",
        "HT_ATTN_BACKEND": "math",
        "MKL_CBWR": "AVX2",
        "OMP_DYNAMIC": "FALSE",
    },
    "absent": ["HT_COMPILE", "HT_FP8", "HT_MOE_GROUPED", "OMP_THREAD_LIMIT"],
}
CPU_BF16: dict[str, Any] = {
    "present": {**CPU_FP32["present"], "ONEDNN_MAX_CPU_ISA": "AVX2"},
    "absent": list(CPU_FP32["absent"]),
}


# Every pinned name set to the value that changes bytes, the way a careless host might export them.
HOSTILE: dict[str, str] = {
    "ATEN_CPU_CAPABILITY": "default",
    "MKL_CBWR": "COMPATIBLE",
    "ONEDNN_MAX_CPU_ISA": "ALL",
    "OMP_DYNAMIC": "TRUE",
    "HT_ATTN_BACKEND": "sdpa-flash",
    "HT_FP8": "1",
    "HT_MOE_GROUPED": "1",
    "OMP_THREAD_LIMIT": "2",
}


def _has_avx2() -> bool:
    if platform.machine() not in ("x86_64", "AMD64"):
        return False
    try:
        flags = Path("/proc/cpuinfo").read_text()
    except OSError:
        return False
    return re.search(r"\bavx2\b", flags) is not None


# A pinned CPU manifest is refused by design on a host that cannot honor it.
needs_avx2 = pytest.mark.skipif(not _has_avx2(), reason="the CPU pins need an AVX2 host")


def clean_env() -> dict[str, str]:
    """The ambient without any variable a pin may touch, whatever CI exported globally."""
    return {
        k: v
        for k, v in os.environ.items()
        if not k.startswith(("ATEN_", "MKL_", "ONEDNN_", "OMP_", "HT_")) or k == "HT_IMAGE_DIGEST"
    }


def ambient(monkeypatch: pytest.MonkeyPatch, **values: str) -> None:
    """Make this process's environment ``clean_env()`` plus ``values``, as children inherit it."""
    for name in set(os.environ) - set(clean_env()):
        monkeypatch.delenv(name)
    for name, value in values.items():
        monkeypatch.setenv(name, value)


def child(code: str, env: dict[str, str], *args: str) -> subprocess.CompletedProcess[str]:
    """Run ``code`` in a fresh interpreter: ATen, MKL and oneDNN read their variables once."""
    return subprocess.run(
        [sys.executable, "-c", code, *args],
        env=env,
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )
