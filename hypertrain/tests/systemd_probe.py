"""Classification of the `systemd-run` probe result (imported by conftest and its unit test)."""

from __future__ import annotations

from typing import Literal

# Messages meaning "this user/host has no usable system manager", not "the probe broke".
_SYSTEMD_UNAVAILABLE = (
    "Interactive authentication required",
    "Access denied",
    "Failed to connect to bus",
    "System has not been booted with systemd",
    "Permission denied",
)


def classify_systemd_probe(returncode: int, stderr: str) -> Literal["ok", "unavailable", "error"]:
    if returncode == 0:
        return "ok"
    return "unavailable" if any(m in stderr for m in _SYSTEMD_UNAVAILABLE) else "error"
