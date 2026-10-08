"""In-process DiLoCo simulator on the hypertrain trainer core (todo 7, CPU only).

Topology: M replicas in R regions (n_r = M // R). Each replica runs ``train_round`` for H inner
steps from its region's state; a region plain-averages its replicas every H steps (regional sync,
eta_r = 1, no momentum) and the R regions sync globally every K regional syncs (H_g = K * H).
Flat DiLoCo is R = M, K = 1. Global outer optimizers on the region pseudo-gradients
d_r = theta_global - theta_r:
  nesterov   : d_r -> dense-int8 codec (manifest outer.bits=8, ef_beta=0), decoded, summed in
               sorted-hotkey order in fp32, g = sum / R; u = mu*u + g; theta -= lr*(g + mu*u)
  sparseloco : per-region error feedback with the trainer sparseloco codec; theta -= lr*mean
  average    : theta = mean of region states (no delta arithmetic; M=1 -> bitwise pass-through)
Inner-state policies per round: carry (keep m, v, step), reset (zeros), reset + re-warmup,
derived (m = 0, v0 = derive_v0 of the previous global outer gradient; round 0 is reset).

ponytail: robust aggregation (preclip/CClip) is not simulated (honest replicas only); add when
the experiment must measure the clipping cost.
"""

from __future__ import annotations

import hypertrain.trainer  # noqa: F401  (determinism pin before torch)

# isort: split

import hashlib
import math
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

import numpy as np
import numpy.typing as npt
import torch
from torch import Tensor

from hypertrain.data.assignment import assign_round
from hypertrain.protocol.messages import f32val
from hypertrain.trainer.compress import compress, decompress, state_hash
from hypertrain.trainer.config import CompressConfig, TrainConfig
from hypertrain.trainer.loop import Assignment, train_round
from hypertrain.trainer.model import forward, init_params, param_shapes
from hypertrain.trainer.optim import OptState, derive_v0, init_state

Params = dict[str, Tensor]
SampleFn = Callable[[int], npt.NDArray[np.uint32]]


@dataclass(frozen=True)
class Arm:
    name: str
    state_policy: str  # carry | reset | derived
    rewarmup_frac: float = 0.0
    outer_momentum: float | None = None  # override of the manifest momentum (NEG arm only)


ARMS: dict[str, Arm] = {
    "A0": Arm("A0", "carry"),
    "A1": Arm("A1", "reset"),
    "A2": Arm("A2", "reset", rewarmup_frac=0.1),
    "A3": Arm("A3", "derived"),
    # NEG: outer momentum with the wrong sign (mu = -0.9 instead of +0.9), inner state carried
    "NEG": Arm("NEG", "carry", outer_momentum=-0.9),
}


@dataclass(frozen=True)
class SimSpec:
    M: int
    H: int
    steps: int  # inner steps per replica (multiple of K*H)
    outer: str  # nesterov | sparseloco | average
    outer_lr: float
    outer_momentum: float
    arm: Arm
    seed: int
    R: int | None = None  # regions; None -> flat (R = M)
    K: int = 1
    run_id: str = "5e" * 32
    sparse: CompressConfig | None = None  # sparseloco codec (outer == sparseloco)

    @property
    def regions(self) -> int:
        return self.M if self.R is None else self.R

    def __post_init__(self) -> None:
        if self.M < 1 or self.H < 1 or self.K < 1 or self.M % self.regions:
            raise ValueError("need M, H, K >= 1 and R | M")
        if self.steps < 1 or self.steps % (self.K * self.H):
            raise ValueError("steps must be a positive multiple of K*H")
        if self.outer not in ("nesterov", "sparseloco", "average"):
            raise ValueError("unknown outer optimizer")


def hotkeys(seed: int, m: int) -> list[str]:
    """Synthetic replica hotkeys; slot i of the assignment goes to the i-th sorted hotkey."""
    return sorted(hashlib.sha256(f"sim-hk|{seed}|{i}".encode()).hexdigest() for i in range(m))


