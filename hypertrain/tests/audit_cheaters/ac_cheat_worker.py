"""Torchrun worker for the worker-level cheating doubles of ac_cheats.py (cheats 1 and 5).

Run as ``python -m torch.distributed.run ... -m ac_cheat_worker <attempt> <backend>`` in place of
``hypertrain.miner.island_worker``. It changes the arithmetic of one training step inside the
production worker and nothing else, so every artifact it publishes is self-consistent and passes
the miner's own ``validate_artifacts``: detection can only come from an independent replay.

The choice arrives as JSON in HT_AC_CHEAT: ``{"kind": "ulp" | "skip", "step": int | null}``;
``null`` means the last inner step. A step the run never reaches fails the worker, so a
mis-parameterised double cannot silently publish honest work.
"""

from __future__ import annotations

import hypertrain.trainer  # noqa: F401

# isort: split
import json
import os
import sys
from pathlib import Path
from typing import Any

import torch

import hypertrain.miner.island_worker as honest
import hypertrain.trainer.island as island

ENV = "HT_AC_CHEAT"


def _target(cfg: Any, spec: dict[str, Any]) -> int:
    target = cfg.inner.H if spec["step"] is None else int(spec["step"])
    if not 1 <= target <= cfg.inner.H:
        raise RuntimeError(f"cheat step {target} outside the {cfg.inner.H} inner steps")
    return target


def _one_ulp_after_step(spec: dict[str, Any]) -> None:
    train_island = honest.train_island

    def cheating(cfg: Any, *args: Any, **kwargs: Any) -> Any:
        target, fired = _target(cfg, spec), []

        def after_step(t: int, theta: dict[str, torch.Tensor]) -> None:
            if t != target:
                return
            first = theta[sorted(theta)[0]].view(-1)
            first[0] = torch.nextafter(first[0], torch.tensor(float("inf")))
            fired.append(t)

        result = train_island(cfg, *args, after_step=after_step, **kwargs)
        if not fired:
            raise RuntimeError("one-ulp cheat never fired")
        return result

    honest.train_island = cheating


def _skip_optimizer_step(spec: dict[str, Any]) -> None:
    train_island, real_step = honest.train_island, island.step

    def cheating(cfg: Any, *args: Any, **kwargs: Any) -> Any:
        target, calls = _target(cfg, spec), []

        def step(inner: Any, params: Any, grads: Any, st: Any, *rest: Any, **kw: Any) -> None:
            calls.append(1)
            if len(calls) == target:
                st.step += 1  # the step counter advances; the weights and moments do not
                return
            real_step(inner, params, grads, st, *rest, **kw)

        island.step = step
        try:
            return train_island(cfg, *args, **kwargs)
        finally:
            island.step = real_step

    honest.train_island = cheating


INSTALL = {"ulp": _one_ulp_after_step, "skip": _skip_optimizer_step}


def main() -> None:
    spec = json.loads(os.environ[ENV])
    INSTALL[spec["kind"]](spec)
    honest.execute(Path(sys.argv[1]), sys.argv[2])


if __name__ == "__main__":
    main()
