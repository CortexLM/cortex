"""Operator aggregator: deterministic outer step, soft screens, event tapes, one-round rollback.

All arithmetic is CPU numpy float32, element-wise, over tensors in UTF-8-sorted name order and
contributions in UTF-8-sorted id order. Norms and dot products use math.fsum over exact float64
products (exactly rounded, so order- and platform-independent). No reduction depends on threads.

Outer step (ultrabrain section 3c):
  g = CenteredClip_{center}(preclip(delta_i)) with weights 1/n
  nesterov:   u' = mu*u + g ; theta' = theta - eta*(g + mu*u') ; center' = g
  sparseloco: theta' = theta - eta*g ; u' = u ; center' = g   (EF is miner-side, committed)
Every merge emits a signed event tape that lists its inputs and weights; replaying the tape from
stored artifacts reproduces the output state bit for bit.
"""

from __future__ import annotations

import fcntl
import itertools
import json
import math
import os
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np
from numpy.typing import NDArray
from safetensors.numpy import load as st_load
from safetensors.numpy import save as st_save

from hypertrain.data.store import CorruptObjectError, ObjectNotFound, Store
from hypertrain.protocol.envelope import seal
from hypertrain.protocol.hashing import sha256_hex, tensor_hash
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import KeyError_, Keypair, decode_hotkey, verify
from hypertrain.protocol.messages import Commit, Rollback, RunManifest, f32hex, f32val

Arr = NDArray[Any]
Params = dict[str, Arr]

TAPE_V = "ht-tape-v1"
TAPE_DOMAIN = b"hypertrain/1|EventTape|"
# Copy-suspicion thresholds: ultrabrain section 3a (honest overlap 0.07-0.12 measured in V4).
COPY_OVERLAP = 0.4
COPY_COSINE = 0.95
# ponytail: top-k fraction for the overlap screen is not a manifest field yet; 1% of coordinates.
COPY_TOPK_FRAC = 0.01
_PARTS = ("theta", "u", "center")


class AggregatorError(RuntimeError):
    pass


class HashMismatch(AggregatorError):
    """A stored object does not hash to the key it was committed under."""


class MissingObject(AggregatorError):
    pass


class MalformedDelta(AggregatorError):
    pass


class ReplayMismatch(AggregatorError):
    pass


class FinalityError(AggregatorError):
    """Model state past d_final is never rewritten; later fraud is a money-only clawback."""


class TapeError(AggregatorError):
    pass


class SequenceError(AggregatorError):
    """Round out of order, prev state not the last applied output, or stale/duplicate rollback."""


class JournalError(AggregatorError):
    pass


def _utf8(s: str) -> bytes:
    return s.encode("utf-8")


def names(p: Mapping[str, Arr]) -> list[str]:
    return sorted(p, key=_utf8)


def th(p: Mapping[str, Arr]) -> str:
    """TH over float32 tensors; same convention as trainer.compress.state_hash."""
    return tensor_hash(
        (n, "f32", tuple(a.shape), np.ascontiguousarray(a, dtype="<f4").tobytes())
        for n, a in p.items()
    )


def sqnorm(p: Mapping[str, Arr]) -> float:
    return math.fsum(
        itertools.chain.from_iterable(
            np.square(a.astype(np.float64)).ravel().tolist() for a in p.values()
        )
    )


def dot(a: Mapping[str, Arr], b: Mapping[str, Arr]) -> float:
    return math.fsum(
        itertools.chain.from_iterable(
            (a[n].astype(np.float64) * b[n].astype(np.float64)).ravel().tolist() for n in names(a)
        )
    )


def _check_params(p: Mapping[str, Arr], like: Mapping[str, Arr], what: str) -> None:
    if names(p) != names(like):
        raise MalformedDelta(f"{what}: tensor names differ from the model")
    for n in p:
        a = p[n]
        if a.dtype != np.float32 or a.shape != like[n].shape:
            raise MalformedDelta(f"{what}: {n} must be float32 with model shape")
        if not bool(np.isfinite(a).all()):
            raise MalformedDelta(f"{what}: {n} has nonfinite values")


