"""N-ary dispute bisection step -> layer -> op and referee resolution (Verde-style).

Every point of a level carries one sha256 per party:
  step  level, point t      : tensor_root(theta_t, opt_state_t) after inner step t (0 = start)
  layer level, inside step t: 0 = input tokens, i+1 = output of layer i, L+1 = state after
                              the step (the "tail" pseudo-layer L: final norm, head, loss, update)
  op    level, inside layer l: 0 = layer input, then one point per op output (layer 0 starts
                              with ``embed``; OPS / TAIL_OPS)
A forward op point hashes that op's output over all micro-batches of the step.

Each bisection round both parties publish N+1 hashes at p_j = a + ceil(j*(b-a)/N); the referee
keeps the first piece whose end hashes differ, so one level takes ceil(log_N(b-a)) rounds.
At the op the referee takes the agreed state before the step from a party, checks it against
the agreed step hash, re-executes the step itself up to the op, checks its own hash of the
op input equals the agreed one, executes the op, and names the party whose output hash differs.
"""

from __future__ import annotations

import hashlib
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from typing import Literal, Protocol

import torch
import torch.nn.functional as F
from torch import Tensor

import hypertrain.trainer  # noqa: F401  (determinism before torch users below)
from hypertrain.auditor.replay import pack_state, tensor_root, unpack_state
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages import Bisect, Resolution
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, Params, SampleFn, _load_batch
from hypertrain.trainer.model import (
    _arch_impl,
    _attn,
    _moe,
    _rms,
    _swiglu,
    compute_dtype,
    embed,
)
from hypertrain.trainer.optim import OptState, init_state, lr_at, step

Level = Literal["step", "layer", "op"]
OPS = ("attn_norm", "attn", "attn_residual", "mlp_norm", "mlp", "mlp_residual")
TAIL_OPS = ("final_norm", "head", "loss", "update")


class RefereeError(RuntimeError):
    """The referee cannot reproduce a point both parties agree on: no loser is named."""


def ceil_log(n: int, base: int) -> int:
    r, span = 0, 1
    while span < n:
        span *= base
        r += 1
    return r


