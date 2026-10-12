"""Intra-island multi-rank deterministic layout: data + expert parallel, optional ZeRO-1.

Layout (protocol ``reference_spec.layout``, signed into run_id): n_gpus = dp_size * ep_size
ranks; rank r has ep_rank = r % ep_size, dp_rank = r // ep_size. Every rank trains on its own
micro_batch * grad_accum samples per step; the step's global batch is that times n_gpus, taken from
the assignment in rank order. Dense tensors are replicated on every rank; expert e lives on
ep_rank e // (n_experts / ep_size) and is replicated over the dp_size ranks with that ep_rank.

Gradients: all_gather + fixed rank-order fp32 sum over each tensor's replicas, divided by n_gpus.
all_reduce / reduce / reduce_scatter are never used on verified tensors (``forbid_reductions``
makes them raise). Tokens reach their experts by all_to_all, a pure permutation whose backward is
the reverse all_to_all. ZeRO-1: each piece's optimizer state lives on one replica (round-robin over
the sorted piece list); the owner steps it and all ranks all_gather the updated piece. Leaves and
the delta are ht-leaf-v1 (todo 6, unchanged) over gathered full tensors, so a single-process
emulation of the same layout (``emulate`` + ``ThreadComm``) reproduces every leaf bitwise.
Layout invariance across different GPU counts is out of scope: auditors replay the same layout.
"""

from __future__ import annotations

import math
import threading
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager, nullcontext
from dataclasses import dataclass
from typing import Any, Protocol

import torch
import torch.nn.functional as F
from torch import Tensor

from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import Layout, RunManifest
from hypertrain.protocol.messages_v2 import RunManifestV2
from hypertrain.trainer.compress import compress, payload_hash, state_hash
from hypertrain.trainer.config import ModelConfig, TrainConfig
from hypertrain.trainer.determinism import require_threads
from hypertrain.trainer.loop import (
    Assignment,
    RoundResult,
    SampleFn,
    _load_batch,
    _make_leaf,
    _norm,
)
from hypertrain.trainer.model import _swiglu, forward, param_shapes, permute_rows, route
from hypertrain.trainer.optim import OptState, fixed_sum, init_state, lr_at, step, uses_muon

Params = dict[str, Tensor]
Piece = tuple[str, int]  # (tensor name, ep shard index; 0 for dense tensors)
FORBIDDEN_OPS = (
    "all_reduce",
    "all_reduce_coalesced",
    "reduce",
    "reduce_scatter",
    "reduce_scatter_tensor",
)


class LayoutMismatch(ValueError):
    """The launched process group or requested layout differs from the manifest's."""


class ForbiddenCollective(RuntimeError):
    """A non-fixed-order reduction was attempted on verified tensors."""


@dataclass(frozen=True, slots=True)
class TraceContext:
    rank: int
    step: int
    microbatch: int
    layer: int
    op: str


TraceHook = Callable[[TraceContext, Tensor], Tensor]
CancelHook = Callable[[], None]


class HookComm:
    """Actual collective boundary hooks; identity hooks leave operation order unchanged."""

    def __init__(self, comm: Comm, hook: TraceHook | None, cancel: CancelHook | None) -> None:
        self.comm, self.hook, self.cancel = comm, hook, cancel
        self.rank, self.world = comm.rank, comm.world
        self.step = 0

    def emit(self, op: str, x: Tensor) -> Tensor:
        if self.cancel is not None:
            self.cancel()
        return (
            x if self.hook is None else self.hook(TraceContext(self.rank, self.step, -1, -1, op), x)
        )

    def all_gather(self, t: Tensor) -> list[Tensor]:
        got = self.comm.all_gather(self.emit("all_gather.input", t))
        return [self.emit(f"all_gather.rank.{r}", x) for r, x in enumerate(got)]

    def all_to_all(self, x: Tensor, send: list[int], recv: list[int]) -> Tensor:
        return self.emit(
            "all_to_all.output", self.comm.all_to_all(self.emit("all_to_all.input", x), send, recv)
        )

    def all_reduce_sum(self, t: Tensor) -> Tensor:
        return self.comm.all_reduce_sum(t)


def island_from_manifest(body: Mapping[str, Any]) -> tuple[TrainConfig, Layout, str]:
    """Validate ``body`` as a protocol RunManifest; return (train config, layout, run_id)."""
    if body.get("manifest_version") == 2:
        wrapper = RunManifestV2.model_validate(body)
        return (
            TrainConfig.from_manifest_v2(wrapper),
            wrapper.training.reference_spec.layout,
            wrapper.run_id(),
        )
    m = RunManifest.model_validate(body)
    lay = m.reference_spec.layout
    cfg = TrainConfig.from_manifest(body)
    if cfg.model.is_moe:
        if cfg.model.n_experts % lay.ep_size:
            raise ValueError("ep_size must divide n_experts")
    elif lay.ep_size != 1:
        raise ValueError("dense models require ep_size == 1")
    return cfg, lay, m.run_id()