@dataclass(frozen=True)
class OuterState:
    theta: Params
    u: Params
    center: Params

    @classmethod
    def init(cls, theta: Mapping[str, Arr]) -> OuterState:
        t = {n: np.array(theta[n], dtype=np.float32) for n in names(theta)}
        return cls(
            t,
            {n: np.zeros_like(a) for n, a in t.items()},
            {n: np.zeros_like(a) for n, a in t.items()},
        )

    def to_bytes(self) -> bytes:
        flat: dict[str, Arr] = {}
        for part in _PARTS:
            for n, a in getattr(self, part).items():
                flat[f"{part}/{n}"] = np.ascontiguousarray(a, dtype="<f4")
        return st_save(flat)

    @classmethod
    def from_bytes(cls, data: bytes) -> OuterState:
        try:
            flat = st_load(data)
        except Exception as exc:
            raise MalformedDelta(f"state object is not safetensors: {type(exc).__name__}") from exc
        parts: dict[str, Params] = {p: {} for p in _PARTS}
        for k, a in flat.items():
            part, _, n = k.partition("/")
            if part not in parts or not n:
                raise MalformedDelta(f"state object has unexpected tensor {k!r}")
            parts[part][n] = a
        theta = parts["theta"]
        if not theta:
            raise MalformedDelta("state object has no theta tensors")
        for part in _PARTS:
            _check_params(parts[part], theta, f"state.{part}")
        return cls(theta, parts["u"], parts["center"])

    def hashes(self) -> dict[str, str]:
        return {
            "theta_hash": th(self.theta),
            "outer_state_hash": th(self.u),
            "center_hash": th(self.center),
        }


@dataclass(frozen=True)
class OuterParams:
    opt: str
    lr: str
    momentum: str
    preclip_norm: str
    cclip_tau: str
    cclip_iters: int

    def __post_init__(self) -> None:
        if self.opt not in ("nesterov", "sparseloco"):
            raise ValueError("outer.opt must be nesterov or sparseloco")
        if self.cclip_iters < 1:
            raise ValueError("cclip_iters must be >= 1")
        vals = [f32val(x) for x in (self.lr, self.momentum, self.preclip_norm, self.cclip_tau)]
        if not all(math.isfinite(v) for v in vals) or vals[0] <= 0 or vals[2] <= 0:
            raise ValueError("outer lr/preclip_norm must be finite and > 0")
        if not 0 <= vals[1] < 1 or vals[3] <= 0:
            raise ValueError("outer momentum must be in [0, 1) and cclip_tau > 0")

    @classmethod
    def from_manifest(cls, m: RunManifest) -> OuterParams:
        o = m.outer
        return cls(o.opt, o.lr, o.momentum, o.preclip_norm, o.cclip_tau, o.cclip_iters)

    @classmethod
    def from_body(cls, b: Mapping[str, Any]) -> OuterParams:
        return cls(
            str(b["opt"]),
            str(b["lr"]),
            str(b["momentum"]),
            str(b["preclip_norm"]),
            str(b["cclip_tau"]),
            int(b["cclip_iters"]),
        )

    def body(self) -> dict[str, Any]:
        return {
            "opt": self.opt,
            "lr": self.lr,
            "momentum": self.momentum,
            "preclip_norm": self.preclip_norm,
            "cclip_tau": self.cclip_tau,
            "cclip_iters": self.cclip_iters,
        }


def preclip(delta: Mapping[str, Arr], max_norm: float) -> tuple[Params, bool]:
    r = math.sqrt(sqnorm(delta))
    if r <= max_norm:
        return {n: delta[n] for n in names(delta)}, False
    s = np.float32(max_norm / r)
    return {n: delta[n] * s for n in names(delta)}, True


def centered_clip(
    deltas: Sequence[Mapping[str, Arr]],
    weights: Sequence[np.float32],
    center: Mapping[str, Arr],
    tau: float,
    iters: int,
) -> Params:
    """v <- v + sum_i w_i * clip_tau(delta_i - v), iterated; deltas in the caller's fixed order."""
    v = {n: np.array(center[n], dtype=np.float32) for n in names(center)}
    for _ in range(iters):
        acc = {n: np.zeros_like(a) for n, a in v.items()}
        for d, w in zip(deltas, weights, strict=True):
            diff = {n: d[n] - v[n] for n in v}
            r = math.sqrt(sqnorm(diff))
            s = np.float32(1.0 if r <= tau else tau / r)
            for n in v:
                acc[n] = acc[n] + w * (diff[n] * s)
        v = {n: v[n] + acc[n] for n in v}
    return v


