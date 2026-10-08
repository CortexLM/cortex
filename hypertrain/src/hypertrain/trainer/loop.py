"""Inner loop over H steps with ht-leaf-v1 leaves every J steps, delta compression, and replay.

leaf_t (t = 0, J, ..., H) = protocol LeafPreimage(...).digest(), the only leaf hash, with
  stages[s] = (TH(theta_t^s), TH(m_t^s), TH(v_t^s)), batch_ids_sha256 = sha256(u64le batch ids
  of steps t-J+1..t; empty for t=0), rng_ctr = 53-bit Philox key of (run_id, w, t, -1).
loss_t = mean CE of step t, norm_t = ||theta_t - theta_{t-J}||_2 (both 0 at t = 0).
leaves_root = MerkleTree(leaves).root.
"""

from __future__ import annotations

import hashlib
import math
import struct
from collections.abc import Callable, Sequence
from dataclasses import dataclass

import numpy as np
import numpy.typing as npt
import torch
from torch import Tensor

from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import LeafPreimage, StageState, f32hex
from hypertrain.trainer.compress import compress, payload_hash, state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.determinism import require_threads
from hypertrain.trainer.model import forward, param_shapes, stage_of
from hypertrain.trainer.optim import OptState, init_state, lr_at, step
from hypertrain.trainer.rng import rng_ctr

Params = dict[str, Tensor]
SampleFn = Callable[[int], npt.NDArray[np.uint32]]
StepHook = Callable[[int, Params], None]


@dataclass(frozen=True)
class Assignment:
    run_id: str
    w: int
    sample_ids: tuple[int, ...]
    global_step0: int = 0

    def batch_ids(self, cfg: TrainConfig, t: int) -> tuple[int, ...]:
        per = cfg.inner.micro_batch * cfg.inner.grad_accum
        return self.sample_ids[(t - 1) * per : t * per]


@dataclass(frozen=True)
class LeafRecord:
    preimage: LeafPreimage
    digest: bytes

    @property
    def t(self) -> int:
        return self.preimage.t


@dataclass
class RoundResult:
    leaves: list[LeafRecord]
    leaves_root: str
    final_theta: Params
    final_state: OptState
    final_theta_hash: str
    delta_payload: bytes
    delta_hash: str
    ef_in_hash: str
    ef_out: Params
    ef_out_hash: str

    @property
    def leaf_digests(self) -> list[bytes]:
        return [x.digest for x in self.leaves]


@dataclass(frozen=True)
class ReplayReport:
    result: str
    first_bad_leaf: int | None
    recomputed_leaves_root: str
    recomputed_delta_hash: str
    delta_match: bool | None


def _f32(x: float) -> float:
    return float(np.float32(x))


def stage_states(cfg: TrainConfig, theta: Params, st: OptState) -> list[StageState]:
    out = []
    for s in range(cfg.n_stages):
        names = [n for n in theta if stage_of(n, cfg.model, cfg.n_stages) == s]
        out.append(
            StageState(
                theta=state_hash({n: theta[n] for n in names}),
                m=state_hash({n: st.m[n] for n in names}),
                v=state_hash({n: st.v[n] for n in names if n in st.v}),
            )
        )
    return out


def batch_hash(ids: Sequence[int]) -> str:
    return hashlib.sha256(b"".join(struct.pack("<Q", i) for i in ids)).hexdigest()


def _load_batch(
    cfg: TrainConfig,
    ids: Sequence[int],
    get_sample: SampleFn,
    device: torch.device | str = "cpu",
) -> Tensor:
    """Validated u32 samples -> int64 tokens [B, T+1] on ``device`` (the params' device)."""
    rows = []
    for i in ids:
        s = np.asarray(get_sample(i))
        if s.dtype != np.uint32 or s.shape != (cfg.model.seq_len + 1,):
            raise ValueError(f"sample {i} must be u32[seq_len+1]")
        if int(s.max()) >= cfg.model.vocab:
            raise ValueError(f"sample {i} has token id >= vocab")
        rows.append(s.astype(np.int64))
    return torch.from_numpy(np.stack(rows)).to(device)


def _norm(a: Params, b: Params) -> float:
    total = 0.0
    for n in sorted(a):
        total += float((a[n].double() - b[n].double()).pow(2).sum())
    return _f32(math.sqrt(total))


def _make_leaf(
    cfg: TrainConfig, a: Assignment, t: int, theta: Params, st: OptState, loss: float, norm: float
) -> LeafRecord:
    ids = (
        tuple(i for k in range(t - cfg.inner.J + 1, t + 1) for i in a.batch_ids(cfg, k))
        if t
        else ()
    )
    pre = LeafPreimage(
        run_id=a.run_id,
        w=a.w,
        t=t,
        stages=stage_states(cfg, theta, st),
        batch_ids_sha256=batch_hash(ids),
        rng_ctr=rng_ctr(a.run_id, a.w, t, -1),
        loss_f32=f32hex(loss),
        norm_f32=f32hex(norm),
    )
    return LeafRecord(pre, bytes.fromhex(pre.digest()))


