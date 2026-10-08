"""Spec-level single-process reference for data-parallel island layouts (ep_size == 1).

Written from the todo-11 spec using only todo-6 primitives (no island code): per step the global
batch is n_gpus * micro_batch * grad_accum ids; rank r takes slice r; each rank's gradient is the
sum over its micro-batches of grad((ce + aux_coef * aux) / grad_accum); the reduced gradient is the
rank-order fp32 sum divided by n_gpus; one optimizer step on the full model; the leaf loss is the
rank-order mean of per-rank losses; leaves follow ht-leaf-v1 over all ranks' ids.
"""

from __future__ import annotations

import math

import numpy as np
import torch

from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import Layout, LeafPreimage, f32hex
from hypertrain.trainer.compress import compress, payload_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, SampleFn, batch_hash, stage_states
from hypertrain.trainer.model import forward
from hypertrain.trainer.optim import OptState, init_state, lr_at, step
from hypertrain.trainer.rng import rng_ctr

Params = dict[str, torch.Tensor]


def _tokens(cfg: TrainConfig, ids: tuple[int, ...], get: SampleFn) -> torch.Tensor:
    return torch.from_numpy(np.stack([np.asarray(get(i)).astype(np.int64) for i in ids]))


def _leaf(
    cfg: TrainConfig,
    a: Assignment,
    t: int,
    ids: tuple[int, ...],
    th: Params,
    st: OptState,
    loss: float,
    norm: float,
) -> bytes:
    pre = LeafPreimage(
        run_id=a.run_id,
        w=a.w,
        t=t,
        stages=stage_states(cfg, th, st),
        batch_ids_sha256=batch_hash(ids),
        rng_ctr=rng_ctr(a.run_id, a.w, t, -1),
        loss_f32=f32hex(loss),
        norm_f32=f32hex(norm),
    )
    return bytes.fromhex(pre.digest())


def reference_round(
    cfg: TrainConfig, lay: Layout, theta0: Params, a: Assignment, get: SampleFn
) -> dict[str, object]:
    assert lay.ep_size == 1, "per-rank recomputation is bitwise only without expert sharding"
    n, mb, ga, J = lay.n_gpus, cfg.inner.micro_batch, cfg.inner.grad_accum, cfg.inner.J
    per = mb * ga
    names = sorted(theta0)
    theta = {k: v.clone() for k, v in theta0.items()}
    st = init_state(cfg.inner, theta)
    leaves = [_leaf(cfg, a, 0, (), theta, st, 0.0, 0.0)]
    leaf_thetas = [{k: v.clone() for k, v in theta.items()}]
    window = {k: v.clone() for k, v in theta.items()}
    for t in range(1, cfg.inner.H + 1):
        step_ids = a.sample_ids[(t - 1) * per * n : t * per * n]
        rank_grads: list[Params] = []
        rank_loss: list[float] = []
        for r in range(n):
            ids = step_ids[r * per : (r + 1) * per]
            g_r = {k: torch.zeros_like(theta[k]) for k in names}
            loss_r = 0.0
            for j in range(ga):
                p = {k: theta[k].detach().requires_grad_(True) for k in names}
                ce, aux = forward(cfg.model, p, _tokens(cfg, ids[j * mb : (j + 1) * mb], get))
                total = (ce + cfg.model.aux_loss_coef * aux) / ga
                gs = torch.autograd.grad(total, [p[k] for k in names], allow_unused=True)
                for k, g in zip(names, gs, strict=True):
                    if g is not None:
                        g_r[k].add_(g)
                loss_r += float(ce.detach())
            rank_grads.append(g_r)
            rank_loss.append(loss_r / ga)
        red = {}
        for k in names:
            acc = rank_grads[0][k].clone()
            for r in range(1, n):
                acc.add_(rank_grads[r][k])
            red[k] = acc.div_(n)
        step(cfg.inner, theta, red, st, lr_at(cfg.inner, a.global_step0 + t - 1, t - 1))
        loss = rank_loss[0]
        for x in rank_loss[1:]:
            loss += x
        loss /= n
        if t % J == 0:
            total_sq = 0.0
            for k in names:
                total_sq += float((theta[k].double() - window[k].double()).pow(2).sum())
            norm = float(np.float32(math.sqrt(total_sq)))
            ids_w = a.sample_ids[(t - J) * per * n : t * per * n]
            leaves.append(_leaf(cfg, a, t, ids_w, theta, st, loss, norm))
            leaf_thetas.append({k: v.clone() for k, v in theta.items()})
            window = {k: v.clone() for k, v in theta.items()}
    payload, _ = compress(
        cfg.compress,
        {k: theta0[k] - theta[k] for k in names},
        {k: torch.zeros_like(v) for k, v in theta.items()},
    )
    return {
        "leaves_root": MerkleTree(leaves).root.hex(),
        "leaves": [x.hex() for x in leaves],
        "delta_hash": payload_hash(payload),
        "final_theta": theta,
        "leaf_thetas": leaf_thetas,
    }