def _flat(d: Mapping[str, Arr]) -> Arr:
    return np.concatenate([d[n].ravel() for n in names(d)])


def copy_flags(deltas: Sequence[tuple[str, Mapping[str, Arr]]]) -> list[dict[str, str]]:
    """Pairwise copy suspicion (top-k index overlap or cosine). Soft: raises q, never slashes."""
    if len(deltas) < 2:
        return []
    flats = [(i, _flat(d)) for i, d in deltas]
    size = flats[0][1].size
    k = max(1, math.ceil(COPY_TOPK_FRAC * size))
    tops: dict[str, set[int]] = {}
    norms: dict[str, float] = {}
    for i, x in flats:
        mag = np.abs(x.astype(np.float64))
        tops[i] = set(np.lexsort((np.arange(size), -mag))[:k].tolist())
        norms[i] = math.sqrt(math.fsum(np.square(x.astype(np.float64)).tolist()))
    out = []
    for (a, xa), (b, xb) in itertools.combinations(flats, 2):
        overlap = len(tops[a] & tops[b]) / k
        den = norms[a] * norms[b]
        cos = (
            math.fsum((xa.astype(np.float64) * xb.astype(np.float64)).tolist()) / den
            if den > 0
            else 0.0
        )
        if overlap > COPY_OVERLAP or cos > COPY_COSINE:
            out.append({"a": a, "b": b, "overlap": f32hex(overlap), "cosine": f32hex(cos)})
    return out


@dataclass(frozen=True)
class AggResult:
    g: Params
    ids: list[str]
    weight: str
    preclipped: list[str]
    flags: list[dict[str, str]] = field(default_factory=list)

    @property
    def raise_q(self) -> list[str]:
        return sorted({f[s] for f in self.flags for s in ("a", "b")}, key=_utf8)


def aggregate(deltas: Mapping[str, Mapping[str, Arr]], center: Params, p: OuterParams) -> AggResult:
    ids = sorted(deltas, key=_utf8)
    if not ids:
        return AggResult({n: np.zeros_like(a) for n, a in center.items()}, [], f32hex(0.0), [])
    w = np.float32(1.0) / np.float32(len(ids))
    clipped, pre = [], []
    for i in ids:
        c, hit = preclip(deltas[i], f32val(p.preclip_norm))
        clipped.append(c)
        if hit:
            pre.append(i)
    g = centered_clip(clipped, [w] * len(ids), center, f32val(p.cclip_tau), p.cclip_iters)
    flags = copy_flags(list(zip(ids, clipped, strict=True)))
    return AggResult(g, ids, f32hex(float(w)), pre, flags)


def regional_mean(
    deltas: Mapping[str, Mapping[str, Arr]], like: Params, p: OuterParams
) -> AggResult:
    """Regional relay merge: weighted mean of preclipped deltas (robust screen runs globally)."""
    ids = sorted(deltas, key=_utf8)
    acc = {n: np.zeros_like(a) for n, a in like.items()}
    if not ids:
        return AggResult(acc, [], f32hex(0.0), [])
    w = np.float32(1.0) / np.float32(len(ids))
    pre, clipped = [], []
    for i in ids:
        c, hit = preclip(deltas[i], f32val(p.preclip_norm))
        clipped.append(c)
        if hit:
            pre.append(i)
        for n in acc:
            acc[n] = acc[n] + w * c[n]
    return AggResult(
        acc, ids, f32hex(float(w)), pre, copy_flags(list(zip(ids, clipped, strict=True)))
    )


