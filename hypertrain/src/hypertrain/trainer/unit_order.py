"""Layout-invariant "unit-order" gradient reduction (heterogeneous compute, plan 1.8 / 6.4).

Opt-in only: reached solely through ``train_unit_order`` (run reduction_order "unit"); the v1
rank-order paths (loop.train_round, island.train_island) and their leaf bytes are untouched.

Each step's global batch is ``units`` fixed units of ``micro_batch`` consecutive sample ids;
rank r of W computes units [r*U/W, (r+1)*U/W), one forward/backward per unit, fp32 grads. Unit
grads are all_gathered (a copy, never all_reduce), summed left to right in unit order 0..U-1 and
divided by U. Every rank then steps the full tensors with optim.step (global norm in sorted-name
order). A unit's gradient does not depend on which rank computed it, so every W dividing U gives
the same bits. Leaves are ht-leaf-v2 (leaf_v2.leaf_digest).
"""

from __future__ import annotations

from collections.abc import Sequence
from contextlib import nullcontext
from dataclasses import dataclass

import torch
from torch import Tensor

from hypertrain.protocol.hashing import MerkleTree
from hypertrain.trainer.compress import compress, payload_hash, state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.determinism import require_threads
from hypertrain.trainer.island import Comm, DistComm, forbid_reductions
from hypertrain.trainer.leaf_v2 import leaf_digest
from hypertrain.trainer.loop import Assignment, SampleFn, _load_batch, _norm, batch_hash
from hypertrain.trainer.model import forward, param_shapes
from hypertrain.trainer.optim import OptState, fixed_sum, init_state, lr_at, step

Params = dict[str, Tensor]


@dataclass
class UnitOrderResult:
    leaves: list[bytes]
    leaves_root: str
    final_theta: Params
    final_state: OptState
    final_theta_hash: str
    delta_payload: bytes
    delta_hash: str
    ef_out: Params
    ef_out_hash: str


def _unit_grads(
    cfg: TrainConfig, theta: Params, ids: Sequence[int], get: SampleFn
) -> tuple[Params, Tensor]:
    """Per-unit grads stacked on dim 0 ([n_units, *shape]) and per-unit CE (float64)."""
    names, mb = sorted(theta), cfg.inner.micro_batch
    per: list[list[Tensor]] = []
    ces: list[Tensor] = []
    for u in range(len(ids) // mb):
        params = {n: theta[n].detach().requires_grad_(True) for n in names}
        tokens = _load_batch(cfg, ids[u * mb : (u + 1) * mb], get, theta[names[0]].device)
        ce, aux = forward(cfg.model, params, tokens)
        gs = torch.autograd.grad(
            ce + cfg.model.aux_loss_coef * aux, [params[n] for n in names], allow_unused=True
        )
        per.append(
            [torch.zeros_like(theta[n]) if g is None else g for n, g in zip(names, gs, strict=True)]
        )
        ces.append(ce.detach().double().reshape(1))
    return {n: torch.stack([p[i] for p in per]) for i, n in enumerate(names)}, torch.cat(ces)


def _reduce_units(comm: Comm, local: Params, units: int) -> Params:
    out: Params = {}
    for n in sorted(local):
        gathered = torch.cat(comm.all_gather(local[n]))  # [units, *shape] in unit order
        acc = gathered[0].clone()
        for u in range(1, units):
            acc.add_(gathered[u])
        out[n] = acc.div_(units)
    return out


def train_unit_order(
    cfg: TrainConfig,
    comm: Comm,
    theta_start: Params,
    a: Assignment,
    get_sample: SampleFn,
    units: int,
    ef_in: Params | None = None,
) -> UnitOrderResult:
    """One H-step round; every rank returns the same result for any world dividing ``units``."""
    require_threads(cfg.cpu_threads)
    if units < 1 or units % comm.world:
        raise ValueError(f"units={units} must be a positive multiple of world {comm.world}")
    if cfg.inner.grad_accum != 1:
        raise ValueError("unit order replaces grad_accum: set grad_accum=1")
    if sorted(theta_start) != sorted(param_shapes(cfg.model)):
        raise ValueError("theta_start does not match the model parameter set")
    if any(x.dtype != torch.float32 for x in theta_start.values()):
        raise ValueError("theta_start must be fp32 master weights")
    H, J, mb = cfg.inner.H, cfg.inner.J, cfg.inner.micro_batch
    per_step = units * mb
    if len(a.sample_ids) != H * per_step:
        raise ValueError("assignment length must equal H * units * micro_batch")
    guard = forbid_reductions() if isinstance(comm, DistComm) else nullcontext()
    mine = per_step // comm.world
    theta = {n: x.clone() for n, x in sorted(theta_start.items())}
    st = init_state(cfg.inner, theta)

    def leaf(t: int, loss: float, norm: float) -> bytes:
        ids = a.sample_ids[max(0, t - J) * per_step : t * per_step] if t else ()
        return leaf_digest(a.run_id, a.w, t, theta, st.m, st.v, batch_hash(ids), loss, norm)

    leaves = [leaf(0, 0.0, 0.0)]
    window_start = {n: x.clone() for n, x in theta.items()}
    with guard:
        for t in range(1, H + 1):
            batch = a.sample_ids[(t - 1) * per_step : t * per_step]
            ids = batch[comm.rank * mine : (comm.rank + 1) * mine]
            local, ces = _unit_grads(cfg, theta, ids, get_sample)
            red = _reduce_units(comm, local, units)
            del local
            all_ce = torch.cat(comm.all_gather(ces))
            loss = fixed_sum(list(all_ce)) / units
            step(cfg.inner, theta, red, st, lr_at(cfg.inner, a.global_step0 + t - 1, t - 1))
            if t % J == 0:
                leaves.append(leaf(t, float(loss), _norm(theta, window_start)))
                window_start = {n: x.clone() for n, x in theta.items()}
    if ef_in is None:
        ef_in = {n: torch.zeros_like(x) for n, x in theta.items()}
    payload, ef_out = compress(cfg.compress, {n: theta_start[n] - theta[n] for n in theta}, ef_in)
    return UnitOrderResult(
        leaves=leaves,
        leaves_root=MerkleTree(leaves).root.hex(),
        final_theta=theta,
        final_state=st,
        final_theta_hash=state_hash(theta),
        delta_payload=payload,
        delta_hash=payload_hash(payload),
        ef_out=ef_out,
        ef_out_hash=state_hash(ef_out),
    )
