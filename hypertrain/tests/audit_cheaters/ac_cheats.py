"""Cheating-miner doubles for the audit-cheaters suite.

Each double is a named, documented behaviour that replaces one step of an otherwise honest
``NetworkMiner.run_round``. A double is applied by patching
``hypertrain.miner.island_launch.launch_island`` with ``cheat.launch(real, ctx)`` and calling
``cheat.wire(miner, ctx)``; nothing in ``src/`` is touched. Which seam a double uses decides what
it can reach:

* ``worker``    the arithmetic of one step, inside the torchrun worker (ac_cheat_worker.py); the
                artifacts it publishes are self-consistent and pass the miner's validate_artifacts.
* ``artifacts`` the published tree is rewritten after the real launch, before run_round reads it
                (run_round copies its four hash commitments from summary.json and never
                re-validates, core.py:1250-1270).
* ``job``       the staged job is changed (other samples, other start state) before the real
                launch; the artifacts are again self-consistent, for the wrong job.
* ``wire``      a signed body is rewritten and re-signed on its way to the master.
* ``stage``     the miner stops performing a stage of the round (raises Withdrawn).
* ``time``      the master clock moves past a window before a stage.

``expected`` names where the suite expects the first refusal: ``audit``, ``intake:/<route>``,
``stall`` (the master refuses nothing but the round cannot settle), ``none-inert`` (accepted and
harmless: nothing the network consumes changes) or ``none-undetected`` (accepted and nothing
catches it: a hole). It is an expectation to be confirmed by a test, not a result.
"""

from __future__ import annotations

import hypertrain.trainer  # noqa: F401

# isort: split
import hashlib
import json
import os
import shutil
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Protocol
from unittest import mock

import numpy as np
import torch

from hypertrain.auditor.replay import pack_state, unpack_state
from hypertrain.miner import island_launch
from hypertrain.miner.core import Api
from hypertrain.miner.island_launch import IslandArtifacts, confined
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import LeafPreimage
from hypertrain.protocol.messages_v2 import IslandJobV1, RankWorkResult
from hypertrain.trainer.compress import compress
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

HERE = Path(__file__).resolve().parent
SEAMS = frozenset({"worker", "artifacts", "job", "wire", "stage", "time", "none"})
EXPECTED = frozenset(
    {
        "none",
        "audit",
        "intake:/commit",
        "intake:/leaves",
        "intake:/upload-grant",
        "intake:/delta",
        "stall",
        "none-inert",
        "none-undetected",
    }
)
SUMMARY_FIELDS = ("final_theta_hash", "ef_out_hash", "leaves_root", "delta_hash")

LaunchFn = Callable[..., IslandArtifacts]


class CheatContext(Protocol):
    """What the harness (or an audit-level test) offers a double."""

    w: int
    hotkey: str
    rows: Any  # numpy array of the whole dataset
    tree: Any  # MerkleTree over the dataset rows

    def peer_artifacts(self) -> IslandArtifacts:
        """Published artifacts of an honest peer of the same round (already UPLOADED)."""
        ...

    def own_previous_artifacts(self) -> IslandArtifacts | None:
        """This miner's published directory of round w-1, if any."""
        ...

    def advance_beacon(self, number: int) -> None:
        """Move the master clock to beacon ``number``."""
        ...


class Withdrawn(Exception):
    """The miner stops performing a stage of the round; ``stage`` is the route it never calls."""

    def __init__(self, stage: str) -> None:
        super().__init__(f"miner withdrew before {stage}")
        self.stage = stage


@dataclass(frozen=True)
class Cheat:
    """The honest control (cheat 0) and the base of every double."""

    cheat: int
    name: str
    summary: str
    seam: str
    expected: str
    needs_previous_round: bool = False

    def launch(self, real: LaunchFn, ctx: CheatContext) -> LaunchFn:
        return real

    def wire(self, miner: Any, ctx: CheatContext) -> None:
        return None


def lie(value: str) -> str:
    """A different, well-formed 64-hex value derived from ``value``."""
    return hashlib.sha256(bytes.fromhex(value)).hexdigest()


