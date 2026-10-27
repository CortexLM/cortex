"""The environment pin as the executor's request carries it, and as an honest container reports it.

One pin for the runner tests: the CPU class the suite honors and the B300 device class, so the
fixtures name the classes through ``class_pin`` and never copy the runner's own name lists.
"""

from __future__ import annotations

from typing import Any

from hypertrain.protocol.env_pins import EnvPin, class_pin, observed_environ, pinned_environ

PIN = EnvPin.from_body(class_pin("avx2", "sm_103"))
CAPABILITY = (10, 3)


def request_pin() -> dict[str, Any]:
    """What the executor adds to request.json for ``PIN``: the names to set (merged into the
    ``env`` mapping by the caller), the names to remove and the GPU class."""
    return {"env": dict(PIN.present), "unset": list(PIN.absent), "gpu": PIN.gpu}


def honest_report(devices: int = 1) -> dict[str, Any]:
    """The probe fields of a container that honors ``PIN``, with the devices it reports."""
    return {
        "pinned_env": observed_environ(pinned_environ({}, PIN), PIN),
        "cpu_capability": PIN.capability,
        "capabilities": [list(CAPABILITY)] * devices,
    }
