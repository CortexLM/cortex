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
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
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
from hypertrain.protocol.messages_v2 import AuditJobV2, CommitV2, RunManifestV2, StartStateV2
from hypertrain.trainer.compress import compress, payload_hash, state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import (
    CancelHook,
    Comm,
    IslandAssignment,
    TraceContext,
    TraceHook,
    train_island,
)
from hypertrain.trainer.loop import (
    Assignment,
    Params,
    RoundResult,
    SampleFn,
    StepHook,
    batch_hash,
    replay,
    stage_states,
    train_round,
)
from hypertrain.trainer.model import init_params
from hypertrain.trainer.optim import OptState, init_state
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
    out = {f"theta/{n}": x.detach().cpu().contiguous() for n, x in theta.items()}
    if st is not None:
        out |= {f"m/{n}": x.detach().cpu().contiguous() for n, x in st.m.items()}
        out |= {f"v/{n}": x.detach().cpu().contiguous() for n, x in st.v.items()}
        out["step"] = torch.tensor(st.step, dtype=torch.int64, device="cpu")
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


def optimizer_hash(st: OptState) -> str:
    return sha256_hex(
        bytes.fromhex(state_hash(st.m) + state_hash(st.v)) + struct.pack("<Q", st.step)
    )


@dataclass(frozen=True, slots=True)
class VerifiedAnchor:
    run_id: str
    hotkey: str
    w: int
    layout_hash: str
    state: OptState
    ef: Params
    proof_hash: str
    anchor_hash: str
    theta: Params
    backend: str