def artifacts_of(published: Path) -> IslandArtifacts:
    ranks = sorted(published.glob("rank-*"), key=lambda p: int(p.name.removeprefix("rank-")))
    work = tuple(
        RankWorkResult.model_validate(json.loads((r / "summary.json").read_bytes())["work"])
        for r in ranks
    )
    d = published / "rank-0"
    return IslandArtifacts(
        published,
        d / "state.safetensors",
        d / "ef.safetensors",
        d / "delta.bin",
        d / "leaves.json",
        work,
    )


def _summary(published: Path) -> dict[str, Any]:
    return json.loads((published / "rank-0/summary.json").read_bytes())


def _write_summary(published: Path, summary: dict[str, Any]) -> None:
    (published / "rank-0/summary.json").write_bytes(canonicalize(summary))


def _leaves(published: Path) -> list[dict[str, Any]]:
    return json.loads((published / "rank-0/leaves.json").read_bytes())


def _write_leaves(published: Path, leaves: list[dict[str, Any]]) -> None:
    """Rewrite rank-0/leaves.json and keep summary.json's leaf commitments self-consistent."""
    (published / "rank-0/leaves.json").write_bytes(canonicalize(leaves))
    digests = [LeafPreimage.model_validate(x).digest() for x in leaves]
    summary = _summary(published)
    summary["commitments"]["leaves"] = digests
    summary["commitments"]["leaves_root"] = MerkleTree(
        [bytes.fromhex(d) for d in digests]
    ).root.hex()
    _write_summary(published, summary)


# --- cheat 0: honest control -----------------------------------------------------------------

HONEST = Cheat(0, "honest", "an honest miner: changes nothing", "none", "none")


# --- worker seam: cheats 1 and 5 -------------------------------------------------------------


@contextmanager
def _worker_shim(spec: dict[str, Any]) -> Iterator[None]:
    """Run the next launch with ac_cheat_worker in place of the production torchrun worker."""
    original = island_launch.launch_argv

    def launch_argv(*args: Any, **kwargs: Any) -> list[str]:
        argv = original(*args, **kwargs)
        argv[argv.index("hypertrain.miner.island_worker")] = "ac_cheat_worker"
        return argv

    path = os.pathsep.join(p for p in (str(HERE), os.environ.get("PYTHONPATH")) if p)
    with (
        mock.patch.object(island_launch, "launch_argv", launch_argv),
        mock.patch.dict(os.environ, {"HT_AC_CHEAT": json.dumps(spec), "PYTHONPATH": path}),
    ):
        yield


@dataclass(frozen=True, kw_only=True)
class WorkerCheat(Cheat):
    """``kind`` is ``ulp`` (one ulp on one element after ``step``) or ``skip`` (the optimizer
    update of ``step`` is not applied). ``step=None`` is the last inner step."""

    kind: str
    step: int | None

    def launch(self, real: LaunchFn, ctx: CheatContext) -> LaunchFn:
        def cheating(job: IslandJobV1, directory: Path, **kwargs: Any) -> IslandArtifacts:
            with _worker_shim({"kind": self.kind, "step": self.step}):
                return real(job, directory, **kwargs)

        return cheating


# --- artifacts seam: cheats 1b, 2, 3, 4, 5b, 6b, 7 ---------------------------------------------


@dataclass(frozen=True, kw_only=True)
class ArtifactCheat(Cheat):
    """Template: produce a published tree (real launch by default), then edit it once.

    A second real launch on an edited tree raises (validate_artifacts), so the wrapper is
    idempotent through a marker beside ``published``.
    """

    def produce(
        self, real: LaunchFn, job: IslandJobV1, directory: Path, ctx: CheatContext, **kwargs: Any
    ) -> IslandArtifacts:
        return real(job, directory, **kwargs)

    def tamper(self, directory: Path, published: Path, job: IslandJobV1, ctx: CheatContext) -> None:
        return None

    def launch(self, real: LaunchFn, ctx: CheatContext) -> LaunchFn:
        def cheating(job: IslandJobV1, directory: Path, **kwargs: Any) -> IslandArtifacts:
            marker = directory / "cheat-applied"
            if not marker.exists():
                published = self.produce(real, job, directory, ctx, **kwargs).directory
                self.tamper(directory, published, job, ctx)
                marker.write_text(self.name)
            return artifacts_of(directory / "published")

        return cheating


