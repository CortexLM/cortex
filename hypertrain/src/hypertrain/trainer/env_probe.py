"""Can this host honor a manifest's environment pin? (ADR 0011, part 3)

Child form, started by ``protocol.env_pins.require_honorable`` under the pinned environment:
``python -m hypertrain.trainer.env_probe '<pin json>'``. Importing this package already runs
``setup_determinism``, so torch is imported here and never in the parent.

Operator form, before a manifest is signed or a miner joins:
``python -m hypertrain.trainer.env_probe --manifest FILE`` prints the variables to export and
to unset, after checking that a process started with them is accepted.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Any

from hypertrain.protocol.env_pins import EnvPin, EnvPinUnhonorable, require_honorable
from hypertrain.trainer.determinism import (
    EnvPinMismatch,
    env_observation,
    require_env_pin,
    require_gpu_class,
)


def _pin_of(document: dict[str, Any]) -> dict[str, Any]:
    """The pin of a v1 or v2 manifest or training body (v2 wrappers keep it under ``training``)."""
    body = document.get("training", document)
    pinned = body["reference_spec"]["env"].get("pinned")
    if not isinstance(pinned, dict):
        raise ValueError("the manifest has no environment pin")
    return pinned


def _report(path: Path) -> int:
    try:
        pin = EnvPin.from_body(_pin_of(json.loads(path.read_bytes())))
        observed = require_honorable(pin, "cuda" if pin.gpu != "none" else "cpu")
    except (OSError, ValueError, KeyError, TypeError, EnvPinUnhonorable) as error:
        print(f"{path}: {error}", file=sys.stderr)
        return 1
    print(
        json.dumps(
            {
                "honorable": True,
                "cpu": pin.cpu,
                "gpu": pin.gpu,
                "cpu_capability": observed["cpu_capability"],
                "env": dict(pin.present),
                "unset": list(pin.absent),
            }
        )
    )
    return 0


def _device(backend: str) -> tuple[int, int] | None:
    """The compute capability every visible CUDA device shares; None for the CPU backend."""
    if backend == "cpu":
        return None
    import torch

    if not torch.cuda.is_available():
        raise EnvPinMismatch("environment pin mismatch: no CUDA device is visible")
    found = {torch.cuda.get_device_capability(i) for i in range(torch.cuda.device_count())}
    if len(found) != 1:
        raise EnvPinMismatch(f"environment pin mismatch: devices of {len(found)} classes")
    return next(iter(found))


def _probe(pin_json: str, backend: str) -> int:
    pin = EnvPin.from_body(json.loads(pin_json))
    try:
        require_env_pin(pin)
        device = _device(backend)
        require_gpu_class(pin, device)
    except EnvPinMismatch as error:
        print(error, file=sys.stderr)
        return 1
    print(json.dumps(env_observation(pin, device)))
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m hypertrain.trainer.env_probe")
    parser.add_argument("pin", nargs="?", help="pin body as JSON (the form require_honorable runs)")
    parser.add_argument("backend", nargs="?", choices=("cpu", "cuda"), default="cpu")
    parser.add_argument("--manifest", type=Path, help="report the environment a manifest pins")
    args = parser.parse_args(argv)
    if (args.pin is None) == (args.manifest is None):
        parser.error("give either a pin body or --manifest FILE")
    return _report(args.manifest) if args.manifest else _probe(args.pin, args.backend)


if __name__ == "__main__":
    sys.exit(main())