def _train_step(
    cfg: TrainConfig, a: Assignment, theta: Params, st: OptState, t: int, get_sample: SampleFn
) -> float:
    names = sorted(theta)
    ga, mb = cfg.inner.grad_accum, cfg.inner.micro_batch
    ids = a.batch_ids(cfg, t)
    if len(ids) != ga * mb:
        raise ValueError(f"assignment too short for step {t}")
    grads = {n: torch.zeros_like(theta[n]) for n in names}
    loss_sum = 0.0
    for j in range(ga):
        params = {n: theta[n].detach().requires_grad_(True) for n in names}
        tokens = _load_batch(cfg, ids[j * mb : (j + 1) * mb], get_sample, theta[names[0]].device)
        ce, aux = forward(cfg.model, params, tokens)
        total = (ce + cfg.model.aux_loss_coef * aux) / ga
        gs = torch.autograd.grad(total, [params[n] for n in names], allow_unused=True)
        for n, g in zip(names, gs, strict=True):
            if g is not None:
                grads[n].add_(g)
        loss_sum += float(ce.detach())
    lr = lr_at(cfg.inner, a.global_step0 + t - 1, t - 1)
    step(cfg.inner, theta, grads, st, lr)
    return loss_sum / ga


def train_round(
    cfg: TrainConfig,
    theta_start: Params,
    a: Assignment,
    get_sample: SampleFn,
    ef_in: Params | None = None,
    carry: OptState | None = None,
    v0: Params | None = None,
    after_step: StepHook | None = None,
) -> RoundResult:
    """Run H inner steps from the public start; ``after_step`` exists only for fault injection."""
    require_threads(cfg.cpu_threads)
    if sorted(theta_start) != sorted(param_shapes(cfg.model)):
        raise ValueError("theta_start does not match the model parameter set")
    if any(x.dtype != torch.float32 for x in theta_start.values()):
        raise ValueError("theta_start must be fp32 master weights")
    H, J = cfg.inner.H, cfg.inner.J
    if len(a.sample_ids) != H * cfg.inner.micro_batch * cfg.inner.grad_accum:
        raise ValueError("assignment length must equal H * micro_batch * grad_accum")
    theta = {n: x.clone() for n, x in theta_start.items()}
    st = init_state(cfg.inner, theta, carry, v0)
    leaves = [_make_leaf(cfg, a, 0, theta, st, 0.0, 0.0)]
    window_start = {n: x.clone() for n, x in theta.items()}
    loss = 0.0
    for t in range(1, H + 1):
        loss = _train_step(cfg, a, theta, st, t, get_sample)
        if after_step is not None:
            after_step(t, theta)
        if t % J == 0:
            leaves.append(_make_leaf(cfg, a, t, theta, st, loss, _norm(theta, window_start)))
            window_start = {n: x.clone() for n, x in theta.items()}
    if ef_in is None:
        ef_in = {n: torch.zeros_like(x) for n, x in theta.items()}
    delta = {n: theta_start[n] - theta[n] for n in theta}
    payload, ef_out = compress(cfg.compress, delta, ef_in)
    return RoundResult(
        leaves=leaves,
        leaves_root=MerkleTree([x.digest for x in leaves]).root.hex(),
        final_theta=theta,
        final_state=st,
        final_theta_hash=state_hash(theta),
        delta_payload=payload,
        delta_hash=payload_hash(payload),
        ef_in_hash=state_hash(ef_in),
        ef_out=ef_out,
        ef_out_hash=state_hash(ef_out),
    )


def replay(
    cfg: TrainConfig,
    theta_start: Params,
    a: Assignment,
    get_sample: SampleFn,
    committed_leaves: Sequence[bytes],
    committed_delta_hash: str | None = None,
    ef_in: Params | None = None,
    carry: OptState | None = None,
    v0: Params | None = None,
) -> ReplayReport:
    """Recompute every leaf from the public start; bitwise comparison only (no tolerance)."""
    ref = train_round(cfg, theta_start, a, get_sample, ef_in, carry, v0)
    mine = ref.leaf_digests
    first_bad: int | None = None
    for i, d in enumerate(mine):
        if i >= len(committed_leaves) or committed_leaves[i] != d:
            first_bad = i
            break
    if first_bad is None and len(committed_leaves) != len(mine):
        first_bad = len(mine)
    delta_match = None if committed_delta_hash is None else committed_delta_hash == ref.delta_hash
    ok = first_bad is None and delta_match is not False
    return ReplayReport(
        result="MATCH" if ok else "MISMATCH",
        first_bad_leaf=first_bad,
        recomputed_leaves_root=ref.leaves_root,
        recomputed_delta_hash=ref.delta_hash,
        delta_match=delta_match,
    )
