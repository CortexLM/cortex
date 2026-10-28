"""Make a process honor the environment pin of the simulator's configs (stdlib and protocol only).

``build_cfg`` and ``base_cfg`` take their TrainConfig from the example manifest, so every job
trains under that manifest's pin and ``diloco.train_round`` refuses a process that does not honor
it. ATen, MKL and oneDNN read their variables at first use: call :func:`pin_process` before
anything imports torch, in an entry point and in any script that drives the simulator, whatever
its caller exported. Workers started afterwards inherit the environment.
"""

from __future__ import annotations

import os

from hypertrain.protocol.env_pins import pinned_environ
from hypertrain.protocol.example import example_manifest


def pin_process() -> None:
    pinned = pinned_environ(os.environ, example_manifest().reference_spec.env.pin())
    os.environ.clear()
    os.environ.update(pinned)