class ChunkedShards:
    """u32[1025] shard samples cut into seq_len+1 windows (stride seq_len): id -> window."""

    def __init__(self, shard_dir: Path, seq_len: int) -> None:
        import json

        man = json.loads((Path(shard_dir) / "manifest.json").read_text())
        self.src_len = int(man["seq_len"])
        self.sps = int(man["samples_per_shard"])
        if self.src_len % seq_len:
            raise ValueError("model seq_len must divide the shard seq_len")
        self.seq_len = seq_len
        self.per = self.src_len // seq_len
        self.maps = [
            np.memmap(
                Path(shard_dir) / f"shard-{i:05d}.u32",
                dtype="<u4",
                mode="r",
                shape=(self.sps, self.src_len + 1),
            )
            for i in range(int(man["n_shards"]))
        ]
        self.n = len(self.maps) * self.sps * self.per

    def __call__(self, i: int) -> npt.NDArray[np.uint32]:
        if not 0 <= i < self.n:
            raise IndexError(i)
        s, c = divmod(i, self.per)
        row = self.maps[s // self.sps][s % self.sps]
        return np.array(row[c * self.seq_len : c * self.seq_len + self.seq_len + 1], np.uint32)


def round_ids(spec: SimSpec, cfg: TrainConfig, w: int, n_samples: int) -> list[tuple[int, ...]]:
    """Per-replica sample ids of sub-round w (disjoint across replicas and rounds)."""
    per = spec.H * cfg.inner.micro_batch * cfg.inner.grad_accum
    sig = hashlib.sha256(f"sim-beacon|{spec.seed}|{w}".encode()).digest()
    a = assign_round(
        bytes.fromhex(spec.run_id),
        w,
        sig,
        n_samples=n_samples,
        n_slots=spec.M,
        batch=per,
        base_w=w * spec.M * per,
    )
    return [tuple(s) for s in a.slices]


def _mean(states: Sequence[Params]) -> Params:
    if len(states) == 1:
        return {n: x.clone() for n, x in states[0].items()}
    out: Params = {}
    for n in states[0]:
        acc = torch.zeros_like(states[0][n])
        for s in states:
            acc += s[n]
        out[n] = acc / len(states)
    return out


def _zero_state(cfg: TrainConfig, theta: Params) -> OptState:
    """Fresh inner state (Muon tensors carry no v)."""
    return init_state(replace(cfg.inner, state_policy="reset"), theta)


def _finite(p: Params) -> bool:
    return all(bool(torch.isfinite(x).all()) for x in p.values())


@torch.no_grad()
def heldout_loss(cfg: TrainConfig, theta: Params, ids: Sequence[int], get: SampleFn) -> float:
    """Mean CE over fixed held-out windows in batches of 16 (equal sizes -> equal weights)."""
    if not _finite(theta):
        return math.inf
    losses = []
    for k in range(0, len(ids), 16):
        rows = np.stack([get(i).astype(np.int64) for i in ids[k : k + 16]])
        ce, _ = forward(cfg.model, theta, torch.from_numpy(rows))
        losses.append(float(ce))
    return float(np.mean(losses))


def inner_cfg(cfg: TrainConfig, H: int, policy: str, rewarmup: int) -> TrainConfig:
    j = math.gcd(cfg.inner.J, H)
    return replace(
        cfg, inner=replace(cfg.inner, H=H, J=j, state_policy=policy, rewarmup_steps=rewarmup)
    )


def simulate(
    spec: SimSpec,
    cfg: TrainConfig,
    get: SampleFn,
    n_samples: int,
    theta0: Params | None = None,
    diverge_loss: float | None = None,
    deadline: float | None = None,
    lawa: tuple[int, int] | None = None,
) -> dict[str, Any]:
    """Run the topology for ``spec.steps`` inner steps per replica; returns final state + trace.

    ``lawa=(stride, k)``: snapshot the global state at the first global sync past every multiple
    of ``stride`` inner steps and return ``lawa_theta`` = mean of the last ``k`` snapshots.
    ``carry`` in the result holds every replica's inner optimizer state at the end.
    """
    t0 = time.time()
    theta = {n: x.clone() for n, x in (theta0 or init_params(cfg.model)).items()}
    hk = hotkeys(spec.seed, spec.M)
    nr = spec.M // spec.regions
    region_of = [i // nr for i in range(spec.M)]
    arm = spec.arm
    rew = round(arm.rewarmup_frac * spec.H)
    mu = spec.outer_momentum if arm.outer_momentum is None else arm.outer_momentum
    u = {n: torch.zeros_like(x) for n, x in theta.items()}
    ef = [{n: torch.zeros_like(x) for n, x in theta.items()} for _ in range(spec.regions)]
    carry: list[OptState] = [_zero_state(cfg, theta) for _ in range(spec.M)]
    snaps: list[Params] = []
    last_g: Params | None = None
    diverge_loss = diverge_loss if diverge_loss is not None else 2 * math.log(cfg.model.vocab)
    dense = CompressConfig("dense-int8", 1.0, 8, 0.0)
    trace: list[float] = []
    diverged, reason = False, ""
    n_global = spec.steps // (spec.K * spec.H)
    w = 0
    for g in range(n_global):
        if deadline is not None and time.time() > deadline:
            return {"censored": True, "sub_rounds": w, "wall_s": time.time() - t0}
        region_theta: list[Params] = [theta] * spec.regions
        for _k in range(spec.K):
            ids = round_ids(spec, cfg, w, n_samples)
            for r in range(spec.regions):
                members = [i for i in range(spec.M) if region_of[i] == r]
                finals, losses = [], []
                for i in members:
                    pol = arm.state_policy
                    v0 = None
                    if pol == "derived":
                        if last_g is None:
                            pol = "reset"
                        else:
                            v0 = derive_v0(last_g, spec.H)
                    c = inner_cfg(cfg, spec.H, pol, rew)
                    a = Assignment(spec.run_id, w, ids[i], global_step0=w * spec.H)
                    res = train_round(
                        c,
                        region_theta[r],
                        a,
                        get,
                        carry=carry[i] if pol == "carry" else None,
                        v0=v0,
                    )
                    carry[i] = res.final_state
                    finals.append(res.final_theta)
                    losses.append(f32val(res.leaves[-1].preimage.loss_f32))
                region_theta[r] = _mean(finals)
                trace.append(float(np.mean(losses)))
                if not all(math.isfinite(x) and x < diverge_loss for x in losses):
                    diverged, reason = True, f"train loss {losses} at sub-round {w}"
            w += 1
            if diverged:
                break
        if diverged:
            break
        if spec.outer == "average":
            theta = _mean(region_theta)
        else:
            deltas = [{n: theta[n] - rt[n] for n in theta} for rt in region_theta]
            if not all(_finite(d) for d in deltas):
                diverged, reason = True, f"nonfinite pseudo-gradient at global round {g}"
                break
            dec: list[Params] = []
            for r, d in enumerate(deltas):
                if spec.outer == "nesterov":
                    payload, _ = compress(dense, d, {n: torch.zeros_like(x) for n, x in d.items()})
                else:
                    assert spec.sparse is not None, "sparseloco needs a codec config"
                    payload, ef[r] = compress(spec.sparse, d, ef[r])
                dec.append(decompress(payload)[1])
            # region key = smallest member hotkey; sum in sorted key order (fp32)
            order = sorted(range(spec.regions), key=lambda r: hk[r * nr])
            gsum = {n: torch.zeros_like(x) for n, x in theta.items()}
            for r in order:
                for n in gsum:
                    gsum[n] += dec[r][n]
            gm = {n: x / spec.regions for n, x in gsum.items()}
            if spec.outer == "nesterov":
                for n in theta:
                    u[n].mul_(mu).add_(gm[n])
                    theta[n] = theta[n] - spec.outer_lr * (gm[n] + mu * u[n])
            else:
                for n in theta:
                    theta[n] = theta[n] - spec.outer_lr * gm[n]
            last_g = gm
        if diverged or not _finite(theta):
            diverged = True
            reason = reason or f"nonfinite global state at round {g}"
            break
        s_end, kh = (g + 1) * spec.K * spec.H, spec.K * spec.H
        if lawa is not None and s_end // lawa[0] > (s_end - kh) // lawa[0]:
            snaps = [*snaps, {n: x.clone() for n, x in theta.items()}][-lawa[1] :]
    return {
        "theta": theta,
        "lawa_theta": _mean(snaps) if snaps else None,
        "lawa_k": len(snaps),
        "carry": carry,
        "theta_hash": state_hash(theta) if _finite(theta) else "nonfinite",
        "trace": trace,
        "diverged": diverged,
        "reason": reason,
        "sub_rounds": w,
        "wall_s": time.time() - t0,
    }


def dp_ids(spec: SimSpec, cfg: TrainConfig, n_samples: int) -> tuple[int, ...]:
    """Data-parallel control batches: step s uses the union of all replicas' step-s batches."""
    mb = cfg.inner.micro_batch * cfg.inner.grad_accum
    out: list[int] = []
    for w in range(spec.steps // spec.H):
        ids = round_ids(spec, cfg, w, n_samples)
        for t in range(spec.H):
            for i in range(spec.M):
                out.extend(ids[i][t * mb : (t + 1) * mb])
    return tuple(out)


def data_parallel(
    spec: SimSpec, cfg: TrainConfig, get: SampleFn, n_samples: int, theta0: Params | None = None
) -> dict[str, Any]:
    """Synchronous DP at equal tokens: one replica, batch M*micro_batch, same samples and lr."""
    t0 = time.time()
    if cfg.inner.grad_accum != 1:
        raise ValueError("DP control assumes grad_accum == 1")
    c = replace(
        cfg,
        inner=replace(
            cfg.inner,
            H=spec.steps,
            J=spec.steps,
            micro_batch=cfg.inner.micro_batch * spec.M,
            state_policy="reset",
            rewarmup_steps=0,
        ),
    )
    th0 = theta0 or init_params(cfg.model)
    res = train_round(c, th0, Assignment(spec.run_id, 0, dp_ids(spec, cfg, n_samples)), get)
    ok = _finite(res.final_theta)
    return {
        "theta": res.final_theta,
        "theta_hash": res.final_theta_hash if ok else "nonfinite",
        "trace": [f32val(res.leaves[-1].preimage.loss_f32)],
        "diverged": not ok,
        "reason": "" if ok else "nonfinite",
        "wall_s": time.time() - t0,
    }


def fragment_of(name: str, n_layers: int, n_frag: int) -> int:
    """Streaming fragment of a tensor: contiguous layer blocks; emb -> first, norm/head -> last."""
    if name.startswith("layers."):
        return int(name.split(".")[1]) * n_frag // n_layers
    return 0 if name.startswith("emb") else n_frag - 1


def simulate_streaming(
    spec: SimSpec,
    cfg: TrainConfig,
    get: SampleFn,
    n_samples: int,
    fragments: int,
    overlap: int = 0,
    alpha: float = 0.5,
    deadline: float | None = None,
) -> dict[str, Any]:
    """Flat streaming DiLoCo (2501.18512 shape): P layer fragments, each synced every H steps
    at staggered offsets of H/P steps (outer Nesterov, dense-int8, inner state carried).

    overlap = tau chunks of H/P steps: the new global fragment arrives tau chunks after the send
    and is merged as theta_i = alpha*theta_i + (1-alpha)*theta_g (streaming mixing). The
    evaluated model is the global state after a final sync of every fragment.
    ponytail: Kale et al. eager local-proxy outer step (2502.12996) is not reproduced; tau=1 here
    is delayed merge only. P=1, tau=0 reduces bitwise to ``simulate`` flat nesterov carry.
    """
    t0 = time.time()
    P = fragments
    if spec.regions != spec.M or spec.K != 1 or spec.outer != "nesterov":
        raise ValueError("streaming is simulated for flat nesterov only")
    if P < 1 or spec.H % P or not 0 <= overlap < P or spec.arm.state_policy != "carry":
        raise ValueError("need P | H, 0 <= overlap < P and carried inner state")
    c = spec.H // P
    theta_g = {n: x.clone() for n, x in init_params(cfg.model).items()}
    frag = {n: fragment_of(n, cfg.model.n_layers, P) for n in theta_g}
    reps = [{n: x.clone() for n, x in theta_g.items()} for _ in range(spec.M)]
    carry = [_zero_state(cfg, theta_g) for _ in range(spec.M)]
    u = {n: torch.zeros_like(x) for n, x in theta_g.items()}
    hk = hotkeys(spec.seed, spec.M)
    order = sorted(range(spec.M), key=lambda i: hk[i])
    dense = CompressConfig("dense-int8", 1.0, 8, 0.0)
    cspec = replace(spec, H=c)
    ccfg = inner_cfg(cfg, c, "carry", 0)
    mu = spec.outer_momentum
    pending: dict[int, tuple[Params, int]] = {}
    trace: list[float] = []
    diverged, reason = False, ""
    diverge_loss = 2 * math.log(cfg.model.vocab)

    def sync(p: int) -> Params:
        names = [n for n in theta_g if frag[n] == p]
        dec = []
        for i in range(spec.M):
            d = {n: theta_g[n] - reps[i][n] for n in names}
            payload, _ = compress(dense, d, {n: torch.zeros_like(x) for n, x in d.items()})
            dec.append(decompress(payload)[1])
        for n in names:
            acc = torch.zeros_like(theta_g[n])
            for i in order:
                acc += dec[i][n]
            g = acc / spec.M
            u[n].mul_(mu).add_(g)
            theta_g[n] = theta_g[n] - spec.outer_lr * (g + mu * u[n])
        return {n: theta_g[n].clone() for n in names}

    n_chunks = spec.steps // c
    for j in range(n_chunks):
        if deadline is not None and time.time() > deadline:
            return {"censored": True, "sub_rounds": j, "wall_s": time.time() - t0}
        ids = round_ids(cspec, cfg, j, n_samples)
        losses = []
        for i in range(spec.M):
            a = Assignment(spec.run_id, j, ids[i], global_step0=j * c)
            res = train_round(ccfg, reps[i], a, get, carry=carry[i])
            carry[i], reps[i] = res.final_state, res.final_theta
            losses.append(f32val(res.leaves[-1].preimage.loss_f32))
        trace.append(float(np.mean(losses)))
        if not all(math.isfinite(x) and x < diverge_loss for x in losses):
            diverged, reason = True, f"train loss {losses} at chunk {j}"
            break
        for q, (vals, due) in list(pending.items()):
            if due == j:
                for i in range(spec.M):
                    for n, x in vals.items():
                        reps[i][n] = alpha * reps[i][n] + (1 - alpha) * x
                del pending[q]
        p = j % P
        new = sync(p)
        if overlap == 0:
            for i in range(spec.M):
                for n, x in new.items():
                    reps[i][n] = x.clone()
        else:
            pending[p] = (new, j + overlap)
    if not diverged:
        for p in range(P):
            if p != (n_chunks - 1) % P:
                sync(p)
    ok = _finite(theta_g)
    return {
        "theta": theta_g,
        "theta_hash": state_hash(theta_g) if ok else "nonfinite",
        "lawa_theta": None,
        "lawa_k": 0,
        "carry": carry,
        "trace": trace,
        "diverged": diverged or not ok,
        "reason": reason or ("" if ok else "nonfinite global state"),
        "sub_rounds": n_chunks,
        "wall_s": time.time() - t0,
    }


def anneal_single(
    spec: SimSpec,
    cfg: TrainConfig,
    get: SampleFn,
    n_samples: int,
    sim: dict[str, Any],
    total_steps: int,
) -> dict[str, Any]:
    """Final anneal at M_eff = 1: continue ``sim`` (run to spec.steps) as one synchronous replica
    of global batch M*b over the same samples the M replicas would have used, to total_steps.

    ponytail: the inner state is the elementwise mean of the replicas' carried (m, v); a real
    island would keep its own state; add a state-handoff arm if this choice matters.
    """
    t0 = time.time()
    L = total_steps - spec.steps
    st = sim["carry"]
    state = OptState(_mean([s.m for s in st]), _mean([s.v for s in st]), st[0].step)
    B = cfg.inner.micro_batch * spec.M
    c = replace(
        cfg,
        inner=replace(
            cfg.inner, H=L, J=math.gcd(cfg.inner.J, L), micro_batch=B, state_policy="carry",
            rewarmup_steps=0,
        ),
    )  # fmt: skip
    ids = dp_ids(replace(spec, steps=total_steps), cfg, n_samples)[-L * B :]
    a = Assignment(spec.run_id, spec.steps, ids, global_step0=spec.steps)
    res = train_round(c, sim["theta"], a, get, carry=state)
    ok = _finite(res.final_theta)
    return sim | {
        "theta": res.final_theta,
        "theta_hash": res.final_theta_hash if ok else "nonfinite",
        "lawa_theta": None,
        "lawa_k": 0,
        "diverged": bool(sim["diverged"]) or not ok,
        "reason": sim["reason"] or ("" if ok else "nonfinite after anneal"),
        "wall_s": sim["wall_s"] + time.time() - t0,
    }


def n_params(cfg: TrainConfig) -> int:
    return sum(math.prod(s) for s in param_shapes(cfg.model).values())
