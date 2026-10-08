"""Replay audits (ultrabrain sections 1.1, 3a, 3b): bitwise only, never a tolerance.

Full mode (state_policy reset/derived): replay the whole round from the public start through
trainer ``replay()`` and compare every committed leaf.
Segment mode (state_policy carry): replay the challenge's windows [a, b] (leaf indices) from the
miner-served state at leaf a; the final window is always included by ``select_segments`` and
its end state also recomputes the delta hash.

Checks run in this order, the first failure decides the verdict:
  BAD_PROOF            published leaves do not hash to the committed leaves_root, a preimage does
                       not hash to its leaf, or a served state does not match its leaf
  ASSIGNMENT_VIOLATION a committed batch hash differs from the assigned batch ids (no replay)
  WITHHELD             a needed StateServe is missing after serve_deadline
  MISMATCH             a recomputed leaf (or the delta hash) differs from the commitment
  MATCH                otherwise
Miner-reported metrics (loss, norm) are never trusted: they are inside the leaf preimages and
are recomputed by the replay; they only rank the top-Q heuristic segments.
"""

from __future__ import annotations

import hashlib
import struct
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, replace
from typing import Literal

import torch
from safetensors.torch import load as st_load
from safetensors.torch import save as st_save

import hypertrain.trainer  # noqa: F401  (determinism before torch users below)
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.messages import (
    AuditChallenge,
    Commit,
    Forfeit,
    LeafPreimage,
    ReplayEnv,
    ReplayVerdict,
    StateServe,
    f32val,
)
from hypertrain.trainer.compress import compress, payload_hash, state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import (
    Assignment,
    Params,
    SampleFn,
    StepHook,
    batch_hash,
    replay,
    stage_states,
    train_round,
)
from hypertrain.trainer.optim import OptState
from hypertrain.trainer.rng import rng_ctr

Result = Literal["MATCH", "MISMATCH", "WITHHELD", "BAD_PROOF", "ASSIGNMENT_VIOLATION"]
NOT_RECOMPUTED = "0" * 64


class AuditInputError(ValueError):
    """The job itself is inconsistent (coordinator side): no verdict, the job fails."""


class NotReady(Exception):
    """A needed StateServe is missing but serve_deadline has not passed yet."""


@dataclass(frozen=True)
class Outcome:
    result: Result
    first_bad_leaf: int | None
    recomputed_leaves_root: str


@dataclass(frozen=True)
class AuditInputs:
    cfg: TrainConfig
    run_id: str
    challenge: AuditChallenge
    commit: Commit
    assignment: Assignment
    leaves: Sequence[str]
    preimages: Sequence[LeafPreimage]
    theta_start: Params
    ef_in: Params | None = None
    v0: Params | None = None


@dataclass(frozen=True)
class ServedState:
    serve: StateServe
    blob: bytes


def challenge_hash(ch: AuditChallenge) -> str:
    return body_digest(ch.model_dump(mode="json"))


def pack_state(theta: Params, st: OptState | None = None) -> bytes:
    out = {f"theta/{n}": x.contiguous() for n, x in theta.items()}
    if st is not None:
        out |= {f"m/{n}": x.contiguous() for n, x in st.m.items()}
        out |= {f"v/{n}": x.contiguous() for n, x in st.v.items()}
        out["step"] = torch.tensor(st.step, dtype=torch.int64)
    return bytes(st_save(out))


def unpack_state(blob: bytes) -> tuple[Params, OptState | None]:
    raw = st_load(blob)
    part: dict[str, Params] = {"theta": {}, "m": {}, "v": {}}
    for key, x in raw.items():
        if key == "step":
            continue
        kind, _, name = key.partition("/")
        if kind not in part or not name or x.dtype != torch.float32:
            raise ValueError(f"unexpected tensor {key!r} in state blob")
        part[kind][name] = x
    if "step" not in raw:
        return part["theta"], None
    step = raw["step"]
    if step.dtype != torch.int64 or step.ndim != 0:
        raise ValueError("step must be an int64 scalar")
    return part["theta"], OptState(part["m"], part["v"], int(step))


def tensor_root(theta: Params, st: OptState) -> str:
    """StateServe.tensor_root = sha256(TH(theta) | TH(m) | TH(v) | step u64le)."""
    return sha256_hex(
        bytes.fromhex(state_hash(theta) + state_hash(st.m) + state_hash(st.v))
        + struct.pack("<Q", st.step)
    )