def outer_step(s: OuterState, g: Mapping[str, Arr], p: OuterParams) -> OuterState:
    eta, mu = np.float32(f32val(p.lr)), np.float32(f32val(p.momentum))
    if p.opt == "nesterov":
        u = {n: mu * s.u[n] + g[n] for n in names(s.theta)}
        theta = {n: s.theta[n] - eta * (g[n] + mu * u[n]) for n in names(s.theta)}
    else:
        u = {n: s.u[n].copy() for n in names(s.theta)}
        theta = {n: s.theta[n] - eta * g[n] for n in names(s.theta)}
    return OuterState(theta, u, {n: np.array(g[n], dtype=np.float32) for n in names(s.theta)})


def tape_message(run_id: str, body: Mapping[str, Any]) -> bytes:
    digest = sha256_hex(canonicalize(body, allow_float=False))
    return TAPE_DOMAIN + digest.encode() + b"|" + run_id.encode()


def sign_tape(kp: Keypair, body: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "body": dict(body),
        "signer": kp.ss58,
        "sig": kp.sign(tape_message(body["run_id"], body)).hex(),
    }


def verify_tape(tape: Mapping[str, Any], signer: str | None = None) -> bool:
    try:
        body, who, sig = tape["body"], tape["signer"], bytes.fromhex(tape["sig"])
        if signer is not None and who != signer:
            return False
        return verify(decode_hotkey(who), tape_message(body["run_id"], body), sig)
    except (KeyError, TypeError, ValueError, KeyError_):
        return False


def tape_hash(tape: Mapping[str, Any]) -> str:
    return sha256_hex(canonicalize(tape["body"], allow_float=False))


def tape_bytes(tape: Mapping[str, Any]) -> bytes:
    return canonicalize(dict(tape), allow_float=False)


def tape_key(tape: Mapping[str, Any]) -> str:
    """Store key of a signed tape (sha256 of its canonical bytes)."""
    return sha256_hex(tape_bytes(tape))


def get_object(store: Store, key: str) -> bytes:
    try:
        data = store.get(key)
    except CorruptObjectError as exc:
        raise HashMismatch(str(exc)) from exc
    except ObjectNotFound as exc:
        raise MissingObject(f"object {key} not in store") from exc
    if sha256_hex(data) != key:
        raise HashMismatch(f"object {key}: content hash differs")
    return data


def load_state(store: Store, key: str) -> OuterState:
    return OuterState.from_bytes(get_object(store, key))


def load_delta(store: Store, delta_hash: str, like: Params) -> Params:
    from hypertrain.trainer.compress import decompress

    payload = get_object(store, delta_hash)
    try:
        _, dec = decompress(payload)
    except ValueError as exc:
        raise MalformedDelta(f"delta {delta_hash}: {exc}") from exc
    out = {n: t.numpy().astype(np.float32, copy=True) for n, t in dec.items()}
    _check_params(out, like, f"delta {delta_hash}")
    return out


def load_tape(store: Store, key: str) -> dict[str, Any]:
    tape: dict[str, Any] = json.loads(get_object(store, key))
    return tape


@dataclass(frozen=True)
class Input:
    id: str
    source: str
    object: str

    def body(self, weight: str) -> dict[str, str]:
        return {"id": self.id, "source": self.source, "object": self.object, "weight": weight}


def _resolve(store: Store, inp: Input, prev: OuterState) -> Params:
    if inp.source == "delta":
        return load_delta(store, inp.object, prev.theta)
    if inp.source == "regional":
        reg = load_state(store, inp.object)
        _check_params(reg.theta, prev.theta, f"regional {inp.id}")
        return {n: prev.theta[n] - reg.theta[n] for n in names(prev.theta)}
    raise TapeError(f"unknown input source {inp.source!r}")


def merge(
    store: Store,
    kind: str,
    prev: OuterState,
    inputs: Sequence[Input],
    p: OuterParams,
) -> tuple[OuterState, AggResult]:
    """Pure merge from stored artifacts. Fetches and verifies every input before computing."""
    ids = [i.id for i in inputs]
    if len(set(ids)) != len(ids):
        raise AggregatorError("duplicate contribution id")
    deltas = {i.id: _resolve(store, i, prev) for i in inputs}
    if kind == "regional":
        agg = regional_mean(deltas, prev.theta, p)
        theta = {n: prev.theta[n] - agg.g[n] for n in names(prev.theta)}
        return OuterState(theta, prev.u, prev.center), agg
    if kind != "global":
        raise TapeError(f"unknown merge kind {kind!r}")
    agg = aggregate(deltas, prev.center, p)
    return outer_step(prev, agg.g, p), agg