def points(a: int, b: int, n: int) -> list[int]:
    n = min(n, b - a)
    return [a + -(-j * (b - a) // n) for j in range(n + 1)]


def tensor_digest(x: Tensor) -> bytes:
    y = x.detach().contiguous()
    h = hashlib.sha256(f"{y.dtype}|{tuple(y.shape)}|".encode())
    h.update(y.reshape(-1).view(torch.uint8).numpy().tobytes())
    return h.digest()


@dataclass(frozen=True)
class Fault:
    """Adds ``delta`` to element 0 of one op output at one step (micro-batch 0 for forward ops;
    the first parameter for ``update``). Fault injection for tests and adversarial honeypots."""

    step: int
    layer: int
    op: str
    delta: float = 1e-3


@dataclass
class StepTrace:
    digests: dict[tuple[int, str], list[bytes]] = field(default_factory=dict)

    def add(self, layer: int, op: str, x: Tensor) -> None:
        self.digests.setdefault((layer, op), []).append(tensor_digest(x))

    def point(self, layer: int, op: str) -> str:
        return sha256_hex(b"".join(self.digests[(layer, op)]))


Hook = Callable[[int, str, Tensor], Tensor]


def traced_forward(
    cfg: TrainConfig, p: Params, tokens: Tensor, hook: Hook
) -> tuple[Tensor, Tensor]:
    """Op-for-op copy of trainer ``model.forward``; ``hook(layer, op, out)`` sees every output.

    Layer ``-1`` op ``input`` is the token batch; layer L ops are TAIL_OPS[:3].
    """
    m = cfg.model
    if m.arch != "decoder":
        return _arch_impl(m).traced_forward(cfg, p, tokens, hook)
    dt = compute_dtype(m)
    hook(-1, "input", tokens)
    inp, tgt = tokens[:, :-1], tokens[:, 1:]
    x = hook(0, "embed", embed(p["emb.weight"], inp).to(dt))
    aux = torch.zeros((), dtype=torch.float32)
    for i in range(m.n_layers):
        pre = f"layers.{i:03d}."
        y = hook(i, "attn_norm", _rms(x, p[pre + "attn_norm"]))
        a = hook(i, "attn", _attn(m, p, pre, y, dt))
        x = hook(i, "attn_residual", x + a)
        h = hook(i, "mlp_norm", _rms(x, p[pre + "mlp_norm"]))
        if m.is_moe:
            o, extra = _moe(m, p, pre, h, dt)
            o = hook(i, "mlp", o)
            aux = aux + extra
        else:
            o = hook(
                i,
                "mlp",
                _swiglu(h, p[pre + "w1"].to(dt), p[pre + "w3"].to(dt), p[pre + "w2"].to(dt)),
            )
        x = hook(i, "mlp_residual", x + o)
    n = m.n_layers
    z = hook(n, "final_norm", _rms(x, p["norm.weight"]))
    logits = hook(n, "head", (z @ p["head.weight"].to(dt)).float())
    loss = hook(n, "loss", F.cross_entropy(logits.reshape(-1, m.vocab), tgt.reshape(-1)))
    return loss, aux


def traced_step(
    cfg: TrainConfig,
    a: Assignment,
    theta: Params,
    st: OptState,
    t: int,
    get_sample: SampleFn,
    fault: Fault | None = None,
) -> StepTrace:
    """Inner step t in place on (theta, st), mirroring trainer ``_train_step`` + ``step``."""
    trace = StepTrace()
    names = sorted(theta)
    ga, mb = cfg.inner.grad_accum, cfg.inner.micro_batch
    ids = a.batch_ids(cfg, t)
    grads = {n: torch.zeros_like(theta[n]) for n in names}
    for j in range(ga):

        def hook(layer: int, op: str, out: Tensor, j: int = j) -> Tensor:
            hit = fault is not None and (fault.step, fault.layer, fault.op) == (t, layer, op)
            if hit and j == 0 and out.is_floating_point():
                assert fault is not None
                out = out + torch.where(
                    torch.arange(out.numel()).reshape(out.shape) == 0,
                    torch.tensor(fault.delta, dtype=out.dtype),
                    torch.tensor(0.0, dtype=out.dtype),
                )
            trace.add(layer, op, out)
            return out

        params = {n: theta[n].detach().requires_grad_(True) for n in names}
        tokens = _load_batch(cfg, ids[j * mb : (j + 1) * mb], get_sample)
        ce, aux = traced_forward(cfg, params, tokens, hook)
        total = (ce + cfg.model.aux_loss_coef * aux) / ga
        gs = torch.autograd.grad(total, [params[n] for n in names], allow_unused=True)
        for n, g in zip(names, gs, strict=True):
            if g is not None:
                grads[n].add_(g)
    step(cfg.inner, theta, grads, st, lr_at(cfg.inner, a.global_step0 + t - 1, t - 1))
    n_layers = cfg.model.n_layers
    if fault is not None and (fault.step, fault.layer, fault.op) == (t, n_layers, "update"):
        with torch.no_grad():
            theta[names[0]].view(-1)[0] += fault.delta
    trace.digests[(n_layers, "update")] = [bytes.fromhex(tensor_root(theta, st))]
    return trace


class Party(Protocol):
    hotkey: str

    def hashes(self, level: Level, ctx: tuple[int, ...], at: Sequence[int]) -> list[str]: ...

    def state_blob(self, t: int) -> bytes: ...


class Executor:
    """A party that re-executes the round step by step (optionally with one injected fault)."""

    def __init__(
        self,
        hotkey: str,
        cfg: TrainConfig,
        theta_start: Params,
        a: Assignment,
        get_sample: SampleFn,
        fault: Fault | None = None,
        carry: OptState | None = None,
        v0: Params | None = None,
    ) -> None:
        self.hotkey, self.cfg, self.a, self.get, self.fault = hotkey, cfg, a, get_sample, fault
        theta = {n: x.clone() for n, x in theta_start.items()}
        st = init_state(cfg.inner, theta, carry, v0)
        self._states: list[tuple[Params, OptState]] = [(theta, st)]
        self._traces: dict[int, StepTrace] = {}

    def state(self, t: int) -> tuple[Params, OptState]:
        while len(self._states) <= t:
            k = len(self._states)
            theta, st = self._states[-1]
            theta, st = {n: x.clone() for n, x in theta.items()}, st.clone()
            self._traces[k] = traced_step(self.cfg, self.a, theta, st, k, self.get, self.fault)
            self._states.append((theta, st))
        return self._states[t]

    def trace(self, t: int) -> StepTrace:
        self.state(t)
        return self._traces[t]

    def state_blob(self, t: int) -> bytes:
        theta, st = self.state(t)
        return pack_state(theta, st)

    def hashes(self, level: Level, ctx: tuple[int, ...], at: Sequence[int]) -> list[str]:
        if level == "step":
            return [tensor_root(*self.state(t)) for t in at]
        names = point_names(self.cfg, level, ctx)
        tr = self.trace(ctx[0])
        return [tr.point(*names[i]) for i in at]


def point_names(cfg: TrainConfig, level: Level, ctx: tuple[int, ...]) -> list[tuple[int, str]]:
    n = cfg.model.n_layers
    if level == "layer":
        return [(-1, "input"), *((i, "mlp_residual") for i in range(n)), (n, "update")]
    layer = ctx[1]
    start = (-1, "input") if layer == 0 else (layer - 1, "mlp_residual")
    ops = TAIL_OPS if layer == n else (("embed", *OPS) if layer == 0 else OPS)
    return [start, *((layer, o) for o in ops)]


def level_span(cfg: TrainConfig, level: Level, ctx: tuple[int, ...]) -> int:
    return len(point_names(cfg, level, ctx)) - 1


@dataclass
class DisputeResult:
    step: int
    layer: int
    op: str
    rounds: dict[str, int]
    transcript: list[Bisect]
    resolution: Resolution


def _bisect_level(
    dispute_id: str,
    level: Level,
    ctx: tuple[int, ...],
    a: int,
    b: int,
    n: int,
    parties: tuple[Party, Party],
    transcript: list[Bisect],
) -> tuple[int, int, int, str]:
    """Narrow [a, b] to one unit; returns (a, b, rounds, agreed hash at a)."""
    rounds = 0
    agreed: str | None = None
    while b - a > 1:
        at = points(a, b, n)
        hs = [p.hashes(level, ctx, at) for p in parties]
        for p, h in zip(parties, hs, strict=True):
            transcript.append(
                Bisect(
                    dispute_id=dispute_id,
                    level=level,
                    interval=(a, b),
                    N=len(at) - 1,
                    hashes=h,
                    party=p.hotkey,
                )
            )
        rounds += 1
        if hs[0][0] != hs[1][0]:
            raise RefereeError(f"parties disagree at the agreed start of {level} {ctx}")
        j = next((j for j in range(1, len(at)) if hs[0][j] != hs[1][j]), None)
        if j is None:
            raise RefereeError(f"parties agree on the whole {level} interval {ctx}")
        a, b, agreed = at[j - 1], at[j], hs[0][j - 1]
    if agreed is None:
        ends = [p.hashes(level, ctx, [a, b]) for p in parties]
        if ends[0][0] != ends[1][0] or ends[0][1] == ends[1][1]:
            raise RefereeError(f"{level} {ctx} is not a single disputed unit")
        agreed = ends[0][0]
    return a, b, rounds, agreed


def run_dispute(
    dispute_id: str,
    cfg: TrainConfig,
    challenger: Party,
    defender: Party,
    referee: Executor,
    n: int,
    interval: tuple[int, int],
) -> DisputeResult:
    """Step -> layer -> op bisection on ``interval`` (steps) and referee resolution."""
    if n < 2:
        raise ValueError("N must be >= 2")
    parties = (challenger, defender)
    tr: list[Bisect] = []
    rounds: dict[str, int] = {}
    s0, s1, rounds["step"], step_agreed = _bisect_level(
        dispute_id, "step", (), interval[0], interval[1], n, parties, tr
    )
    t = s1
    l0, _, rounds["layer"], _ = _bisect_level(
        dispute_id, "layer", (t,), 0, level_span(cfg, "layer", (t,)), n, parties, tr
    )
    ctx = (t, l0)
    o0, o1, rounds["op"], op_agreed = _bisect_level(
        dispute_id, "op", ctx, 0, level_span(cfg, "op", ctx), n, parties, tr
    )
    layer, op = point_names(cfg, "op", ctx)[o1]
    claims = {p.hotkey: p.hashes("op", ctx, [o1])[0] for p in parties}

    blob = defender.state_blob(s0)
    theta, st = unpack_state(blob)
    if st is None or tensor_root(theta, st) != step_agreed:
        blob = challenger.state_blob(s0)
        theta, st = unpack_state(blob)
        if st is None or tensor_root(theta, st) != step_agreed:
            raise RefereeError("no party served the agreed state before the disputed step")
    trace = traced_step(cfg, referee.a, theta, st, t, referee.get)
    start_name = point_names(cfg, "op", ctx)[o0]
    if trace.point(*start_name) != op_agreed:
        raise RefereeError("referee cannot reproduce the agreed op input")
    truth = trace.point(layer, op)
    wrong = [k for k, h in claims.items() if h != truth]
    if len(wrong) != 1:
        raise RefereeError(f"referee output matches {2 - len(wrong)} parties")
    res = Resolution(
        dispute_id=dispute_id,
        op_spec=f"step={t};layer={layer};op={op}",
        inputs_hash=sha256_hex(bytes.fromhex(step_agreed + op_agreed)),
        output_hash=truth,
        loser=wrong[0],
    )
    return DisputeResult(t, layer, op, rounds, tr, res)
