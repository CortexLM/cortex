"""Process-environment pins a manifest may impose on its island workers (ADR 0011, part 3).

The table is closed and append-only: a name or a value enters only with a recorded
measurement that it changes round bytes, and is never removed or narrowed, because a signed
manifest keeps its meaning forever. A manifest constrains exactly the names it lists.
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

PRESENT_VALUES: Final[Mapping[str, frozenset[str]]] = {
    "ATEN_CPU_CAPABILITY": frozenset({"avx2"}),
    "HT_ATTN_BACKEND": frozenset({"math", "sdpa-flash"}),
    "MKL_CBWR": frozenset({"AVX2"}),
    "OMP_DYNAMIC": frozenset({"FALSE"}),
    "ONEDNN_MAX_CPU_ISA": frozenset({"AVX2"}),
}
# Switches the trainer reads as `== "1"`, plus OMP_THREAD_LIMIT, which changes the OpenMP team
# size without being visible to torch.get_num_threads(): they can only be pinned unset.
ABSENT_ONLY: Final[frozenset[str]] = frozenset(
    {"HT_COMPILE", "HT_FP8", "HT_MOE_GROUPED", "OMP_THREAD_LIMIT"}
)


@dataclass(frozen=True, slots=True)
class EnvPin:
    """A validated pin, hashable so a probe result can be cached on it."""

    present: tuple[tuple[str, str], ...]  # sorted by name
    absent: tuple[str, ...]  # sorted, unique

    @classmethod
    def from_body(cls, body: Mapping[str, Any]) -> EnvPin:
        present, absent = body["present"], body["absent"]
        check(present, absent)
        return cls(tuple(sorted(present.items())), tuple(absent))

    def body(self) -> dict[str, Any]:
        return {"present": dict(self.present), "absent": list(self.absent)}


def check(present: Mapping[str, str], absent: Iterable[str]) -> None:
    """Refuse anything outside the table, and any non-canonical spelling of an allowed pin."""
    names = list(absent)
    if not present and not names:
        raise ValueError("an env pin must name at least one variable")
    for name, value in sorted(present.items()):
        if value not in PRESENT_VALUES.get(name, frozenset()):
            raise ValueError(f"env pin {name}={value!r} is not an allowed pin")
    for name in names:
        if name not in ABSENT_ONLY and name not in PRESENT_VALUES:
            raise ValueError(f"env pin cannot name {name!r} as absent")
    if names != sorted(set(names)):
        raise ValueError("env pin 'absent' must be sorted and unique")
    for name in names:
        if name in present:
            raise ValueError(f"env pin names {name!r} both present and absent")


def pinned_environ(base: Mapping[str, str], pin: EnvPin | None) -> dict[str, str]:
    """``base`` with the absent names removed and the present ones set; a copy either way."""
    if pin is None:
        return dict(base)
    gone = set(pin.absent)
    return {k: v for k, v in base.items() if k not in gone} | dict(pin.present)


def divergence(live: Mapping[str, str], pin: EnvPin) -> dict[str, tuple[str | None, str | None]]:
    """Name -> (manifest value or None for absent, live value or None) for each violated pin."""
    out: dict[str, tuple[str | None, str | None]] = {
        k: (v, live.get(k)) for k, v in pin.present if live.get(k) != v
    }
    out.update({k: (None, live[k]) for k in pin.absent if k in live})
    return out


class EnvPinUnhonorable(RuntimeError):
    """This host cannot honor the pin: a process started under it was refused or never ran."""


@functools.cache
def require_honorable(pin: EnvPin) -> str:
    """Let a child started under the pinned environment check the pin; return ATen's capability.

    ATen reports what it resolved, and ignores a bogus or unsupported value silently, so a static
    look at ``/proc/cpuinfo`` would duplicate its detection and could disagree. The parent never
    imports torch. Cached per pin and process; a refusal is not cached.
    """
    argv = [sys.executable, "-m", "hypertrain.trainer.env_probe", json.dumps(pin.body())]
    try:
        done = subprocess.run(  # noqa: S603 - this interpreter, a fixed module, one JSON argument
            argv, env=pinned_environ(os.environ, pin), capture_output=True, timeout=120, check=False
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise EnvPinUnhonorable(f"environment probe could not run: {error}") from error
    if done.returncode != 0:
        raise EnvPinUnhonorable(done.stderr[-1000:].decode(errors="replace").strip())
    try:
        return str(json.loads(done.stdout)["cpu_capability"])
    except (ValueError, KeyError, TypeError) as error:
        tail = done.stdout[-200:].decode(errors="replace")
        raise EnvPinUnhonorable(f"environment probe gave no verdict: {tail!r}") from error