@dataclass(frozen=True, kw_only=True)
class StateFileOnly(ArtifactCheat):
    """state.safetensors is rewritten with one element one ulp up; summary.json, leaves and delta
    stay honest. WorkProof.artifact_refs[0] hashes the file but nothing ever reads it back."""

    def tamper(self, directory: Path, published: Path, job: IslandJobV1, ctx: CheatContext) -> None:
        path = published / "rank-0/state.safetensors"
        theta, state = unpack_state(path.read_bytes())
        first = theta[sorted(theta)[0]].view(-1)
        first[0] = torch.nextafter(first[0], torch.tensor(float("inf")))
        path.write_bytes(pack_state(theta, state))


@dataclass(frozen=True, kw_only=True)
class ForgedDelta(ArtifactCheat):
    """delta.bin is replaced by a valid codec payload of a delta scaled by ``scale`` (same
    shapes, plausible magnitude); summary.json's delta_hash is the sha256 of the forged bytes,
    so commit, WorkProof, grant, relay upload and delta manifest are all consistent."""

    scale: float

    def tamper(self, directory: Path, published: Path, job: IslandJobV1, ctx: CheatContext) -> None:
        cfg = TrainConfig.from_manifest_v2(job.manifest)
        start, _ = unpack_state(confined(directory, job.object_paths["start_state"]).read_bytes())
        ef_in, _ = unpack_state(confined(directory, job.object_paths["ef_in"]).read_bytes())
        final, _ = unpack_state((published / "rank-0/state.safetensors").read_bytes())
        forged = {k: (start[k] - final[k]) * self.scale for k in final}
        payload, _ = compress(cfg.compress, forged, ef_in)
        (published / "rank-0/delta.bin").write_bytes(payload)
        summary = _summary(published)
        summary["commitments"]["delta_hash"] = hashlib.sha256(payload).hexdigest()
        _write_summary(published, summary)


@dataclass(frozen=True, kw_only=True)
class CopiedPeer(ArtifactCheat):
    """The peer's published tree is used as the miner's own, byte for byte. Generous to the
    attacker: a real copier sees only public objects, not the delta bytes."""

    def produce(
        self, real: LaunchFn, job: IslandJobV1, directory: Path, ctx: CheatContext, **kwargs: Any
    ) -> IslandArtifacts:
        shutil.copytree(ctx.peer_artifacts().directory, directory / "published")
        return artifacts_of(directory / "published")


@dataclass(frozen=True, kw_only=True)
class ReplayedRound(ArtifactCheat):
    """The miner's own tree of round w-1 is published again for round w. ``relabel`` rewrites
    every leaf to round w and recomputes the leaves root (the delta stays the old one); without
    it the tree is verbatim and its leaves still say round w-1."""

    relabel: bool

    def produce(
        self, real: LaunchFn, job: IslandJobV1, directory: Path, ctx: CheatContext, **kwargs: Any
    ) -> IslandArtifacts:
        previous = ctx.own_previous_artifacts()
        if previous is None:
            raise ValueError(f"{self.name} needs this miner's published round {ctx.w - 1}")
        shutil.copytree(previous.directory, directory / "published")
        return artifacts_of(directory / "published")

    def tamper(self, directory: Path, published: Path, job: IslandJobV1, ctx: CheatContext) -> None:
        if self.relabel:
            _write_leaves(published, [{**x, "w": ctx.w} for x in _leaves(published)])


@dataclass(frozen=True, kw_only=True)
class TruncatedLeaves(ArtifactCheat):
    """The last leaf is dropped: the miner claims one window fewer than the manifest's H."""

    def tamper(self, directory: Path, published: Path, job: IslandJobV1, ctx: CheatContext) -> None:
        _write_leaves(published, _leaves(published)[:-1])