class AnchorCache:
    """Only deterministic genesis and successful independent full replay enter this cache.

    L0 persists/pins entries and supplies its authenticated public theta at the next round.
    A supplied miner state or a Merkle-bound prefix is never a cache insertion API.
    """

    def __init__(self) -> None:
        self.entries: dict[tuple[str, str, int, str], VerifiedAnchor] = {}

    @staticmethod
    def layout_hash(wrapper: RunManifestV2) -> str:
        from hypertrain.protocol.jcs import canonicalize

        return sha256_hex(canonicalize(wrapper.training.reference_spec.layout.model_dump()))

    def genesis(self, wrapper: RunManifestV2, hotkey: str, theta: Params) -> VerifiedAnchor:
        cfg = TrainConfig.from_manifest_v2(wrapper)
        if cfg.model.od is not None and cfg.model.od.warm_start:
            raise AuditInputError("warm start requires independently verified checkpoint lineage")
        if state_hash(theta) != state_hash(init_params(cfg.model)):
            raise AuditInputError("genesis theta differs from deterministic initialization")
        reset = replace(cfg.inner, state_policy="reset")
        st = init_state(reset, theta)
        ef = {n: torch.zeros_like(x) for n, x in theta.items()}
        proof = sha256_hex(bytes.fromhex(wrapper.run_id() + optimizer_hash(st) + state_hash(ef)))
        entry = VerifiedAnchor(
            wrapper.run_id(),
            hotkey,
            -1,
            self.layout_hash(wrapper),
            st,
            ef,
            proof,
            proof,
            {n: x.clone() for n, x in theta.items()},
            "genesis",
        )
        self.entries[(entry.run_id, hotkey, -1, entry.layout_hash)] = entry
        return entry

    def warm_start(
        self,
        wrapper: RunManifestV2,
        hotkey: str,
        theta: Params,
        source: VerifiedAnchor,
    ) -> VerifiedAnchor:
        """New OD run from a cached independently replayed checkpoint; heads only may extend."""
        from hypertrain.models.opendecision import extend_params

        cfg = TrainConfig.from_manifest_v2(wrapper)
        key = (source.run_id, source.hotkey, source.w, source.layout_hash)
        if self.entries.get(key) is not source or source.w < 0:
            raise AuditInputError("warm-start source is not independently replayed")
        if cfg.model.od is None or not cfg.model.od.warm_start:
            raise AuditInputError("manifest does not permit OD warm start")
        if state_hash(theta) != state_hash(extend_params(source.theta, cfg.model)):
            raise AuditInputError("warm-start checkpoint tensor mismatch")
        st = init_state(replace(cfg.inner, state_policy="reset"), theta)
        ef = {n: torch.zeros_like(x) for n, x in theta.items()}
        proof = sha256_hex(bytes.fromhex(wrapper.run_id() + source.proof_hash + state_hash(theta)))
        entry = VerifiedAnchor(
            wrapper.run_id(),
            hotkey,
            -1,
            self.layout_hash(wrapper),
            st,
            ef,
            proof,
            proof,
            {n: x.clone() for n, x in theta.items()},
            source.backend,
        )
        self.entries[(entry.run_id, hotkey, -1, entry.layout_hash)] = entry
        return entry

    def persist(self, directory: Path, entry: VerifiedAnchor) -> Path:
        """Durable cache file, atomically published after MATCH; caller owns dispute pins."""
        import json
        import os
        import tempfile

        from hypertrain.gpu_ops.journal import durable_write

        key = (entry.run_id, entry.hotkey, entry.w, entry.layout_hash)
        if self.entries.get(key) is not entry:
            raise AuditInputError("cannot persist unverified anchor")
        directory.mkdir(parents=True, exist_ok=True)
        target = directory / entry.anchor_hash
        if target.exists():
            return target
        with tempfile.TemporaryDirectory(dir=directory, prefix="anchor-") as tmp:
            path = Path(tmp)
            state = pack_state(entry.theta, entry.state)
            ef = pack_state(entry.ef)
            durable_write(path / "state", state)
            durable_write(path / "ef", ef)
            durable_write(
                path / "metadata",
                json.dumps(
                    {
                        "run_id": entry.run_id,
                        "hotkey": entry.hotkey,
                        "w": entry.w,
                        "layout_hash": entry.layout_hash,
                        "proof_hash": entry.proof_hash,
                        "anchor_hash": entry.anchor_hash,
                        "backend": entry.backend,
                        "state_sha256": sha256_hex(state),
                        "ef_sha256": sha256_hex(ef),
                    },
                    sort_keys=True,
                ).encode(),
            )
            os.rename(path, target)
        return target

    def restore(
        self,
        path: Path,
        wrapper: RunManifestV2,
        expected_anchor: str,
        expected_proof: str,
        expected_state_root: str,
        expected_ef_hash: str,
        *,
        expected_hotkey: str,
        expected_round: int,
        expected_backend: str,
    ) -> VerifiedAnchor:
        """L0 supplies roots AND identity/round/backend from its authorized replay journal.

        Metadata beside the tensors is never authority for execution provenance.
        """
        import json

        key = (wrapper.run_id(), expected_hotkey, expected_round, self.layout_hash(wrapper))
        # An unsuccessful reload must not leave a previously inserted anchor usable.
        self.entries.pop(key, None)
        data = json.loads((path / "metadata").read_bytes())
        state_blob, ef_blob = (path / "state").read_bytes(), (path / "ef").read_bytes()
        if (
            data["run_id"] != wrapper.run_id()
            or data["layout_hash"] != self.layout_hash(wrapper)
            or data["anchor_hash"] != expected_anchor
            or data["proof_hash"] != expected_proof
            or data["hotkey"] != expected_hotkey
            or type(data["w"]) is not int
            or data["w"] != expected_round
            or data["backend"] != expected_backend
            or sha256_hex(state_blob) != data["state_sha256"]
            or sha256_hex(ef_blob) != data["ef_sha256"]
        ):
            raise AuditInputError("cached anchor authentication mismatch")
        theta, st = unpack_state(state_blob)
        ef, _ = unpack_state(ef_blob)
        if st is None:
            raise AuditInputError("cached anchor has no optimizer")
        if tensor_root(theta, st) != expected_state_root or state_hash(ef) != expected_ef_hash:
            raise AuditInputError("cached anchor does not match authenticated replay roots")
        entry = VerifiedAnchor(
            data["run_id"],
            data["hotkey"],
            data["w"],
            data["layout_hash"],
            st,
            ef,
            data["proof_hash"],
            data["anchor_hash"],
            theta,
            data["backend"],
        )
        self.entries[(entry.run_id, entry.hotkey, entry.w, entry.layout_hash)] = entry
        return entry

    def prior(self, wrapper: RunManifestV2, start: StartStateV2) -> VerifiedAnchor:
        key = (wrapper.run_id(), start.hotkey, start.w - 1, self.layout_hash(wrapper))
        try:
            entry = self.entries[key]
        except KeyError as e:
            raise AuditInputError(
                "ANCHOR_BUDGET_EXCEEDED: no independently verified predecessor"
            ) from e
        if (
            start.parent_anchor_hash != entry.anchor_hash
            or start.anchor_verdict_hash != entry.proof_hash
            or start.opt_state_hash != optimizer_hash(entry.state)
            or start.ef_hash != state_hash(entry.ef)
        ):
            raise AuditInputError("carried anchor lineage mismatch")
        return entry