def check_launch(
    lay: Layout,
    world_size: int,
    dp_size: int | None = None,
    ep_size: int | None = None,
    zero1: bool | None = None,
) -> None:
    """Refuse a process group or launcher layout that differs from the manifest's."""
    if world_size != lay.n_gpus:
        raise LayoutMismatch(f"world size {world_size} != manifest n_gpus {lay.n_gpus}")
    for name, want in (("dp_size", dp_size), ("ep_size", ep_size), ("zero1", zero1)):
        if want is not None and want != getattr(lay, name):
            raise LayoutMismatch(f"launch {name}={want} != manifest {getattr(lay, name)}")


class Comm(Protocol):
    rank: int
    world: int

    def all_gather(self, t: Tensor) -> list[Tensor]: ...

    def all_to_all(self, x: Tensor, send: list[int], recv: list[int]) -> Tensor: ...

    def all_reduce_sum(self, t: Tensor) -> Tensor: ...


class DistComm:
    """torch.distributed world group (gloo on CPU; nccl needs device tensors, see risks)."""

    def __init__(self) -> None:
        import torch.distributed as dist

        self._dist = dist
        self.rank = dist.get_rank()
        self.world = dist.get_world_size()
        self.ops: set[str] = set()

    def all_gather(self, t: Tensor) -> list[Tensor]:
        self.ops.add("all_gather")
        t = t.contiguous()
        out = [torch.empty_like(t) for _ in range(self.world)]
        self._dist.all_gather(out, t)
        return out

    def all_to_all(self, x: Tensor, send: list[int], recv: list[int]) -> Tensor:
        self.ops.add("all_to_all")
        if x.dtype == torch.bfloat16:  # bf16 -> fp32 -> bf16 is exact
            return self.all_to_all(x.float(), send, recv).to(torch.bfloat16)
        out = x.new_empty((sum(recv), *x.shape[1:]))
        self._dist.all_to_all_single(out, x.contiguous(), recv, send)
        return out

    def all_reduce_sum(self, t: Tensor) -> Tensor:
        """Test-only forbidden path; raises under ``forbid_reductions``."""
        self.ops.add("all_reduce")
        out = t.clone()
        self._dist.all_reduce(out)
        return out


class ThreadHub:
    def __init__(self, world: int, timeout: float = 600.0) -> None:
        self.world = world
        self.barrier = threading.Barrier(world, timeout=timeout)  # deadlock guard, not a wait
        self.slots: list[Any] = [None] * world

    def exchange(self, rank: int, item: Any) -> list[Any]:
        self.slots[rank] = item
        self.barrier.wait()
        out = list(self.slots)
        self.barrier.wait()
        return out


class ThreadComm:
    """Single-process emulation of the same collectives (one thread per emulated rank)."""

    def __init__(self, hub: ThreadHub, rank: int) -> None:
        self.hub, self.rank, self.world = hub, rank, hub.world

    def all_gather(self, t: Tensor) -> list[Tensor]:
        return [x.clone() for x in self.hub.exchange(self.rank, t.clone())]

    def all_to_all(self, x: Tensor, send: list[int], recv: list[int]) -> Tensor:
        parts = self.hub.exchange(self.rank, list(torch.split(x, send)))
        mine = [parts[s][self.rank] for s in range(self.world)]
        if [p.shape[0] for p in mine] != recv:
            raise RuntimeError("all_to_all receive splits disagree with senders")
        return torch.cat(mine)

    def all_reduce_sum(self, t: Tensor) -> Tensor:
        raise ForbiddenCollective("all_reduce on verified tensors")


def emulate[T](world: int, fn: Callable[[Comm], T]) -> list[T]:
    """Run ``fn`` once per emulated rank (threads + ThreadComm); return per-rank results."""
    hub = ThreadHub(world)
    results: list[Any] = [None] * world
    errors: list[BaseException] = []

    def run(rank: int) -> None:
        try:
            results[rank] = fn(ThreadComm(hub, rank))
        except BaseException as e:  # noqa: BLE001 - re-raised below after all threads stop
            errors.append(e)
            hub.barrier.abort()

    threads = [threading.Thread(target=run, args=(r,)) for r in range(world)]
    for th in threads:
        th.start()
    for th in threads:
        th.join()
    if errors:
        raise next(
            (e for e in errors if not isinstance(e, threading.BrokenBarrierError)), errors[0]
        )
    return results