def _uniform(seed: bytes, n: int, ctr: list[int]) -> int:
    """Unbiased integer in [0, n) by rejection over sha256(seed | ctr)."""
    limit = (1 << 256) - (1 << 256) % n
    while True:
        x = int.from_bytes(hashlib.sha256(seed + struct.pack("<Q", ctr[0])).digest(), "big")
        ctr[0] += 1
        if x < limit:
            return x % n


def select_segments(
    run_id: str,
    w: int,
    beacon_sig_sha256: str,
    target: str,
    norms: Sequence[float],
    k: int,
    q_top: int,
    final_always: bool = True,
) -> list[tuple[int, int]]:
    """Windows [i, i+1] (leaf indices) to audit; norms[i] = committed norm of leaf i+1.

    k random windows (Fisher-Yates from sha256 of the post-commit beacon) plus the final window
    always plus the q_top largest committed norms (ties -> lower index). ``final_always=False``
    exists only for the ablation that shows the final-always rule is what catches last-step
    cheats.
    """
    u = len(norms)
    seed = b"ht-segments|" + f"{run_id}|{w}|{beacon_sig_sha256}|{target}".encode()
    chosen: set[int] = {u - 1} if final_always else set()
    pool = [i for i in range(u) if i not in chosen]
    ctr = [0]
    for j in range(min(k, len(pool))):
        r = j + _uniform(seed, len(pool) - j, ctr)
        pool[j], pool[r] = pool[r], pool[j]
        chosen.add(pool[j])
    rest = sorted((i for i in range(u) if i not in chosen), key=lambda i: (-norms[i], i))
    chosen.update(rest[:q_top])
    return [(i, i + 1) for i in sorted(chosen)]


def _expected_batch_hash(cfg: TrainConfig, a: Assignment, t: int) -> str:
    if t == 0:
        return batch_hash(())
    ids = [i for s in range(t - cfg.inner.J + 1, t + 1) for i in a.batch_ids(cfg, s)]
    return batch_hash(ids)


def _precheck(x: AuditInputs) -> Outcome | None:
    ch, c, cfg = x.challenge, x.commit, x.cfg
    if ch.target != c.hotkey or ch.w != c.w or x.assignment.w != c.w:
        raise AuditInputError("challenge, commit and assignment disagree on target or round")
    if x.assignment.run_id != x.run_id:
        raise AuditInputError("assignment run_id differs from the manifest run_id")
    n = cfg.inner.n_leaves
    if c.n_leaves != n or len(x.leaves) != n or len(x.preimages) != n:
        return Outcome("BAD_PROOF", None, NOT_RECOMPUTED)
    try:
        digests = [bytes.fromhex(h) for h in x.leaves]
    except ValueError:
        return Outcome("BAD_PROOF", None, NOT_RECOMPUTED)
    if any(len(d) != 32 for d in digests) or MerkleTree(digests).root.hex() != c.leaves_root:
        return Outcome("BAD_PROOF", None, NOT_RECOMPUTED)
    for i, pre in enumerate(x.preimages):
        t = i * cfg.inner.J
        bad = pre.run_id != x.run_id or pre.w != c.w or pre.t != t or pre.digest() != x.leaves[i]
        if bad:
            return Outcome("BAD_PROOF", i, NOT_RECOMPUTED)
    for i, pre in enumerate(x.preimages):
        if pre.batch_ids_sha256 != _expected_batch_hash(cfg, x.assignment, i * cfg.inner.J):
            return Outcome("ASSIGNMENT_VIOLATION", i, NOT_RECOMPUTED)
    return None


def _ef_in(x: AuditInputs) -> Params | None:
    ef = x.ef_in
    if ef is None:
        ef = {n: torch.zeros_like(t) for n, t in x.theta_start.items()}
    return ef if state_hash(ef) == x.commit.ef_in_hash else None


def audit_full(x: AuditInputs, get_sample: SampleFn) -> Outcome:
    if x.cfg.inner.state_policy == "carry":
        raise AuditInputError("full-round replay needs a public start state (reset/derived)")
    if x.challenge.mode != "full":
        raise AuditInputError("audit_full called for a segments challenge")
    pre = _precheck(x)
    if pre is not None:
        return pre
    ef = _ef_in(x)
    if ef is None:
        return Outcome("BAD_PROOF", None, NOT_RECOMPUTED)
    rep = replay(
        x.cfg,
        x.theta_start,
        x.assignment,
        get_sample,
        [bytes.fromhex(h) for h in x.leaves],
        x.commit.delta_hash,
        ef_in=ef,
        v0=x.v0,
    )
    result: Result = "MATCH" if rep.result == "MATCH" else "MISMATCH"
    return Outcome(result, rep.first_bad_leaf, rep.recomputed_leaves_root)