def audit_island(
    job: AuditJobV2,
    comm: Comm,
    get_sample: SampleFn,
    theta_blob: bytes,
    ef_blob: bytes,
    v0_blob: bytes,
    cache: AnchorCache,
    *,
    now_round: int,
    cancel: CancelHook | None = None,
    hook: TraceHook | None = None,
    device: torch.device | str = "cpu",
    beacon_now: Callable[[], int] | None = None,
) -> tuple[Outcome, RoundResult]:
    """Full exact-layout replay from an independently checked carry lineage, never served prefix.

    L0 authenticates job issuer/roles/reservation before calling. Live lease rechecked here.
    If one predecessor is missing L0 first executes it under the same reserved 2H budget;
    ordinary worker never silently performs an unreserved historical rebuild.
    """
    job.validate_embedded(now_round)

    def live() -> None:
        if beacon_now is not None:
            job.validate_embedded(beacon_now())
        if cancel is not None:
            cancel()

    cfg = TrainConfig.from_manifest_v2(job.manifest)
    if job.anchor_age > 1 or not cfg.inner.H <= job.replay_step_budget <= 2 * cfg.inner.H:
        raise AuditInputError("ANCHOR_BUDGET_EXCEEDED")
    start = job.start_state
    if sha256_hex(theta_blob) != start.state_object_sha256:
        raise AuditInputError("start object hash mismatch")
    if sha256_hex(ef_blob) != job.ef_in.sha256 or len(ef_blob) != job.ef_in.size:
        raise AuditInputError("EF object hash/size mismatch")
    if sha256_hex(v0_blob) != job.v0.sha256 or len(v0_blob) != job.v0.size:
        raise AuditInputError("v0 object hash/size mismatch")
    theta, supplied = unpack_state(theta_blob)
    ef, _ = unpack_state(ef_blob)
    v0, _ = unpack_state(v0_blob)
    if (
        state_hash(theta) != start.theta_hash
        or state_hash(ef) != start.ef_hash
        or start.ef_object_sha256 != job.ef_in.sha256
    ):
        raise AuditInputError("start theta/EF binding mismatch")
    entry = cache.prior(job.manifest, start)
    backend = torch.device(device).type
    if entry.backend not in ("genesis", backend):
        raise AuditInputError("CPU anchor cannot serve as CUDA replay oracle")
    if supplied is None or optimizer_hash(supplied) != optimizer_hash(entry.state):
        raise AuditInputError("fabricated carried prefix")
    if cfg.inner.state_policy == "carry" and start.global_step0 != entry.state.step:
        raise AuditInputError("global optimizer step mismatch")
    theta = {n: x.to(device) for n, x in theta.items()}
    ef = {n: x.to(device) for n, x in ef.items()}
    v0 = {n: x.to(device) for n, x in v0.items()}
    carry = OptState(
        {n: x.to(device) for n, x in entry.state.m.items()},
        {n: x.to(device) for n, x in entry.state.v.items()},
        entry.state.step,
    )
    commit = CommitV2.model_validate(job.commit_envelope["body"])
    a = IslandAssignment(
        job.run_id,
        commit.w,
        tuple(job.sample_ids),
        start.global_step0,
        job.manifest.training.reference_spec.layout.n_gpus,
    )
    if any(
        p.batch_ids_sha256 != _expected_batch_hash(cfg, a, i * cfg.inner.J)
        for i, p in enumerate(job.preimages)
    ):
        raise AuditInputError("ASSIGNMENT_VIOLATION")
    result = train_island(
        cfg,
        job.manifest.training.reference_spec.layout,
        comm,
        theta,
        a,
        get_sample,
        ef_in=ef,
        carry=carry if cfg.inner.state_policy == "carry" else None,
        v0=v0 if cfg.inner.state_policy == "derived" else None,
        cancel=live,
        hook=hook,
    )
    live()
    first = next(
        (
            i
            for i, (p, r) in enumerate(zip(job.preimages, result.leaves, strict=True))
            if p.digest() != r.preimage.digest()
        ),
        None,
    )
    match = (
        first is None
        and result.leaves_root == commit.leaves_root
        and result.delta_hash == commit.delta_hash
        and result.final_theta_hash == commit.final_theta_hash
        and result.ef_in_hash == commit.ef_in_hash
        and result.ef_out_hash == commit.ef_out_hash
    )
    out = Outcome("MATCH" if match else "MISMATCH", first, result.leaves_root)
    if match:
        proof = sha256_hex(bytes.fromhex(job.job_id + result.leaves_root + result.delta_hash))
        anchor_hash = sha256_hex(bytes.fromhex(start.digest() + proof))
        layout = cache.layout_hash(job.manifest)
        cache.entries[(job.run_id, commit.hotkey, commit.w, layout)] = VerifiedAnchor(
            job.run_id,
            commit.hotkey,
            commit.w,
            layout,
            result.final_state.clone(),
            {n: x.clone() for n, x in result.ef_out.items()},
            proof,
            anchor_hash,
            {n: x.clone() for n, x in result.final_theta.items()},
            backend,
        )
    return out, result