def tape_inputs(body: Mapping[str, Any]) -> list[Input]:
    return [Input(str(i["id"]), str(i["source"]), str(i["object"])) for i in body["inputs"]]


def replay_tape(store: Store, tape: Mapping[str, Any], signer: str | None = None) -> OuterState:
    """Recompute a merge from its tape + stored artifacts; raise unless every hash matches."""
    if not verify_tape(tape, signer):
        raise TapeError("event tape signature does not verify")
    b = tape["body"]
    if b.get("v") != TAPE_V:
        raise TapeError("unsupported tape version")
    prev = load_state(store, b["prev_state"])
    if th(prev.theta) != b["prev_theta_hash"]:
        raise ReplayMismatch("prev_state theta hash differs from the tape")
    new, agg = merge(store, b["kind"], prev, tape_inputs(b), OuterParams.from_body(b["params"]))
    weights = {i["weight"] for i in b["inputs"]}
    if b["inputs"] and weights != {agg.weight}:
        raise ReplayMismatch("tape weights differ from the recomputed weights")
    got = {"out_state": sha256_hex(new.to_bytes()), **new.hashes()}
    for k, v in got.items():
        if b[k] != v:
            raise ReplayMismatch(f"replayed {k} {v} differs from tape {b[k]}")
    return new


@dataclass
class RollbackResult:
    tapes: tuple[dict[str, Any], dict[str, Any]]
    envelope: dict[str, Any]
    state_key: str


_JKEYS = {"seq", "prev", "event", "w", "tapes", "hash"}
_GENESIS = "0" * 64