def replay_windows(
    cfg: TrainConfig,
    theta: Params,
    st: OptState,
    a: Assignment,
    start_leaf: int,
    end_leaf: int,
    get_sample: SampleFn,
    after_last_step: StepHook | None = None,
) -> tuple[list[LeafPreimage], Params, OptState]:
    """Train windows start_leaf..end_leaf-1 one at a time from (theta, st) under carry policy.

    Each window is a trainer ``train_round`` with H = J; its last leaf is re-keyed to the round
    step t (rng_ctr), giving exactly the round's ``ht-leaf-v1`` preimage for leaf i+1.
    ``after_last_step`` (fault injection only) runs after the last step of the last window.
    """
    if cfg.inner.state_policy != "carry" or cfg.inner.rewarmup_steps:
        raise AuditInputError("window replay needs state_policy carry without re-warmup")
    j = cfg.inner.J
    wcfg = replace(cfg, inner=replace(cfg.inner, H=j))
    per = cfg.inner.micro_batch * cfg.inner.grad_accum
    out: list[LeafPreimage] = []
    for i in range(start_leaf, end_leaf):
        s0 = i * j
        wa = Assignment(a.run_id, a.w, a.sample_ids[s0 * per : (s0 + j) * per], a.global_step0 + s0)
        hook = after_last_step if i == end_leaf - 1 else None
        res = train_round(wcfg, theta, wa, get_sample, carry=st, after_step=hook)
        last = res.leaves[-1].preimage
        t = s0 + j
        out.append(last.model_copy(update={"t": t, "rng_ctr": rng_ctr(a.run_id, a.w, t, -1)}))
        theta, st = res.final_theta, res.final_state
    return out, theta, st


def audit_segments(
    x: AuditInputs,
    get_sample: SampleFn,
    serves: Mapping[int, ServedState | None],
    now_round: int,
    expected_segments: Sequence[tuple[int, int]] | None = None,
) -> Outcome:
    """``serves[leaf]`` = state served for each window start; ``expected_segments`` = the
    windows the auditor derived itself (select_segments) to check the coordinator's draw."""
    ch, cfg = x.challenge, x.cfg
    if ch.mode != "segments" or cfg.inner.state_policy != "carry":
        raise AuditInputError("segment replay needs a segments challenge under carry policy")
    segs = [tuple(s) for s in ch.segments]
    if expected_segments is not None and segs != [tuple(s) for s in expected_segments]:
        raise AuditInputError("challenge segments differ from the beacon-derived selection")
    u = cfg.inner.n_leaves - 1
    if not segs or any(not 0 <= a < b <= u for a, b in segs):
        raise AuditInputError("segments must be non-empty windows inside [0, H/J]")
    pre = _precheck(x)
    if pre is not None:
        return pre
    chash = challenge_hash(ch)
    starts: dict[int, tuple[Params, OptState]] = {}
    for a, _ in segs:
        got = serves.get(a)
        if got is None:
            if now_round > ch.serve_deadline:
                return Outcome("WITHHELD", a, NOT_RECOMPUTED)
            raise NotReady(f"StateServe for leaf {a} not served yet")
        s = got.serve
        proof = [bytes.fromhex(p) for p in s.merkle_proof_leaf_in_leaves_root]
        ok = (
            s.hotkey == x.commit.hotkey
            and s.challenge_hash == chash
            and s.t == a * cfg.inner.J
            and MerkleTree.verify(
                bytes.fromhex(x.leaves[a]),
                a,
                proof,
                bytes.fromhex(x.commit.leaves_root),
                cfg.inner.n_leaves,
            )
        )
        try:
            theta, st = unpack_state(got.blob)
        except Exception:  # noqa: BLE001 - any undecodable blob is the miner's bad proof
            return Outcome("BAD_PROOF", a, NOT_RECOMPUTED)
        if not ok or st is None or sorted(theta) != sorted(x.theta_start):
            return Outcome("BAD_PROOF", a, NOT_RECOMPUTED)
        if tensor_root(theta, st) != s.tensor_root or stage_states(cfg, theta, st) != list(
            x.preimages[a].stages
        ):
            return Outcome("BAD_PROOF", a, NOT_RECOMPUTED)
        starts[a] = (theta, st)
    rebuilt: list[bytes] = []
    for a, b in segs:
        theta, st = starts[a]
        try:
            pres, theta_end, _ = replay_windows(cfg, theta, st, x.assignment, a, b, get_sample)
        except ValueError:
            return Outcome("BAD_PROOF", a, NOT_RECOMPUTED)
        for k, p in enumerate(pres):
            rebuilt.append(bytes.fromhex(p.digest()))
            if p.digest() != x.leaves[a + 1 + k]:
                return Outcome("MISMATCH", a + 1 + k, MerkleTree(rebuilt).root.hex())
        if b == u:
            ef = _ef_in(x)
            if ef is None:
                return Outcome("BAD_PROOF", None, NOT_RECOMPUTED)
            delta = {n: x.theta_start[n] - theta_end[n] for n in theta_end}
            payload, _ = compress(cfg.compress, delta, ef)
            if payload_hash(payload) != x.commit.delta_hash:
                return Outcome("MISMATCH", None, MerkleTree(rebuilt).root.hex())
    return Outcome("MATCH", None, MerkleTree(rebuilt).root.hex())


