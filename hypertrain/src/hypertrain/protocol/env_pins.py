"""Process-environment pin of a manifest: one CPU class and one GPU class (ADR 0011, part 3).

Every manifest pins its environment. It names one row of ``CPU_CLASSES`` (the ISA the host CPU
code paths are held to) and one row of ``GPU_CLASSES`` (the device the run uses, or ``none`` for
a CPU-only run) and carries the environment those rows imply, ``present`` and ``absent``, which
must equal the rows exactly. The tables are closed and append-only: a row is never edited (a
different environment is a new row), because a signed manifest keeps its meaning forever.
Stdlib only, so the launcher, the preflight and the manifest model never import torch.
"""

from __future__ import annotations

import functools
import json
import os
import subprocess
import sys
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from typing import Any, Final

# Switches the trainer reads as `== "1"`: they can only be pinned unset.
TRAINER_SWITCHES: Final[tuple[str, ...]] = ("HT_COMPILE", "HT_FP8", "HT_MOE_GROUPED")


@dataclass(frozen=True, slots=True)
class CpuClass:
    """The environment that holds ATen, MKL and oneDNN to one instruction set."""

    present: Mapping[str, str]
    absent: tuple[str, ...]
    capability: str  # what torch.backends.cpu.get_cpu_capability() reports once it is honored


@dataclass(frozen=True, slots=True)
class GpuClass:
    """The device a run uses, by compute capability, and the attention backends measured on it."""

    attention: frozenset[str]  # accepted HT_ATTN_BACKEND values
    device: tuple[int, int] | None  # (major, minor); None: the run uses no GPU
    sm_counts: frozenset[int]  # SM counts measured on the device


# OMP_THREAD_LIMIT changes the OpenMP team size without being visible to torch.get_num_threads().
CPU_CLASSES: Final[Mapping[str, CpuClass]] = {
    "avx2": CpuClass(
        present={
            "ATEN_CPU_CAPABILITY": "avx2",
            "MKL_CBWR": "AVX2",
            "OMP_DYNAMIC": "FALSE",
            "ONEDNN_MAX_CPU_ISA": "AVX2",
        },
        absent=("OMP_THREAD_LIMIT",),
        capability="AVX2",
    ),
    "avx512": CpuClass(
        present={
            "ATEN_CPU_CAPABILITY": "avx512",
            "MKL_CBWR": "AVX512",
            "OMP_DYNAMIC": "FALSE",
            "ONEDNN_MAX_CPU_ISA": "AVX512_CORE",
        },
        absent=("OMP_THREAD_LIMIT",),
        capability="AVX512",
    ),
}
GPU_CLASSES: Final[Mapping[str, GpuClass]] = {
    "none": GpuClass(attention=frozenset({"math"}), device=None, sm_counts=frozenset()),
    "sm_103": GpuClass(
        attention=frozenset({"math", "sdpa-flash"}), device=(10, 3), sm_counts=frozenset({148})
    ),
    "sm_120": GpuClass(
        attention=frozenset({"math", "sdpa-flash"}), device=(12, 0), sm_counts=frozenset({170})
    ),
}
ATTENTION: Final[str] = "HT_ATTN_BACKEND"
OBSERVATION_KEYS: Final[frozenset[str]] = frozenset({"env", "cpu_capability", "gpu"})


def gpu_label(capability: tuple[int, int] | None) -> str:
    """The class name of a device's compute capability, ``none`` when the run has no GPU."""
    return "none" if capability is None else f"sm_{capability[0]}{capability[1]}"


def _expected(cpu: str, gpu: str, attention: str) -> tuple[dict[str, str], list[str]]:
    if cpu not in CPU_CLASSES:
        raise ValueError(f"env pin cpu class {cpu!r} is not in the table")
    if gpu not in GPU_CLASSES:
        raise ValueError(f"env pin gpu class {gpu!r} is not in the table")
    c, g = CPU_CLASSES[cpu], GPU_CLASSES[gpu]
    if attention not in g.attention:
        raise ValueError(f"env pin {ATTENTION}={attention!r} is not measured on gpu class {gpu!r}")
    return {**c.present, ATTENTION: attention}, sorted({*c.absent, *TRAINER_SWITCHES})


def class_pin(cpu: str, gpu: str, attention: str = "math") -> dict[str, Any]:
    """The manifest body of a CPU class and a GPU class, the one place a builder gets it from."""
    present, absent = _expected(cpu, gpu, attention)
    return {"cpu": cpu, "gpu": gpu, "present": present, "absent": absent}