@contextmanager
def forbid_reductions() -> Iterator[None]:
    """Make torch.distributed reductions raise ForbiddenCollective inside the block."""
    import torch.distributed as dist

    saved = {n: getattr(dist, n) for n in FORBIDDEN_OPS if hasattr(dist, n)}

    def make(name: str) -> Callable[..., Any]:
        def blocked(*_a: Any, **_k: Any) -> Any:
            raise ForbiddenCollective(f"torch.distributed.{name} on verified tensors")

        return blocked

    for n in saved:
        setattr(dist, n, make(n))
    try:
        yield
    finally:
        for n, f in saved.items():
            setattr(dist, n, f)


@dataclass(frozen=True)
class Geometry:
    model: ModelConfig
    lay: Layout
    rank: int

    @property
    def ep(self) -> int:
        return self.lay.ep_size

    @property
    def ep_rank(self) -> int:
        return self.rank % self.ep

    @property
    def group_base(self) -> int:
        return (self.rank // self.ep) * self.ep

    @property
    def eq(self) -> int:
        return self.model.n_experts // self.ep if self.model.is_moe else 0

    def is_expert(self, name: str) -> bool:
        return (
            self.model.is_moe
            and name.startswith("layers.")
            and name.endswith((".w1", ".w2", ".w3"))
        )

    def pieces(self) -> list[Piece]:
        out: list[Piece] = []
        for n in sorted(param_shapes(self.model)):
            out += [(n, q) for q in range(self.ep)] if self.is_expert(n) else [(n, 0)]
        return out

    def piece_shape(self, p: Piece) -> tuple[int, ...]:
        shape = param_shapes(self.model)[p[0]]
        return (self.eq, *shape[1:]) if self.is_expert(p[0]) else shape

    def my_piece(self, name: str) -> Piece:
        return (name, self.ep_rank if self.is_expert(name) else 0)

    def replicas(self, p: Piece) -> list[int]:
        if self.is_expert(p[0]):
            return [j * self.ep + p[1] for j in range(self.lay.dp_size)]
        return list(range(self.lay.n_gpus))

    def owner(self, p: Piece) -> int:
        reps = self.replicas(p)
        return reps[self.pieces().index(p) % len(reps)] if self.lay.zero1 else reps[0]

    def local(self, name: str, full: Tensor) -> Tensor:
        if not self.is_expert(name):
            return full
        q = self.ep_rank
        return full[q * self.eq : (q + 1) * self.eq]


@dataclass(frozen=True)
class IslandAssignment(Assignment):
    """Assignment whose step batch spans all ranks (rank r takes slice r of each step)."""

    ranks: int = 1

    def batch_ids(self, cfg: TrainConfig, t: int) -> tuple[int, ...]:
        per = cfg.inner.micro_batch * cfg.inner.grad_accum * self.ranks
        return self.sample_ids[(t - 1) * per : t * per]


def gather_pieces(
    comm: Comm,
    geo: Geometry,
    values: Mapping[str, Tensor],
    which: Sequence[Piece],
    src: Callable[[Piece], int],
    device: torch.device,
) -> dict[Piece, Tensor]:
    """Deterministic gather: rank src(p) publishes values[p.name]; one padded all_gather per
    tensor name (bounds the transient buffer to world x one tensor)."""
    out: dict[Piece, Tensor] = {}
    for name in sorted({p[0] for p in which}):
        out |= _gather_group(comm, geo, values, [p for p in which if p[0] == name], src, device)
    return out


def _gather_group(
    comm: Comm,
    geo: Geometry,
    values: Mapping[str, Tensor],
    which: Sequence[Piece],
    src: Callable[[Piece], int],
    device: torch.device,
) -> dict[Piece, Tensor]:
    by_rank = [[p for p in which if src(p) == r] for r in range(comm.world)]
    sizes = [sum(math.prod(geo.piece_shape(p)) for p in ps) for ps in by_rank]
    mine = [values[p[0]].reshape(-1) for p in by_rank[comm.rank]]
    pad = torch.zeros(max(sizes) - sizes[comm.rank], dtype=torch.float32, device=device)
    got = comm.all_gather(torch.cat([*mine, pad]))
    out: dict[Piece, Tensor] = {}
    for r, ps in enumerate(by_rank):
        off = 0
        for p in ps:
            shape = geo.piece_shape(p)
            n = math.prod(shape)
            out[p] = got[r][off : off + n].reshape(shape).clone()
            off += n
    return out


def _assemble(geo: Geometry, pieces: Mapping[Piece, Tensor], names: Sequence[str]) -> Params:
    return {
        n: torch.cat([pieces[(n, q)] for q in range(geo.ep)])
        if geo.is_expert(n)
        else pieces[(n, 0)]
        for n in names
    }


class _AllToAll(torch.autograd.Function):
    @staticmethod
    def forward(ctx: Any, x: Tensor, comm: Comm, send: list[int], recv: list[int]) -> Tensor:
        ctx.comm, ctx.send, ctx.recv = comm, send, recv
        return comm.all_to_all(x, send, recv)

    @staticmethod
    def backward(ctx: Any, grad: Tensor) -> tuple[Tensor, None, None, None]:
        return ctx.comm.all_to_all(grad.contiguous(), ctx.recv, ctx.send), None, None, None


def all_to_all(x: Tensor, comm: Comm, send: list[int], recv: list[int]) -> Tensor:
    out = _AllToAll.apply(x, comm, send, recv)
    assert isinstance(out, Tensor)
    return out


@dataclass(frozen=True)
class EPPlan:
    send: list[int]
    recv: list[int]
    perm: Tensor  # received [src][expert] row order -> [expert][src]
    per_expert: list[int]


def ep_dispatch(comm: Comm, geo: Geometry, rows: Tensor, counts: Tensor) -> tuple[Tensor, EPPlan]:
    """rows sorted by global expert (counts[e] rows each) -> my experts' rows, [expert][src]."""
    ep, eq, base = geo.ep, geo.eq, geo.group_base
    group = range(base, base + ep)
    csend = [eq if g in group else 0 for g in range(comm.world)]
    rc = comm.all_to_all(counts.to(torch.int64), csend, csend).view(ep, eq).tolist()
    send = [0] * comm.world
    recv = [0] * comm.world
    for q in range(ep):
        send[base + q] = int(counts[q * eq : (q + 1) * eq].sum())
        recv[base + q] = sum(rc[q])
    starts: list[list[int]] = []
    off = 0
    for s in range(ep):
        starts.append([])
        for i in range(eq):
            starts[s].append(off)
            off += rc[s][i]
    perm = [starts[s][i] + j for i in range(eq) for s in range(ep) for j in range(rc[s][i])]
    plan = EPPlan(
        send,
        recv,
        torch.tensor(perm, dtype=torch.int64, device=rows.device),
        [sum(c[i] for c in rc) for i in range(eq)],
    )
    if isinstance(comm, HookComm):
        plan = EPPlan(
            plan.send, plan.recv, comm.emit("ep.dispatch.permutation", plan.perm), plan.per_expert
        )
    dispatched = permute_rows(all_to_all(rows, comm, send, recv), plan.perm)
    if isinstance(comm, HookComm):
        dispatched = comm.emit("ep.dispatch.rows", dispatched)
    return dispatched, plan


def ep_combine(comm: Comm, plan: EPPlan, outs: Tensor) -> Tensor:
    """Inverse of ep_dispatch: [expert][src] rows back to the sender, sorted by global expert."""
    back = permute_rows(outs, torch.argsort(plan.perm, stable=True))
    if isinstance(comm, HookComm):
        back = comm.emit("ep.combine.rows", back)
    return all_to_all(back, comm, plan.recv, plan.send)


def _moe_ep(comm: Comm, geo: Geometry) -> Callable[..., tuple[Tensor, Tensor]]:
    """model._moe with experts sharded over the ep group; identical ops when ep_size == 1."""

    def moe(
        cfg: ModelConfig, p: Params, pre: str, x: Tensor, dt: torch.dtype
    ) -> tuple[Tensor, Tensor]:
        b, t, d = x.shape
        n, k, e = b * t, cfg.top_k_experts, cfg.n_experts
        flat = x.reshape(n, d)
        logits = flat.float() @ p[pre + "router"]
        capacity = math.ceil(cfg.capacity_factor * n * k / e)
        experts, keep = route(logits.detach(), k, capacity)
        onehot = F.one_hot(experts.view(k, n).t(), e).to(logits.dtype)
        sel = (onehot * logits[:, None, :]).sum(-1).softmax(-1).t().reshape(-1)
        weight = (sel * keep.to(sel.dtype)).to(dt)
        seg = torch.sort(experts, stable=True).indices
        rows = permute_rows(flat.repeat(k, 1), seg)
        grouped, plan = ep_dispatch(comm, geo, rows, torch.bincount(experts, minlength=e))
        outs = [
            _swiglu(
                chunk, p[pre + "w1"][i].to(dt), p[pre + "w3"][i].to(dt), p[pre + "w2"][i].to(dt)
            )
            for i, chunk in enumerate(torch.split(grouped, plan.per_expert))
        ]
        back = ep_combine(comm, plan, torch.cat(outs, 0))
        y = permute_rows(back, torch.argsort(seg, stable=True)) * weight[:, None]
        combined = y.view(k, n, d).sum(0)
        probs = logits.softmax(-1)
        frac = torch.bincount(experts[keep], minlength=e).float() / max(1, n * k)
        aux = e * (frac * probs.mean(0)).sum()
        return combined.view(b, t, d), aux

    return moe


def _rank_grads(
    cfg: TrainConfig,
    comm: Comm,
    geo: Geometry,
    theta: Params,
    ids: Sequence[int],
    get: SampleFn,
    t: int = 0,
    hook: TraceHook | None = None,
) -> tuple[Params, Tensor]:
    """Mirror of loop._train_step on this rank's samples (no optimizer step)."""
    names = sorted(theta)
    ga, mb = cfg.inner.grad_accum, cfg.inner.micro_batch
    moe = _moe_ep(comm, geo) if cfg.model.is_moe else None
    grads = {n: torch.zeros_like(theta[n]) for n in names}
    loss_sum = torch.zeros((), dtype=torch.float64, device=theta[names[0]].device)
    for j in range(ga):
        params = {n: theta[n].detach().requires_grad_(True) for n in names}
        tokens = _load_batch(cfg, ids[j * mb : (j + 1) * mb], get, theta[names[0]].device)

        def emit(layer: int, op: str, x: Tensor, microbatch: int = j) -> Tensor:
            return x if hook is None else hook(TraceContext(comm.rank, t, microbatch, layer, op), x)

        ce, aux = forward(cfg.model, params, tokens, moe=moe, hook=emit if hook else None)
        total = (ce + cfg.model.aux_loss_coef * aux) / ga
        gs = torch.autograd.grad(total, [params[n] for n in names], allow_unused=True)
        for n, g in zip(names, gs, strict=True):
            if g is not None:
                grads[n].add_(g)
        loss_sum = loss_sum + ce.detach().double()
    return grads, loss_sum / ga


def _reduce(comm: Comm, geo: Geometry, grads: Params, reduction: str) -> Params:
    """all_gather + fixed rank-order fp32 sum over each tensor's replicas, / n_gpus."""
    names = sorted(grads)
    if reduction == "bucketed":
        return _reduce_bucketed(comm, geo, grads)
    if reduction in ("zero2", "zero3"):
        return _reduce_to_owner(comm, geo, grads)
    if reduction != "all_gather":  # test-only flag: proves the guard catches all_reduce
        comm.all_reduce_sum(torch.cat([grads[n].reshape(-1) for n in names]))
    out: Params = {}
    # Per tensor, so the transient gather is world x one tensor (not world x all grads).
    # Replica count is the same on every rank, so all ranks issue the same collectives.
    for n in names:
        reps = geo.replicas(geo.my_piece(n))
        if len(reps) == 1:  # sole replica is this rank: the sum is its own gradient
            out[n] = grads[n].clone().div_(geo.lay.n_gpus)
            continue
        got = comm.all_gather(grads[n])
        acc = got[reps[0]].clone()
        for r in reps[1:]:
            acc.add_(got[r])
        out[n] = acc.div_(geo.lay.n_gpus)
        del got
    return out


BUCKET_BYTES = 25 << 20


def _reduce_bucketed(
    comm: Comm, geo: Geometry, grads: Params, bucket_bytes: int | None = None
) -> Params:
    """Bitwise-identical alternative to the all_gather reference, with 2x instead of world x
    traffic: tensors replicated on every rank are flattened (sorted-name order) into buckets; each
    bucket is reduce-scattered via all_to_all (a permutation, not a reduction op, so
    forbid_reductions holds), each rank sums its chunk over source ranks in rank order (the
    reference order), then the reduced chunks are all_gathered. Expert tensors (replicated on a
    dp subset) keep the reference path."""
    names = sorted(grads)
    world = comm.world
    bucket_bytes = BUCKET_BYTES if bucket_bytes is None else bucket_bytes
    full = [n for n in names if len(geo.replicas(geo.my_piece(n))) == world > 1]
    out = _reduce(comm, geo, {n: grads[n] for n in names if n not in full}, "all_gather")
    buckets: list[list[str]] = [[]]
    size = 0
    for n in full:
        nb = grads[n].numel() * grads[n].element_size()
        if buckets[-1] and size + nb > bucket_bytes:
            buckets.append([])
            size = 0
        buckets[-1].append(n)
        size += nb
    for bucket in (b for b in buckets if b):
        flat = torch.cat([grads[n].reshape(-1) for n in bucket])
        chunk = -(-flat.numel() // world)
        flat = torch.cat([flat, flat.new_zeros(chunk * world - flat.numel())])
        got = comm.all_to_all(flat, [chunk] * world, [chunk] * world).view(world, chunk)
        acc = got[0].clone()
        for r in range(1, world):
            acc.add_(got[r])
        acc.div_(geo.lay.n_gpus)
        red = torch.cat(comm.all_gather(acc))
        off = 0
        for n in bucket:
            k = grads[n].numel()
            out[n] = red[off : off + k].view_as(grads[n]).clone()
            off += k
    return {n: out[n] for n in names}


def _reduce_to_owner(comm: Comm, geo: Geometry, grads: Params) -> Params:
    """ZeRO-2 gradient sharding: one all_to_all sends each tensor's local gradient to the ZeRO-1
    owner of its piece; the owner sums the replicas in rank order and divides by n_gpus (the
    reference order, so bitwise equal to _reduce). Non-owners keep no reduced gradient. Sharding is
    per tensor, not flat: Muon and the per-tensor norm terms need whole tensors."""
    # ponytail: one all_to_all of all grads (2x grad memory transient); bucket it like
    # _reduce_bucketed if the transient shows up in peak memory on the 8x run.
    names, world = sorted(grads), comm.world

    def owner_of(rank: int, n: str) -> int:
        g = Geometry(geo.model, geo.lay, rank)
        return g.owner(g.my_piece(n))

    send = [[n for n in names if owner_of(comm.rank, n) == d] for d in range(world)]
    recv = [[n for n in names if owner_of(s, n) == comm.rank] for s in range(world)]
    flat = torch.cat([grads[n].reshape(-1) for d in range(world) for n in send[d]])
    got = comm.all_to_all(
        flat,
        [sum(grads[n].numel() for n in ns) for ns in send],
        [sum(grads[n].numel() for n in ns) for ns in recv],
    )
    parts: list[Params] = []
    off = 0
    for ns in recv:
        parts.append({})
        for n in ns:
            parts[-1][n] = got[off : off + grads[n].numel()].view_as(grads[n])
            off += grads[n].numel()
    out: Params = {}
    for n in recv[comm.rank]:
        reps = geo.replicas(geo.my_piece(n))
        acc = parts[reps[0]][n].clone()
        for r in reps[1:]:
            acc.add_(parts[r][n])
        out[n] = acc.div_(geo.lay.n_gpus)
    return out


def to_dtensor(theta: Params, mesh: Any) -> Params:
    """FSDP2 layout of full fp32 params: each tensor as a DTensor sharded on dim 0 over ``mesh``
    (torch.distributed.tensor, the storage fully_shard uses). Layout only: training stays on the
    explicit zero3 path above, whose collectives are fixed-order; ``.full_tensor()`` round-trips
    bitwise (all_gather is a copy, not a reduction)."""
    from torch.distributed.tensor import Shard, distribute_tensor

    return {n: distribute_tensor(x, mesh, [Shard(0)]) for n, x in sorted(theta.items())}


def _grad_norm(comm: Comm, geo: Geometry, red: Params, sharded: bool = False) -> Tensor:
    """optim.global_grad_norm over the full (all-shard) gradient, summed in fixed order on device
    (0-dim float64, no host sync). ``sharded``: ``red`` holds only this rank's owned tensors
    (ZeRO-2/3); each term is then taken from the owner."""
    names = sorted(param_shapes(geo.model)) if sharded else sorted(red)
    zero = torch.zeros((), dtype=torch.float64, device=next(iter(red.values())).device)
    parts = torch.stack([red[n].double().pow(2).sum() if n in red else zero for n in names])
    got = comm.all_gather(parts)
    terms: list[Tensor] = []
    for i, n in enumerate(names):
        if sharded:
            qs = range(geo.ep) if geo.is_expert(n) else range(1)
            terms += [got[geo.owner((n, q))][i] for q in qs]
        elif geo.is_expert(n):
            terms += [got[geo.replicas((n, q))[0]][i] for q in range(geo.ep)]
        else:
            terms.append(parts[i])
    return fixed_sum(terms).sqrt()


def _mean_loss(comm: Comm, loss: Tensor | float, device: torch.device) -> Tensor:
    """Rank-order float64 mean as a 0-dim device tensor; callers read it once per leaf."""
    got = comm.all_gather(torch.as_tensor(loss, dtype=torch.float64, device=device).reshape(1))
    return fixed_sum([x[0] for x in got]) / len(got)


def train_island(
    cfg: TrainConfig,
    lay: Layout,
    comm: Comm,
    theta_start: Params,
    a: Assignment,
    get_sample: SampleFn,
    ef_in: Params | None = None,
    carry: OptState | None = None,
    v0: Params | None = None,
    reduction: str = "all_gather",
    hook: TraceHook | None = None,
    cancel: CancelHook | None = None,
    after_step: Callable[[int, Params], None] | None = None,
    checkpoint: Callable[[int, Params, OptState], None] | None = None,
) -> RoundResult:
    """One H-step round on this rank; every rank returns the same full RoundResult.

    ``reduction="bucketed"`` swaps the gradient all_gather for the bucketed reduce-scatter
    (bitwise identical result, see _reduce_bucketed). ``"zero2"`` shards gradients onto the
    ZeRO-1 owners (_reduce_to_owner); ``"zero3"`` additionally keeps only owned parameters between
    steps and all_gathers them before each forward (FSDP-style, fixed order). Both need
    ``lay.zero1`` and are bitwise equal to the reference (tests/layout/test_zero23.py).

    With a torch.distributed comm, reductions are blocked for the whole round.
    """
    guard = forbid_reductions() if isinstance(comm, DistComm) else nullcontext()
    with guard:
        return _train_island(
            cfg,
            lay,
            comm,
            theta_start,
            a,
            get_sample,
            ef_in,
            carry,
            v0,
            reduction,
            hook,
            cancel,
            after_step,
            checkpoint,
        )


def _train_island(
    cfg: TrainConfig,
    lay: Layout,
    comm: Comm,
    theta_start: Params,
    a: Assignment,
    get_sample: SampleFn,
    ef_in: Params | None = None,
    carry: OptState | None = None,
    v0: Params | None = None,
    reduction: str = "all_gather",
    hook: TraceHook | None = None,
    cancel: CancelHook | None = None,
    after_step: Callable[[int, Params], None] | None = None,
    checkpoint: Callable[[int, Params, OptState], None] | None = None,
) -> RoundResult:
    require_threads(cfg.cpu_threads)
    check_launch(lay, comm.world)
    traced = HookComm(comm, hook, cancel)
    comm = traced
    if sorted(theta_start) != sorted(param_shapes(cfg.model)):
        raise ValueError("theta_start does not match the model parameter set")
    if any(x.dtype != torch.float32 for x in theta_start.values()):
        raise ValueError("theta_start must be fp32 master weights")
    if any(tuple(theta_start[n].shape) != s for n, s in param_shapes(cfg.model).items()):
        raise ValueError("theta_start shapes differ from model")
    if carry is not None:
        adam_names = {n for n in theta_start if not uses_muon(cfg.inner, n)}
        if set(carry.m) != set(theta_start) or set(carry.v) != adam_names:
            raise ValueError("carried full state does not match the model")
        if carry.step != a.global_step0:
            raise ValueError("carried optimizer step differs from assignment")
        if any(
            x.dtype != torch.float32 or x.shape != theta_start[n].shape
            for values in (carry.m, carry.v)
            for n, x in values.items()
        ):
            raise ValueError("carried full state shape/dtype mismatch")
    if (
        cfg.model.is_moe
        and cfg.model.n_experts % lay.ep_size
        or not cfg.model.is_moe
        and lay.ep_size != 1
    ):
        raise ValueError("ep_size incompatible with the model")
    H, J, mb, ga = cfg.inner.H, cfg.inner.J, cfg.inner.micro_batch, cfg.inner.grad_accum
    if len(a.sample_ids) != H * mb * ga * lay.n_gpus:
        raise ValueError("assignment length must equal H * micro_batch * grad_accum * n_gpus")
    ia = IslandAssignment(a.run_id, a.w, a.sample_ids, a.global_step0, ranks=lay.n_gpus)
    geo = Geometry(cfg.model, lay, comm.rank)
    names = sorted(theta_start)
    theta = {n: geo.local(n, theta_start[n]).clone() for n in names}
    owned = [n for n in names if geo.owner(geo.my_piece(n)) == comm.rank or not lay.zero1]
    local_carry = None
    if carry is not None:
        local_carry = OptState(
            {n: geo.local(n, carry.m[n]) for n in owned if n in carry.m},
            {n: geo.local(n, carry.v[n]) for n in owned if n in carry.v},
            carry.step,
        )
    local_v0 = None if v0 is None else {n: geo.local(n, v0[n]) for n in owned if n in v0}
    st = init_state(cfg.inner, {n: theta[n] for n in owned}, local_carry, local_v0)
    pieces = geo.pieces()
    adam = [p for p in pieces if not uses_muon(cfg.inner, p[0])]
    device = theta[names[0]].device
    if reduction in ("zero2", "zero3") and not lay.zero1:
        raise ValueError(f"reduction={reduction} needs layout zero1 (it reuses the ZeRO-1 owners)")
    zero3 = reduction == "zero3"
    if zero3:
        theta = {n: theta[n] for n in owned}

    def full() -> tuple[Params, OptState]:
        th = _assemble(geo, gather_pieces(comm, geo, theta, pieces, geo.owner, device), names)
        m = _assemble(geo, gather_pieces(comm, geo, st.m, pieces, geo.owner, device), names)
        adam_names = sorted({p[0] for p in adam})
        v = _assemble(geo, gather_pieces(comm, geo, st.v, adam, geo.owner, device), adam_names)
        return th, OptState(m, v, st.step)

    th_full, st_full = full()
    leaves = [_make_leaf(cfg, ia, 0, th_full, st_full, 0.0, 0.0)]
    if checkpoint is not None:
        checkpoint(0, th_full, st_full)
    window_start = th_full
    del th_full, st_full  # memory: only window_start (theta) is needed between leaves
    per = mb * ga
    for t in range(1, H + 1):
        traced.step = t
        if cancel is not None:
            cancel()
        ids = ia.batch_ids(cfg, t)[comm.rank * per : (comm.rank + 1) * per]
        if zero3:  # ZeRO-3: unshard params just for forward/backward
            synced = gather_pieces(comm, geo, theta, pieces, geo.owner, device)
            theta = {n: synced[geo.my_piece(n)] for n in names}
            del synced
        grads, loss_local = _rank_grads(cfg, comm, geo, theta, ids, get_sample, t, hook)
        if zero3:  # reshard: keep only owned tensors between steps
            theta = {n: theta[n] for n in owned}
        red = _reduce(comm, geo, grads, reduction)
        if hook is not None:
            red = {
                n: hook(TraceContext(comm.rank, t, -1, cfg.model.n_layers + 1, f"gradient.{n}"), x)
                for n, x in red.items()
            }
        del grads
        norm = _grad_norm(comm, geo, red, sharded=reduction in ("zero2", "zero3"))
        lr = lr_at(cfg.inner, a.global_step0 + t - 1, t - 1)
        step(cfg.inner, {n: theta[n] for n in owned}, red, st, lr, norm=norm)
        if after_step is not None:
            after_step(t, theta)
        if hook is not None:
            for n in owned:
                theta[n] = hook(
                    TraceContext(comm.rank, t, -1, cfg.model.n_layers + 1, f"optimizer.theta.{n}"),
                    theta[n],
                )
                st.m[n] = hook(
                    TraceContext(comm.rank, t, -1, cfg.model.n_layers + 1, f"optimizer.m.{n}"),
                    st.m[n],
                )
                if n in st.v:
                    st.v[n] = hook(
                        TraceContext(comm.rank, t, -1, cfg.model.n_layers + 1, f"optimizer.v.{n}"),
                        st.v[n],
                    )
        del red
        if lay.zero1 and lay.n_gpus > 1 and not zero3:
            synced = gather_pieces(comm, geo, theta, pieces, geo.owner, device)
            for n in names:
                theta[n].copy_(synced[geo.my_piece(n)])
            del synced
        loss = _mean_loss(comm, loss_local, device)
        if t % J == 0:
            th_full, st_full = full()
            if checkpoint is not None:
                checkpoint(t, th_full, st_full)
            leaves.append(
                _make_leaf(cfg, ia, t, th_full, st_full, float(loss), _norm(th_full, window_start))
            )
            window_start = th_full
            del th_full, st_full
    del window_start
    th_full, st_full = full()
    if ef_in is None:
        ef_in = {n: torch.zeros_like(x) for n, x in th_full.items()}
    delta = {n: theta_start[n] - th_full[n] for n in th_full}
    payload, ef_out = compress(cfg.compress, delta, ef_in)
    return RoundResult(
        leaves=leaves,
        leaves_root=MerkleTree([x.digest for x in leaves]).root.hex(),
        final_theta=th_full,
        final_state=st_full,
        final_theta_hash=state_hash(th_full),
        delta_payload=payload,
        delta_hash=payload_hash(payload),
        ef_in_hash=state_hash(ef_in),
        ef_out=ef_out,
        ef_out_hash=state_hash(ef_out),
    )