def committed_norms(preimages: Sequence[LeafPreimage]) -> list[float]:
    return [f32val(p.norm_f32) for p in preimages[1:]]


def make_verdict(ch: AuditChallenge, out: Outcome, env: ReplayEnv) -> ReplayVerdict:
    return ReplayVerdict(
        challenge_hash=challenge_hash(ch),
        first_bad_leaf=out.first_bad_leaf,
        result=out.result,
        recomputed_leaves_root=out.recomputed_leaves_root,
        replay_env=env,
    )


Classification = Literal["TRANSIENT", "FAULT", "CONTEST"]


def classify_mismatch(
    verdict: ReplayVerdict,
    committed_leaves_root: str,
    miner_rerun_root: str,
    transients_this_epoch: int,
    forgive_per_epoch: int,
) -> Classification:
    """Miner re-runs the round on its own host after a MISMATCH (ultrabrain section 4).

    rerun == auditor bits  -> the commitment was a hardware glitch: TRANSIENT while the epoch's
                              forgiveness lasts, FAULT after;
    rerun == own commit    -> the miner stands by it: CONTEST (N-ary bisection);
    anything else          -> the miner's stack is not deterministic: FAULT.
    """
    if verdict.result != "MISMATCH":
        raise ValueError("only a MISMATCH verdict can be classified")
    if miner_rerun_root == verdict.recomputed_leaves_root:
        return "TRANSIENT" if transients_this_epoch < forgive_per_epoch else "FAULT"
    if miner_rerun_root == committed_leaves_root:
        return "CONTEST"
    return "FAULT"


_CAUSE = {
    "MISMATCH": "MISMATCH",
    "BAD_PROOF": "MISMATCH",
    "WITHHELD": "WITHHELD",
    "ASSIGNMENT_VIOLATION": "ASSIGNMENT_VIOLATION",
}


def forfeit_for(
    w: int,
    hotkey: str,
    cause: str,
    evidence: Sequence[str],
    round_reward_units: int,
    unvested_escrow_units: int,
) -> Forfeit:
    """TRANSIENT burns the round reward only; every fault cause burns reward + escrow and
    blacklists. ``cause`` is a verdict result, ``TRANSIENT`` or ``DISPUTE_LOST``."""
    if cause == "TRANSIENT":
        return Forfeit(
            w=w,
            hotkey=hotkey,
            cause="TRANSIENT",
            evidence=list(evidence),
            round_reward_burned=round_reward_units,
            escrow_burned_units=0,
            blacklist=False,
            debt_units=0,
        )
    mapped = "DISPUTE_LOST" if cause == "DISPUTE_LOST" else _CAUSE.get(cause)
    if mapped is None:
        raise ValueError(f"{cause!r} is not a forfeit cause")
    return Forfeit.model_validate(
        {
            "w": w,
            "hotkey": hotkey,
            "cause": mapped,
            "evidence": list(evidence),
            "round_reward_burned": round_reward_units,
            "escrow_burned_units": unvested_escrow_units,
            "blacklist": True,
            "debt_units": 0,
        }
    )