class Journal:
    """Append-only hash-chained JSONL: hash = sha256(JCS(entry minus hash)), prev = prior hash."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.entries: list[dict[str, Any]] = []
        path.parent.mkdir(parents=True, exist_ok=True)
        prev = _GENESIS
        if path.exists():
            for n, line in enumerate(path.read_bytes().splitlines()):
                try:
                    e = json.loads(line)
                except ValueError as exc:
                    raise JournalError(f"journal line {n} is not JSON") from exc
                if not isinstance(e, dict) or set(e) != _JKEYS:
                    raise JournalError(f"journal line {n} has unexpected fields")
                body = {k: v for k, v in e.items() if k != "hash"}
                digest = sha256_hex(canonicalize(body, allow_float=False))
                if e["seq"] != n or e["prev"] != prev or e["hash"] != digest:
                    raise JournalError(f"journal hash chain broken at line {n}")
                prev = e["hash"]
                self.entries.append(e)
        self.head = prev
        self._size = path.stat().st_size if path.exists() else 0

    @property
    def anchor(self) -> dict[str, Any]:
        """{seq, hash} of the head entry; sign it into a checkpoint manifest so a later restart
        detects a journal truncated below it (a bare hash chain cannot see a dropped tail)."""
        return {"seq": len(self.entries) - 1, "hash": self.head}

    def check_anchor(self, anchor: Mapping[str, Any]) -> None:
        seq, want = anchor.get("seq"), anchor.get("hash")
        if type(seq) is not int or seq < -1 or not isinstance(want, str):
            raise JournalError("malformed journal anchor")
        got = (
            _GENESIS
            if seq == -1
            else (self.entries[seq]["hash"] if seq < len(self.entries) else None)
        )
        if got != want:
            raise JournalError(
                f"journal does not contain anchored entry {seq} (truncated or forked)"
            )

    def append(self, event: str, w: int, tapes: Sequence[str]) -> None:
        body = {
            "seq": len(self.entries),
            "prev": self.head,
            "event": event,
            "w": w,
            "tapes": list(tapes),
        }
        digest = sha256_hex(canonicalize(body, allow_float=False))
        e = {**body, "hash": digest}
        line = canonicalize(e, allow_float=False) + b"\n"
        with self.path.open("ab") as f:
            # Single writer: exclusive lock, then refuse if anyone appended since we read it.
            fcntl.flock(f.fileno(), fcntl.LOCK_EX)
            if os.fstat(f.fileno()).st_size != self._size:
                raise JournalError("journal changed on disk: another aggregator is writing")
            f.write(line)
            f.flush()
            os.fsync(f.fileno())
        self._size += len(line)
        self.entries.append(e)
        self.head = digest


class Aggregator:
    """Model state, deltas and tapes live in the store by hash; applied/final round state lives in
    a hash-chained journal under `state_dir`, so a restarted aggregator keeps every guard."""

    def __init__(
        self,
        store: Store,
        run_id: str,
        params: OuterParams,
        signer: Keypair,
        state_dir: Path,
        first_round: int = 0,
        anchor: Mapping[str, Any] | None = None,
    ) -> None:
        self.store, self.run_id, self.params, self.signer = store, run_id, params, signer
        self.first_round = first_round
        self.journal = Journal(Path(state_dir) / "journal.jsonl")
        if anchor is not None:
            self.journal.check_anchor(anchor)
        self.applied: dict[int, dict[str, Any]] = {}
        self.final: set[int] = set()
        for e in self.journal.entries:
            w, keys = e["w"], e["tapes"]
            if e["event"] == "apply" and len(keys) == 1:
                self.applied[w] = self._journal_tape(keys[0])
            elif e["event"] == "rollback" and len(keys) == 2:
                self.applied[w] = self._journal_tape(keys[0])
                self.applied[w + 1] = self._journal_tape(keys[1])
            elif e["event"] == "final" and not keys:
                self.final.add(w)
            else:
                raise JournalError(f"journal entry {e['seq']} is malformed")

    def _journal_tape(self, key: str) -> dict[str, Any]:
        tape = load_tape(self.store, key)
        if not verify_tape(tape, self.signer.ss58):
            raise JournalError(f"journal tape {key} is not signed by this coordinator")
        return tape

    @property
    def next_round(self) -> int:
        return max(self.applied) + 1 if self.applied else self.first_round

    def _check_next(self, w: int, prev_key: str) -> None:
        if w != self.next_round:
            raise SequenceError(f"round {w} offered, expected round {self.next_round}")
        last = self.applied.get(w - 1)
        if last is not None and prev_key != last["body"]["out_state"]:
            raise SequenceError(f"round {w} must start from round {w - 1}'s output state")

    def _record_apply(self, w: int, tape: dict[str, Any]) -> None:
        self.journal.append("apply", w, [tape_key(tape)])
        self.applied[w] = tape

    def put_state(self, s: OuterState) -> str:
        return self.store.put(s.to_bytes())

    def _emit(
        self,
        *,
        kind: str,
        w: int,
        prev_key: str,
        prev: OuterState,
        inputs: Sequence[Input],
        excluded: Iterable[str],
        region: str = "",
        k: int = 0,
        extra: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        new, agg = merge(self.store, kind, prev, inputs, self.params)
        by_id = {i.id: i for i in inputs}
        body: dict[str, Any] = {
            "v": TAPE_V,
            "run_id": self.run_id,
            "kind": kind,
            "w": w,
            "region": region,
            "k": k,
            "params": self.params.body(),
            "prev_state": prev_key,
            "prev_theta_hash": th(prev.theta),
            "inputs": [by_id[i].body(agg.weight) for i in agg.ids],
            "excluded": sorted(set(excluded), key=_utf8),
            "screens": {
                "preclipped": agg.preclipped,
                "copy_flags": agg.flags,
                "raise_q": agg.raise_q,
            },
            "out_state": self.put_state(new),
            **new.hashes(),
            **(extra or {}),
        }
        tape = sign_tape(self.signer, body)
        self.store.put(tape_bytes(tape))
        return tape

    def apply_round(
        self, w: int, prev_key: str, commits: Iterable[Commit], exclude: Iterable[str] = ()
    ) -> dict[str, Any]:
        """Optimistic apply: publishes hash(theta^{w+1}) in the returned tape. Raises (and records
        nothing) if any committed delta is missing, corrupt or malformed."""
        self._check_next(w, prev_key)
        excluded = set(exclude)
        inputs = []
        for c in commits:
            if c.w != w:
                raise AggregatorError(f"commit for round {c.w} offered to round {w}")
            if c.hotkey not in excluded:
                inputs.append(Input(c.hotkey, "delta", c.delta_hash))
        prev = load_state(self.store, prev_key)
        tape = self._emit(
            kind="global", w=w, prev_key=prev_key, prev=prev, inputs=inputs, excluded=excluded
        )
        self._record_apply(w, tape)
        return tape

    def regional_merge(
        self,
        w: int,
        region: str,
        k: int,
        prev_key: str,
        commits: Iterable[Commit],
        exclude: Iterable[str] = (),
    ) -> dict[str, Any]:
        """Regional relay sync k (1-based) of round w; committed as hash + store object."""
        if not region or k < 1:
            raise AggregatorError("regional merge needs a region id and k >= 1")
        excluded = set(exclude)
        inputs = []
        for c in commits:
            if c.w != w:
                raise AggregatorError(f"commit for round {c.w} offered to round {w}")
            if c.hotkey not in excluded:
                inputs.append(Input(c.hotkey, "delta", c.delta_hash))
        prev = load_state(self.store, prev_key)
        return self._emit(
            kind="regional",
            w=w,
            prev_key=prev_key,
            prev=prev,
            inputs=inputs,
            excluded=excluded,
            region=region,
            k=k,
        )

    def regional_start(self, global_key: str) -> str:
        """Regional chains start from theta_g with zeroed outer buffers (deterministic key)."""
        return self.put_state(OuterState.init(load_state(self.store, global_key).theta))

    def _global_regions(
        self,
        w: int,
        prev_key: str,
        chains: Mapping[str, Sequence[Mapping[str, Any]]],
        K: int,
        excluded: Iterable[str] = (),
    ) -> dict[str, Any]:
        prev = load_state(self.store, prev_key)
        g_hash = th(prev.theta)
        inputs, lineage = [], {}
        for region in sorted(chains, key=_utf8):
            tapes = chains[region]
            if len(tapes) != K:
                raise TapeError(f"region {region}: expected {K} regional syncs, got {len(tapes)}")
            for j, t in enumerate(tapes):
                b = t["body"]
                ok = verify_tape(t, self.signer.ss58) and b["kind"] == "regional"
                ok = ok and b["region"] == region and b["w"] == w and b["k"] == j + 1
                if not ok:
                    raise TapeError(f"region {region}: regional tape {j + 1} invalid")
                if j == 0 and b["prev_theta_hash"] != g_hash:
                    raise TapeError(f"region {region}: chain does not start at theta_g")
                if j and b["prev_state"] != tapes[j - 1]["body"]["out_state"]:
                    raise TapeError(f"region {region}: regional chain broken at k={j + 1}")
                replay_tape(self.store, t, self.signer.ss58)
            inputs.append(Input(f"region:{region}", "regional", tapes[-1]["body"]["out_state"]))
            lineage[region] = [tape_key(t) for t in tapes]
        return self._emit(
            kind="global",
            w=w,
            prev_key=prev_key,
            prev=prev,
            inputs=inputs,
            excluded=excluded,
            extra={"regional_tapes": lineage},
        )

    def global_from_regions(
        self, w: int, prev_key: str, chains: Mapping[str, Sequence[Mapping[str, Any]]], K: int
    ) -> dict[str, Any]:
        """Global outer step after K regional syncs: delta_r = theta_g - theta_r^K."""
        self._check_next(w, prev_key)
        tape = self._global_regions(w, prev_key, chains, K)
        self._record_apply(w, tape)
        return tape

    def finalize_round(self, w: int) -> None:
        if w not in self.applied:
            raise AggregatorError(f"round {w} was never applied")
        if w not in self.final:
            self.journal.append("final", w, [])
            self.final.add(w)

    def contributors(self, tape: Mapping[str, Any]) -> set[str]:
        """Miner hotkeys whose deltas a global tape includes (through regional tapes if any)."""
        b = tape["body"]
        lineage = b.get("regional_tapes")
        if not lineage:
            return {i["id"] for i in b["inputs"]}
        return {
            i["id"]
            for keys in lineage.values()
            for key in keys
            for i in load_tape(self.store, key)["body"]["inputs"]
        }

    def _recompute(self, b: Mapping[str, Any], prev_key: str, bad: set[str]) -> dict[str, Any]:
        prev = load_state(self.store, prev_key)
        lineage = b.get("regional_tapes")
        if not lineage:
            keep = [i for i in tape_inputs(b) if i.id not in bad]
            return self._emit(
                kind="global",
                w=b["w"],
                prev_key=prev_key,
                prev=prev,
                inputs=keep,
                excluded=bad | set(b["excluded"]),
            )
        start = self.regional_start(prev_key)
        chains: dict[str, list[dict[str, Any]]] = {}
        for region in sorted(lineage, key=_utf8):
            prev_r, tapes = start, []
            for key in lineage[region]:
                old = load_tape(self.store, key)["body"]
                t = self._emit(
                    kind="regional",
                    w=old["w"],
                    prev_key=prev_r,
                    prev=load_state(self.store, prev_r),
                    inputs=[i for i in tape_inputs(old) if i.id not in bad],
                    excluded=bad | set(old["excluded"]),
                    region=region,
                    k=old["k"],
                )
                tapes.append(t)
                prev_r = t["body"]["out_state"]
            chains[region] = tapes
        K = len(lineage[sorted(lineage, key=_utf8)[0]])
        return self._global_regions(b["w"], prev_key, chains, K, bad | set(b["excluded"]))

    def rollback(
        self,
        tape_w: Mapping[str, Any],
        tape_w1: Mapping[str, Any],
        faulted: Iterable[str],
        cause_hashes: Sequence[str],
        exp_drand: int,
    ) -> RollbackResult:
        """At d_final(w): recompute agg_w, step_w, agg_{w+1}, step_{w+1} excluding `faulted`, from
        stored artifacts only; round w+1 is re-aggregated around the new center c'^{w+1}.
        Hierarchical rounds re-run every regional chain without the faulted miners first."""
        bw, bw1 = tape_w["body"], tape_w1["body"]
        for t in (tape_w, tape_w1):
            if not verify_tape(t, self.signer.ss58) or t["body"]["kind"] != "global":
                raise TapeError("rollback needs two signed global tapes")
        w = int(bw["w"])
        if bw1["w"] != w + 1 or bw1["prev_state"] != bw["out_state"]:
            raise TapeError("tapes are not consecutive rounds w, w+1")
        if w in self.final or w + 1 in self.final:
            raise FinalityError(f"round {w} is final; fraud is handled by clawback only")
        cur = [self.applied.get(w), self.applied.get(w + 1)]
        if any(c is None for c in cur) or [tape_key(c) for c in cur if c is not None] != [
            tape_key(tape_w),
            tape_key(tape_w1),
        ]:
            raise SequenceError("tapes are not the currently applied rounds (stale or rolled back)")
        if self.next_round != w + 2:
            raise SequenceError(f"rollback of round {w} must precede round {w + 2}")
        bad = set(faulted)
        if not bad & (self.contributors(tape_w) | self.contributors(tape_w1)):
            raise SequenceError("faulted hotkeys are in neither round; rollback would be a no-op")
        new_w = self._recompute(bw, bw["prev_state"], bad)
        new_w1 = self._recompute(bw1, new_w["body"]["out_state"], bad)
        body = Rollback(
            w=w,
            excluded=sorted(bad, key=_utf8),
            old_theta_hash_w2=bw1["theta_hash"],
            new_theta_hash_w2=new_w1["body"]["theta_hash"],
            new_outer_state_hash=new_w1["body"]["outer_state_hash"],
            recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
            cause_hashes=list(cause_hashes),
        )
        env = seal(self.signer, "Rollback", self.run_id, body, exp_drand)
        self.journal.append("rollback", w, [tape_key(new_w), tape_key(new_w1)])
        self.applied[w], self.applied[w + 1] = new_w, new_w1
        return RollbackResult((new_w, new_w1), env, new_w1["body"]["out_state"])
