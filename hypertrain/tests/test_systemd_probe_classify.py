from __future__ import annotations

import pytest

from systemd_probe import classify_systemd_probe

DENIED = [
    "Failed to start transient service unit: Interactive authentication required.",
    "Failed to start transient service unit: Access denied",
    "Failed to connect to bus: No such file or directory",
    "System has not been booted with systemd as init system (PID 1). Can't operate.",
    "Failed to create bus connection: Permission denied",
]


def test_ok() -> None:
    assert classify_systemd_probe(0, "") == "ok"


@pytest.mark.parametrize("stderr", DENIED)
def test_unavailable(stderr: str) -> None:
    assert classify_systemd_probe(1, stderr) == "unavailable"


@pytest.mark.parametrize(
    "stderr",
    ["Failed to start transient service unit: Resource temporarily unavailable (EAGAIN)", ""],
)
def test_error(stderr: str) -> None:
    assert classify_systemd_probe(1, stderr) == "error"