def audit_island_chain(
    current: AuditJobV2,
    predecessor: AuditJobV2 | None,
    comm: Comm,
    get: SampleFn,
    current_blobs: tuple[bytes, bytes, bytes],
    predecessor_blobs: tuple[bytes, bytes, bytes] | None,
    cache: AnchorCache,
    *,
    now_round: int,
    cancel: CancelHook | None = None,
    device: torch.device | str = "cpu",
    beacon_now: Callable[[], int] | None = None,
) -> tuple[Outcome, RoundResult]:
    """At most one missing predecessor; both accepted jobs consume current reserved 2H.

    L0 supplies authenticated live predecessor job/reservation, never an unsigned prefix.
    """
    current.validate_embedded(beacon_now() if beacon_now is not None else now_round)

    def live() -> None:
        current.validate_embedded(beacon_now() if beacon_now is not None else now_round)
        if cancel is not None:
            cancel()

    h = current.manifest.training.inner.H
    if predecessor is not None:
        if (
            current.anchor_age != 1
            or current.replay_step_budget < 2 * h
            or predecessor.start_state.w + 1 != current.start_state.w
            or predecessor.run_id != current.run_id
            or predecessor.start_state.hotkey != current.start_state.hotkey
            or predecessor_blobs is None
        ):
            raise AuditInputError("ANCHOR_BUDGET_EXCEEDED: predecessor reservation mismatch")
        out, _ = audit_island(
            predecessor,
            comm,
            get,
            *predecessor_blobs,
            cache,
            now_round=now_round,
            cancel=live,
            device=device,
            beacon_now=beacon_now,
        )
        if out.result != "MATCH":
            raise AuditInputError("predecessor replay did not MATCH")
    return audit_island(
        current,
        comm,
        get,
        *current_blobs,
        cache,
        now_round=now_round,
        cancel=live,
        device=device,
        beacon_now=beacon_now,
    )


def replay_island_windows(
    cfg: TrainConfig,
    wrapper: RunManifestV2,
    comm: Comm,
    theta: Params,
    st: OptState,
    a: Assignment,
    start_leaf: int,
    end_leaf: int,
    get_sample: SampleFn,
    *,
    cancel: CancelHook | None = None,
    hook: TraceHook | None = None,
) -> tuple[list[LeafPreimage], Params, OptState]:
    """Same-layout windows for L5 AFTER anchored prefix verification; no independent verdict."""
    if cfg.inner.state_policy != "carry" or cfg.inner.rewarmup_steps:
        raise AuditInputError("island windows require carry without rewarmup")
    if not 0 <= start_leaf < end_leaf <= cfg.inner.H // cfg.inner.J:
        raise AuditInputError("invalid island window")
    per = (
        cfg.inner.micro_batch * cfg.inner.grad_accum * wrapper.training.reference_spec.layout.n_gpus
    )
    wcfg = replace(cfg, inner=replace(cfg.inner, H=cfg.inner.J))
    pres = []
    for i in range(start_leaf, end_leaf):
        s = i * cfg.inner.J
        wa = Assignment(
            a.run_id, a.w, a.sample_ids[s * per : (s + cfg.inner.J) * per], a.global_step0 + s
        )

        def window_hook(ctx: TraceContext, x: torch.Tensor, offset: int = s) -> torch.Tensor:
            assert hook is not None
            return hook(replace(ctx, step=ctx.step + offset), x)

        result = train_island(
            wcfg,
            wrapper.training.reference_spec.layout,
            comm,
            theta,
            wa,
            get_sample,
            carry=st,
            cancel=cancel,
            hook=window_hook if hook is not None else None,
        )
        t = s + cfg.inner.J
        pres.append(
            result.leaves[-1].preimage.model_copy(
                update={"t": t, "rng_ctr": rng_ctr(a.run_id, a.w, t, -1)}
            )
        )
        theta, st = result.final_theta, result.final_state
    return pres, theta, st


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