def check(cpu: str, gpu: str, present: Mapping[str, str], absent: Iterable[str]) -> None:
    """Refuse a pin that is not exactly the rows it names, in any spelling."""
    names = list(absent)
    attention = present.get(ATTENTION)
    if attention is None:
        raise ValueError(f"env pin must pin {ATTENTION}")
    want_present, want_absent = _expected(cpu, gpu, attention)
    if dict(present) != want_present:
        diff = sorted(n for n in {*present, *want_present} if present.get(n) != want_present.get(n))
        raise ValueError(f"env pin present differs from the classes {cpu}/{gpu}: {', '.join(diff)}")
    if names != want_absent:
        raise ValueError(f"env pin absent differs from the classes {cpu}/{gpu}: {want_absent}")


@dataclass(frozen=True, slots=True)
class EnvPin:
    """A validated pin, hashable so a probe result can be cached on it."""

    cpu: str
    gpu: str
    present: tuple[tuple[str, str], ...]  # sorted by name
    absent: tuple[str, ...]  # sorted, unique

    @classmethod
    def from_body(cls, body: Mapping[str, Any]) -> EnvPin:
        cpu, gpu, present, absent = body["cpu"], body["gpu"], body["present"], body["absent"]
        check(cpu, gpu, present, absent)
        return cls(cpu, gpu, tuple(sorted(present.items())), tuple(absent))

    def body(self) -> dict[str, Any]:
        return {
            "cpu": self.cpu,
            "gpu": self.gpu,
            "present": dict(self.present),
            "absent": list(self.absent),
        }

    @property
    def capability(self) -> str:
        return CPU_CLASSES[self.cpu].capability


def pinned_environ(base: Mapping[str, str], pin: EnvPin) -> dict[str, str]:
    """``base`` with the absent names removed and the present ones set; a copy."""
    gone = set(pin.absent)
    return {k: v for k, v in base.items() if k not in gone} | dict(pin.present)


def divergence(live: Mapping[str, str], pin: EnvPin) -> dict[str, tuple[str | None, str | None]]:
    """Name -> (manifest value or None for absent, live value or None) for each violated pin."""
    out: dict[str, tuple[str | None, str | None]] = {
        k: (v, live.get(k)) for k, v in pin.present if live.get(k) != v
    }
    out.update({k: (None, live[k]) for k in pin.absent if k in live})
    return out


def observed_environ(live: Mapping[str, str], pin: EnvPin) -> dict[str, str | None]:
    """The value of every name the pin speaks about, as the process sees it (None: unset)."""
    return {name: live.get(name) for name in sorted({*dict(pin.present), *pin.absent})}


def observation_divergence(pin: EnvPin, observed: Mapping[str, Any]) -> list[str]:
    """The names, in order, in which an observed environment differs from the pin.

    ``observed`` is what a process declares about itself: ``env`` (``observed_environ``),
    ``cpu_capability`` (what ATen resolved) and ``gpu`` (``gpu_label`` of its device).
    """
    env = observed.get("env")
    seen: Mapping[str, Any] = env if isinstance(env, Mapping) else {}
    expected: dict[str, str | None] = {**dict(pin.present), **dict.fromkeys(pin.absent)}
    bad = [name for name in sorted({*expected, *seen}) if seen.get(name) != expected.get(name)]
    if observed.get("cpu_capability") != pin.capability:
        bad.append("cpu_capability")
    if observed.get("gpu") != pin.gpu:
        bad.append("gpu")
    return bad


class EnvPinUnhonorable(RuntimeError):
    """This host cannot honor the pin: a process started under it was refused or never ran."""


@functools.cache
def require_honorable(pin: EnvPin, backend: str = "cpu") -> dict[str, Any]:
    """Let a child started under the pinned environment check the pin; return what it observed.

    The child refuses (exit 1) when its environment, the ATen capability it resolved or the device
    of ``backend`` differs from the pin; its answer is the observation a miner declares.

    ATen reports what it resolved, and ignores a bogus or unsupported value silently, so a static
    look at ``/proc/cpuinfo`` would duplicate its detection and could disagree. The parent never
    imports torch. Cached per pin and backend, per process; a refusal is not cached.
    """
    argv = [sys.executable, "-m", "hypertrain.trainer.env_probe", json.dumps(pin.body()), backend]
    try:
        done = subprocess.run(  # noqa: S603 - this interpreter, a fixed module, fixed arguments
            argv, env=pinned_environ(os.environ, pin), capture_output=True, timeout=120, check=False
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise EnvPinUnhonorable(f"environment probe could not run: {error}") from error
    if done.returncode != 0:
        raise EnvPinUnhonorable(done.stderr[-1000:].decode(errors="replace").strip())
    try:
        observed = json.loads(done.stdout)
        if not isinstance(observed, dict) or not OBSERVATION_KEYS <= observed.keys():
            raise ValueError("not an observation")
        return observed
    except ValueError as error:
        tail = done.stdout[-200:].decode(errors="replace")
        raise EnvPinUnhonorable(f"environment probe gave no verdict: {tail!r}") from error
