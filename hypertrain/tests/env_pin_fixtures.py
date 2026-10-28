"""Pin bodies and observations for tests of ``reference_spec.env.pinned`` (ADR 0011, part 3).

Production code reads only the manifest and the class tables of ``protocol/env_pins.py``; the
tests name the classes through ``class_pin``.
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

from hypertrain.protocol.env_pins import EnvPin, class_pin, observed_environ, pinned_environ

CPU_AVX2: dict[str, Any] = class_pin("avx2", "none")
CPU_AVX512: dict[str, Any] = class_pin("avx512", "none")
# The pin the suite's manifests name and the test process honors (tests/process_pin.py).
TEST_PIN = EnvPin.from_body(CPU_AVX2)


def honest_observation(pin: dict[str, Any] | EnvPin) -> dict[str, Any]:
    """What a process that honors ``pin`` declares: the observation of an honest host."""
    p = pin if isinstance(pin, EnvPin) else EnvPin.from_body(pin)
    return {
        "env": observed_environ(pinned_environ({}, p), p),
        "cpu_capability": p.capability,
        "gpu": p.gpu,
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


def _has_flag(flag: str) -> bool:
    if platform.machine() not in ("x86_64", "AMD64"):
        return False
    try:
        flags = Path("/proc/cpuinfo").read_text()
    except OSError:
        return False
    return re.search(rf"\b{flag}\b", flags) is not None


# A pinned CPU manifest is refused by design on a host that cannot honor it.
needs_avx2 = pytest.mark.skipif(not _has_flag("avx2"), reason="the CPU pins need an AVX2 host")
needs_avx512 = pytest.mark.skipif(
    not _has_flag("avx512f"), reason="the avx512 class needs an AVX-512 host"
)


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