@dataclass(frozen=True, kw_only=True)
class ForgedRngCtr(ArtifactCheat):
    """The rng_ctr of leaf ``leaf`` is wrong; the leaves root and summary are recomputed."""

    leaf: int

    def tamper(self, directory: Path, published: Path, job: IslandJobV1, ctx: CheatContext) -> None:
        leaves = _leaves(published)
        leaves[self.leaf]["rng_ctr"] += 1
        _write_leaves(published, leaves)


@dataclass(frozen=True, kw_only=True)
class SummaryLie(ArtifactCheat):
    """One of the four hashes run_round copies from summary.json is wrong; every artifact is
    honest. Wire-level fields (delta_bytes, metrics_root) are CommitLie."""

    field: str

    def tamper(self, directory: Path, published: Path, job: IslandJobV1, ctx: CheatContext) -> None:
        summary = _summary(published)
        summary["commitments"][self.field] = lie(summary["commitments"][self.field])
        _write_summary(published, summary)


# --- job seam: cheat 6 -------------------------------------------------------------------------


@dataclass(frozen=True, kw_only=True)
class JobCheat(Cheat):
    """The staged job is changed, then launched for real: the artifacts are self-consistent for
    the changed job. ``rewrite`` must be idempotent (a restart launches again)."""

    def rewrite(self, job: IslandJobV1, directory: Path, ctx: CheatContext) -> IslandJobV1:
        raise NotImplementedError

    def launch(self, real: LaunchFn, ctx: CheatContext) -> LaunchFn:
        def cheating(job: IslandJobV1, directory: Path, **kwargs: Any) -> IslandArtifacts:
            return real(self.rewrite(job, directory, ctx), directory, **kwargs)

        return cheating


@dataclass(frozen=True, kw_only=True)
class WrongShard(JobCheat):
    """Trains on sample ids shifted by ``shift`` (mod the dataset size), with the matching rows
    and Merkle proofs, so the worker's membership check passes."""

    shift: int

    def rewrite(self, job: IslandJobV1, directory: Path, ctx: CheatContext) -> IslandJobV1:
        dataset = job.manifest.training.dataset
        ids = [(i + self.shift) % dataset.n_samples for i in job.sample_ids]
        dtype = "<u2" if dataset.sample_format.startswith("u16") else "<u4"
        confined(directory, job.object_paths["samples"]).write_bytes(
            b"".join(np.asarray(ctx.rows[i]).astype(dtype).tobytes() for i in ids)
        )
        confined(directory, job.object_paths["sample_proofs"]).write_bytes(
            json.dumps([[p.hex() for p in ctx.tree.proof(i)] for i in ids]).encode()
        )
        return job.model_copy(update={"sample_ids": ids})


@dataclass(frozen=True, kw_only=True)
class WrongInitialTheta(JobCheat):
    """Starts from the weights of another init seed (the start-state pin of the job is adjusted
    to match, as a cheating miner controls its own job copy)."""

    def rewrite(self, job: IslandJobV1, directory: Path, ctx: CheatContext) -> IslandJobV1:
        path = confined(directory, job.object_paths["start_state"])
        raw = path.read_bytes()
        if hashlib.sha256(raw).hexdigest() == job.start_state_sha256:
            model = TrainConfig.from_manifest_v2(job.manifest).model
            wrong = init_params(replace(model, init_seed=model.init_seed + 1))
            path.write_bytes(pack_state(wrong, unpack_state(raw)[1]))
            raw = path.read_bytes()
        return job.model_copy(update={"start_state_sha256": hashlib.sha256(raw).hexdigest()})


# --- wire seam: cheats 7 (delta_bytes, metrics_root) and 8 ---------------------------------------


def _stage(path: str) -> str:
    return path.partition("?")[0].rstrip("/").rsplit("/", 1)[-1]


class WireRule:
    """Rewrites or refuses a call of NetworkMiner.api; ``before`` returns (path, kwargs)."""

    def before(self, method: str, path: str, kw: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        return path, kw

    def after(self, method: str, path: str, result: Any) -> None:
        return None


class _WireApi(Api):
    def __init__(self, inner: Api, rule: WireRule) -> None:
        super().__init__(inner.c, inner.base)
        self.inner, self.rule = inner, rule

    def call(self, method: str, path: str, **kw: Any) -> Any:
        path, kw = self.rule.before(method, path, kw)
        result = self.inner.call(method, path, **kw)
        self.rule.after(method, path, result)
        return result


class _Withdraw(WireRule):
    def __init__(self, stage: str) -> None:
        self.stage = stage

    def before(self, method: str, path: str, kw: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        if method == "POST" and _stage(path) == self.stage:
            raise Withdrawn(self.stage)
        return path, kw


class _LateCommit(WireRule):
    def __init__(self, ctx: CheatContext) -> None:
        self.ctx, self.d_commit = ctx, None

    def after(self, method: str, path: str, result: Any) -> None:
        if method == "GET" and path.rsplit("/", 2)[-2] == "rounds":
            self.d_commit = int(result["round_open"]["body"]["d_commit"])

    def before(self, method: str, path: str, kw: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        if method == "POST" and _stage(path) == "commit":
            if self.d_commit is None:
                raise RuntimeError("late-commit saw no RoundOpen before the commit")
            self.ctx.advance_beacon(self.d_commit)
        return path, kw


class _CommitLie(WireRule):
    """Re-signs the commit with one field changed and keeps what binds to its digest (the
    WorkProof challenge_hash and the upload-grant commit subject) consistent, so the first
    refusal is the field's own check and not a digest mismatch of the follow-up calls."""

    def __init__(self, miner: Any, field: str) -> None:
        self.miner, self.field, self.digests = miner, field, ("", "")

    def _reseal(self, kind: str, env: dict[str, Any], body: dict[str, Any]) -> dict[str, Any]:
        return envelope_v2.seal(self.miner.kp, kind, self.miner.run_id, body, env["exp_drand"])

    def before(self, method: str, path: str, kw: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        if method != "POST":
            return path, kw
        stage, original, lied = _stage(path), *self.digests
        if stage == "commit":
            env = kw["json"]
            body = dict(env["body"])
            old = body[self.field]
            body[self.field] = old + 1 if isinstance(old, int) else lie(old)
            self.digests = (body_digest(env["body"]), body_digest(body))
            return path, {**kw, "json": self._reseal("CommitV2", env, body)}
        if stage == "leaves" and kw["json"]["body"]["challenge_hash"] == original:
            env = kw["json"]
            return path, {
                **kw,
                "json": self._reseal("WorkProof", env, {**env["body"], "challenge_hash": lied}),
            }
        if stage == "upload-grant" and original:
            return path.replace(original, lied), kw
        return path, kw


@dataclass(frozen=True, kw_only=True)
class WireCheat(Cheat):
    """Installs a WireRule on the miner's Api: ``stage`` (Withdrawn), ``late`` (the clock moves
    past d_commit before the commit; it moves the shared master clock, so run this miner last)
    or ``commit`` (one CommitV2 field, ``field``, is wrong and re-signed)."""

    kind: str
    target: str

    def rule(self, miner: Any, ctx: CheatContext) -> WireRule:
        match self.kind:
            case "stage":
                return _Withdraw(self.target)
            case "late":
                return _LateCommit(ctx)
            case "commit":
                return _CommitLie(miner, self.target)
        raise ValueError(self.kind)

    def wire(self, miner: Any, ctx: CheatContext) -> None:
        miner.api = _WireApi(miner.api, self.rule(miner, ctx))


# --- the registry ------------------------------------------------------------------------------


def _commit_lie(field: str, expected: str) -> Cheat:
    common = dict(
        cheat=7,
        name="commit-lie-" + field.replace("_", "-"),
        summary=f"the commit claims a wrong {field}; every uploaded byte is honest",
        expected=expected,
    )
    if field in SUMMARY_FIELDS:
        return SummaryLie(seam="artifacts", field=field, **common)
    return WireCheat(seam="wire", kind="commit", target=field, **common)


ALL: tuple[Cheat, ...] = (
    HONEST,
    WorkerCheat(
        cheat=1,
        name="altered-weights-one-ulp",
        summary="one weight is one ulp off after the last step; leaves, delta and commit follow",
        seam="worker",
        expected="audit",
        kind="ulp",
        step=None,
    ),
    WorkerCheat(
        cheat=1,
        name="altered-weights-one-ulp-first-step",
        summary="one weight is one ulp off after the first step, then training continues",
        seam="worker",
        expected="audit",
        kind="ulp",
        step=1,
    ),
    StateFileOnly(
        cheat=1,
        name="altered-weights-state-file-only",
        summary="only state.safetensors is edited after the commitments were computed",
        seam="artifacts",
        expected="none-inert",
    ),
    ReplayedRound(
        cheat=2,
        name="replay-previous-round-relabelled",
        summary="round w-1 leaves relabelled to round w with a recomputed root, old delta",
        seam="artifacts",
        expected="audit",
        needs_previous_round=True,
        relabel=True,
    ),
    ReplayedRound(
        cheat=2,
        name="replay-previous-round-verbatim",
        summary="the published tree of round w-1 is submitted again untouched",
        seam="artifacts",
        expected="intake:/leaves",
        needs_previous_round=True,
        relabel=False,
    ),
    ForgedDelta(
        cheat=3,
        name="forged-delta-valid-hash",
        summary="a valid codec payload of a rescaled delta, committed under its own sha256",
        seam="artifacts",
        expected="audit",
        scale=1.25,
    ),
    CopiedPeer(
        cheat=4,
        name="copied-peer-upload-resigned",
        summary="an honest peer's published tree is submitted as the miner's own",
        seam="artifacts",
        expected="audit",
    ),
    WorkerCheat(
        cheat=5,
        name="skipped-optimizer-step",
        summary="the optimizer update of the first step is not applied; the counter advances",
        seam="worker",
        expected="audit",
        kind="skip",
        step=1,
    ),
    TruncatedLeaves(
        cheat=5,
        name="truncated-step-count",
        summary="the last window is missing: one leaf fewer than the manifest's H//J + 1",
        seam="artifacts",
        expected="intake:/commit",
    ),
    WrongShard(
        cheat=6,
        name="wrong-data-shard",
        summary="trains on the assigned ids shifted by one, with matching rows and proofs",
        seam="job",
        expected="audit",
        shift=1,
    ),
    ForgedRngCtr(
        cheat=6,
        name="wrong-seed-forged-rng-ctr",
        summary="the rng counter of leaf 1 is wrong; the leaves root is recomputed",
        seam="artifacts",
        expected="audit",
        leaf=1,
    ),
    WrongInitialTheta(
        cheat=6,
        name="wrong-initial-theta",
        summary="trains from the weights of another init seed instead of the round start",
        seam="job",
        expected="audit",
    ),
    _commit_lie("final_theta_hash", "audit"),
    _commit_lie("ef_out_hash", "audit"),
    _commit_lie("leaves_root", "intake:/leaves"),
    _commit_lie("delta_hash", "intake:/upload-grant"),
    _commit_lie("delta_bytes", "intake:/upload-grant"),
    _commit_lie("metrics_root", "none-undetected"),
    WireCheat(
        cheat=8,
        name="no-show-before-commit",
        summary="the miner trains but never posts its commit",
        seam="stage",
        expected="stall",
        kind="stage",
        target="commit",
    ),
    WireCheat(
        cheat=8,
        name="late-commit",
        summary="the master clock passes d_commit before the miner posts its commit",
        seam="time",
        expected="intake:/commit",
        kind="late",
        target="commit",
    ),
    WireCheat(
        cheat=8,
        name="commit-without-leaves",
        summary="the commit is accepted; the leaves object is never published",
        seam="stage",
        expected="stall",
        kind="stage",
        target="leaves",
    ),
    WireCheat(
        cheat=8,
        name="commit-without-delta",
        summary="commit and leaves are accepted; no delta is ever uploaded",
        seam="stage",
        expected="stall",
        kind="stage",
        target="upload-grant",
    ),
)

BY_NAME = {c.name: c for c in ALL}
