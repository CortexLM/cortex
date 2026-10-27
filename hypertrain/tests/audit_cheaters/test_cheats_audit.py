"""Audit-level regression tests of the cheating-miner doubles (ac_cheats.py).

No network service: each double is applied to a staged island job on the tiny CPU model and the
miner's product is judged by the PRODUCTION auditor, ``execute_island_audit`` (real torchrun, the
code the master runs in ``execute_audit_v2``), or ``audit_island`` for the large honest sweeps.
The comparison is never re-implemented here. The HTTP-level behaviour (intake refusals, ledger
effects) is covered through ac_world.py by the other files of this directory.

Geometry of the cheat table: H=4, J=2, two logical ranks, state_policy carry, so there are three
leaves (t=0, 2, 4) and a defect in step s first shows in leaf ceil(s / 2).
"""

from __future__ import annotations

import hypertrain.trainer  # noqa: F401

# isort: split
import functools
import importlib.util
import json
import math
import os
import re
import shutil
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import ac_cheat_worker
import numpy as np
import pytest
import torch
from ac_cheats import (
    ALL,
    BY_NAME,
    EXPECTED,
    HONEST,
    SEAMS,
    Cheat,
    ForgedRngCtr,
    Withdrawn,
    WorkerCheat,
    artifacts_of,
    lie,
)
from ac_world import record
from pydantic import ValidationError

from hypertrain.auditor.replay import (
    AnchorCache,
    VerifiedAnchor,
    audit_island,
    optimizer_hash,
    pack_state,
)
from hypertrain.auditor.worker import LeaseGuard, execute_island_audit
from hypertrain.miner.island_launch import IslandArtifacts, launch_island
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.envelope_v2 import seal, verify_envelope
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import LeafPreimage
from hypertrain.protocol.messages_v2 import (
    ArtifactRef,
    AuditChallengeV2,
    AuditJobV2,
    CommitV2,
    IslandJobV1,
    RunManifestV2,
    StartStateV2,
    WorkProof,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import emulate, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params

LAUNCH_FIXTURE = Path(__file__).parents[1] / "miner/test_island_launch_v2.py"


@functools.cache
def _tiny_manifest(n_gpus: int) -> RunManifestV2:
    spec = importlib.util.spec_from_file_location("ac_island_launch_fixture", LAUNCH_FIXTURE)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.tiny_manifest(n_gpus)


@dataclass(frozen=True)
class Geometry:
    H: int = 4
    J: int = 2
    n_gpus: int = 2
    policy: str = "carry"
    init_seed: int = 0

    @property
    def batch(self) -> int:
        return self.H * self.n_gpus


@dataclass
class Start:
    """What the master hands a miner at round w: public theta plus its verified anchor."""

    w: int
    hotkey: str
    theta: dict[str, torch.Tensor]
    anchor: VerifiedAnchor
    ids: list[int]

    @property
    def global_step0(self) -> int:
        return self.anchor.state.step


@dataclass
class Product:
    """What a miner publishes for one round, whichever way it was produced."""

    preimages: list[dict[str, Any]]
    commit: dict[str, Any]


class World:
    """One manifest, one dataset, two miners (A cheats or is honest, B is an honest peer)."""

    def __init__(self, geometry: Geometry, seed: int = 0, miners: int = 2) -> None:
        g = self.geometry = geometry
        n_rows = 1 << math.ceil(math.log2(2 * g.batch))
        rng = np.random.default_rng(seed)
        self.rows = rng.integers(0, 16, (n_rows, 5), dtype=np.uint32)
        self.tree = MerkleTree([r.astype("<u4").tobytes() for r in self.rows])
        body = _tiny_manifest(g.n_gpus).body()
        body["training"]["inner"].update(H=g.H, J=g.J, state_policy=g.policy)
        body["training"]["model"]["init_seed"] = g.init_seed
        body["training"]["dataset"].update(
            merkle_root=self.tree.root.hex(), n_samples=n_rows, depth=int(math.log2(n_rows))
        )
        self.manifest = RunManifestV2.model_validate(body)
        self.cfg = TrainConfig.from_manifest_v2(self.manifest)
        self.layout = self.manifest.training.reference_spec.layout
        self.run_id = self.manifest.run_id()
        self.theta0 = init_params(self.cfg.model)
        self.deadline = int(time.time()) + 3600
        self.keys = [Keypair(bytes([60 + seed % 8 * 4 + i]) * 32) for i in range(miners)]
        self.auditor = Keypair(bytes([45]) * 32)
        self.cache = AnchorCache()
        for key in self.keys:
            self.cache.genesis(self.manifest, key.ss58, self.theta0)

    def key_of(self, hotkey: str) -> Keypair:
        return next(k for k in self.keys if k.ss58 == hotkey)

    def theta(self, w: int) -> dict[str, torch.Tensor]:
        return {k: v + 0.001 * w for k, v in self.theta0.items()}

    def start(self, who: int, w: int, rotate: int = 0) -> Start:
        key, layout = self.keys[who], self.cache.layout_hash(self.manifest)
        anchor = self.cache.entries[(self.run_id, key.ss58, w - 1, layout)]
        slot = (who + w + rotate) % 2
        ids = list(range(slot * self.geometry.batch, (slot + 1) * self.geometry.batch))
        return Start(w, key.ss58, self.theta(w), anchor, ids)

    def blobs(self, s: Start) -> tuple[bytes, bytes, bytes]:
        """Whole-state objects as the auditor receives them (theta and the anchor's m/v)."""
        derived = self.geometry.policy == "derived"
        return (
            pack_state(s.theta, s.anchor.state),
            pack_state(s.anchor.ef),
            pack_state(s.anchor.state.v if derived else {}),
        )

    def stage(self, directory: Path, s: Start) -> IslandJobV1:
        """The miner's job directory, laid out the way the master stages it (island_job_v2)."""
        full, ef, v0 = self.blobs(s)
        inputs = {
            "start_state": full if self.geometry.policy == "carry" else pack_state(s.theta),
            "ef_in": ef,
            "v0": v0,
            "samples": b"".join(self.rows[i].astype("<u4").tobytes() for i in s.ids),
            "sample_proofs": json.dumps(
                [[p.hex() for p in self.tree.proof(i)] for i in s.ids]
            ).encode(),
        }
        directory.mkdir(parents=True, exist_ok=True)
        for name, data in inputs.items():
            (directory / name).write_bytes(data)
        return IslandJobV1(
            job_version=1,
            run_id=self.run_id,
            w=s.w,
            manifest=self.manifest,
            sample_ids=list(s.ids),
            global_step0=s.global_step0,
            start_state_sha256=sha256_hex(inputs["start_state"]),
            ef_in_sha256=sha256_hex(inputs["ef_in"]),
            v0_sha256=sha256_hex(inputs["v0"]),
            object_paths={name: name for name in inputs},
            deadline=self.deadline,
        )

    def product(self, s: Start, art: IslandArtifacts) -> Product:
        """The commit exactly as NetworkMiner.run_round builds it (core.py:1250-1270)."""
        preimages = json.loads(art.leaves.read_bytes())
        summary = json.loads((art.directory / "rank-0/summary.json").read_bytes())["commitments"]
        commit = {
            "w": s.w,
            "hotkey": s.hotkey,
            "leaf_scheme": "ht-leaf-v1",
            "n_leaves": len(preimages),
            "metrics_root": MerkleTree(
                [bytes.fromhex(p["loss_f32"] + p["norm_f32"]) for p in preimages]
            ).root.hex(),
            "tokens": len(s.ids) * self.cfg.model.seq_len,
            "delta_bytes": art.delta.stat().st_size,
            "ef_in_hash": state_hash(s.anchor.ef),
            **{
                k: summary[k]
                for k in ("leaves_root", "final_theta_hash", "ef_out_hash", "delta_hash")
            },
        }
        return Product(preimages, commit)

    def trained(self, s: Start) -> Product:
        """The honest product of the in-process reference trainer (no subprocess)."""
        policy = self.geometry.policy
        result = emulate(
            self.geometry.n_gpus,
            lambda c: train_island(
                self.cfg,
                self.layout,
                c,
                s.theta,
                Assignment(self.run_id, s.w, tuple(s.ids), s.global_step0),
                lambda i: self.rows[i],
                carry=s.anchor.state if policy == "carry" else None,
                ef_in=s.anchor.ef,
                v0=s.anchor.state.v if policy == "derived" else None,
            ),
        )[0]
        preimages = [x.preimage.model_dump(mode="json") for x in result.leaves]
        commit = {
            "w": s.w,
            "hotkey": s.hotkey,
            "leaf_scheme": "ht-leaf-v1",
            "n_leaves": len(preimages),
            "metrics_root": MerkleTree(
                [bytes.fromhex(p["loss_f32"] + p["norm_f32"]) for p in preimages]
            ).root.hex(),
            "tokens": len(s.ids) * self.cfg.model.seq_len,
            "delta_bytes": len(result.delta_payload),
            "ef_in_hash": result.ef_in_hash,
            "leaves_root": result.leaves_root,
            "final_theta_hash": result.final_theta_hash,
            "ef_out_hash": result.ef_out_hash,
            "delta_hash": result.delta_hash,
        }
        return Product(preimages, commit)

    def audit_job(self, s: Start, product: Product, signer: Keypair | None = None) -> AuditJobV2:
        """The lease payload the master builds from the miner's commit and published leaves."""
        full, ef, v0 = self.blobs(s)
        key = signer or self.key_of(s.hotkey)
        start = StartStateV2(
            run_id=self.run_id,
            w=s.w,
            hotkey=s.hotkey,
            theta_hash=state_hash(s.theta),
            state_object_sha256=sha256_hex(full),
            opt_state_hash=optimizer_hash(s.anchor.state),
            ef_object_sha256=sha256_hex(ef),
            ef_hash=state_hash(s.anchor.ef),
            parent_anchor_hash=s.anchor.anchor_hash,
            global_step0=s.global_step0,
            anchor_verdict_hash=s.anchor.proof_hash,
        )
        challenge = AuditChallengeV2(
            w=s.w,
            target=s.hotkey,
            beacon_round=1,
            beacon_sig_sha256="22" * 32,
            mode="full",
            segments=[],
            reasons=["final"],
            serve_deadline=100,
            anchor_hash=start.digest(),
            audit_mode="anchored-full",
        )
        return AuditJobV2(
            run_id=self.run_id,
            job_id=sha256_hex(f"ac-job|{s.hotkey}|{s.w}|{product.commit['leaves_root']}".encode()),
            auditor_id=self.auditor.ss58,
            attempt=1,
            lease_nonce="44" * 32,
            lease_expires=50,
            absolute_deadline=100,
            reservation_id="55" * 32,
            replay_step_budget=2 * self.geometry.H,
            anchor_age=0,
            manifest=self.manifest,
            challenge_envelope=seal(self.auditor, "AuditChallengeV2", self.run_id, challenge, 100),
            commit_envelope=seal(key, "CommitV2", self.run_id, product.commit, 100),
            sample_ids=list(s.ids),
            start_state=start,
            preimages=[LeafPreimage.model_validate(p) for p in product.preimages],
            ef_in=ArtifactRef(sha256=sha256_hex(ef), size=len(ef)),
            v0=ArtifactRef(sha256=sha256_hex(v0), size=len(v0)),
            created_beacon=1,
        )

    def audit(self, job: AuditJobV2, s: Start, directory: Path) -> tuple[str, int | None]:
        """Production audit: master-staged inputs, real torchrun replay, verdict and first leaf."""
        full, ef, v0 = self.blobs(s)
        directory.mkdir(parents=True, exist_ok=True)
        for name, data in zip(("start_state", "ef_in", "v0"), (full, ef, v0), strict=True):
            (directory / name).write_bytes(data)
        (directory / "samples").write_bytes(
            b"".join(self.rows[i].astype("<u4").tobytes() for i in s.ids)
        )
        (directory / "sample_proofs").write_text(
            json.dumps([[p.hex() for p in self.tree.proof(i)] for i in s.ids])
        )
        with LeaseGuard(50, 100, time.time() + 300) as guard:
            outcome, _ = execute_island_audit(
                job,
                directory,
                self.cache,
                guard,
                now_round=2,
                deadline_unix=int(time.time()) + 300,
                backend="cpu",
            )
        return outcome.result, outcome.first_bad_leaf

    def audit_fast(self, job: AuditJobV2, s: Start) -> tuple[str, int | None]:
        """The same comparison in-process (audit_island under emulate): ~0.03 s, for sweeps."""
        full, ef, v0 = self.blobs(s)
        outcome = emulate(
            self.geometry.n_gpus,
            lambda c: audit_island(
                job, c, lambda i: self.rows[i], full, ef, v0, self.cache, now_round=2
            ),
        )[0][0]
        return outcome.result, outcome.first_bad_leaf


@dataclass
class Ctx:
    """ac_cheats.CheatContext for one audit-level miner."""

    w: int
    hotkey: str
    rows: Any
    tree: Any
    peer: IslandArtifacts | None = None
    previous: IslandArtifacts | None = None

    def __post_init__(self) -> None:
        self.beacons: list[int] = []

    def peer_artifacts(self) -> IslandArtifacts:
        assert self.peer is not None
        return self.peer

    def own_previous_artifacts(self) -> IslandArtifacts | None:
        return self.previous

    def advance_beacon(self, number: int) -> None:
        self.beacons.append(number)


class EchoApi:
    """Stand-in for the master: answers a POST with the body it was sent and records the call."""

    c = None
    base = ""

    def __init__(self, views: dict[str, Any] | None = None) -> None:
        self.views, self.log = views or {}, []

    def call(self, method: str, path: str, **kw: Any) -> Any:
        self.log.append((method, path))
        return self.views.get(path) if method == "GET" else kw.get("json")


def cached(honest: IslandArtifacts) -> Callable[..., IslandArtifacts]:
    """A launch that serves the already computed honest publication. Launching is deterministic:
    test_benign_variations_do_not_change_the_product pins that, byte for byte."""

    def launch(job: IslandJobV1, directory: Path, **kwargs: Any) -> IslandArtifacts:
        shutil.copytree(honest.directory, directory / "published")
        return artifacts_of(directory / "published")

    return launch


@dataclass
class Base:
    """One honest miner-round the cheats of a table start from."""

    start: Start
    art: IslandArtifacts
    peer: IslandArtifacts | None = None
    previous: IslandArtifacts | None = None


def mine(world: World, base: Base, cheat: Cheat, root: Path, label: str) -> Product:
    """Run ``cheat`` as the miner of ``base`` and return what it would post: leaves and commit."""
    s, directory = base.start, root / label
    job = world.stage(directory, s)
    ctx = Ctx(s.w, s.hotkey, world.rows, world.tree, base.peer, base.previous)
    real = launch_island if cheat.seam in ("worker", "job") else cached(base.art)
    art = cheat.launch(real, ctx)(job, directory, backend="cpu", trace=True)
    product = world.product(s, art)
    key = world.key_of(s.hotkey)
    miner = SimpleNamespace(kp=key, run_id=world.run_id, api=EchoApi())
    cheat.wire(miner, ctx)
    posted = miner.api.call(
        "POST", "/commit", json=seal(key, "CommitV2", world.run_id, product.commit, 100)
    )
    product.commit = posted["body"]
    return product


def judge(world: World, base: Base, product: Product, root: Path, label: str):
    return world.audit(world.audit_job(base.start, product), base.start, root / f"audit-{label}")


@pytest.fixture(scope="module")
def table(tmp_path_factory: pytest.TempPathFactory) -> SimpleNamespace:
    """Round 0 of the cheat table: honest miners A and B on disjoint slots, published and
    audited once (an audited MATCH is the anchor round 1 starts from)."""
    root = tmp_path_factory.mktemp("ac-table")
    world = World(Geometry())
    bases, verdicts = {}, {}
    for name, who in (("a", 0), ("b", 1)):
        s = world.start(who, 0)
        art = launch_island(world.stage(root / name, s), root / name, backend="cpu", trace=True)
        bases[name] = Base(s, art)
        product = world.product(s, art)
        verdicts[name] = judge(world, bases[name], product, root, name)
    bases["a"].peer = bases["b"].art
    return SimpleNamespace(root=root, world=world, base=bases, honest=verdicts)


@pytest.fixture(scope="module")
def round1(table: SimpleNamespace) -> SimpleNamespace:
    """Honest round 1 of miner A, started from its audited round-0 anchor, and its control."""
    world, root = table.world, table.root
    s = world.start(0, 1)
    art = launch_island(world.stage(root / "a1", s), root / "a1", backend="cpu", trace=True)
    base = Base(s, art, previous=table.base["a"].art)
    verdict = judge(world, base, world.product(s, art), root, "a1")
    return SimpleNamespace(base=base, honest=verdict)


# --- the cheat table: the production audit detects every self-consistent cheat ---------------


def window_of(step: int | None) -> int:
    """The leaf a defect of inner step ``step`` (None: the last step) first shows in. Leaf k binds
    the state after step k*J, so a step-level defect first shows in leaf ceil(step / J)."""
    g = Geometry()
    return math.ceil((g.H if step is None else step) / g.J)


def double_of[T: Cheat](name: str, kind: type[T]) -> T:
    double = BY_NAME[name]
    assert isinstance(double, kind)
    return double


# name -> first_bad_leaf, every row MISMATCH. The leaf of a worker double follows from its step
# (Geometry: H=4, J=2, leaves at t=0, 2, 4) and the forged rng_ctr from its leaf. Leaf 0 binds the
# start state and has no batch, so a copied peer (same round-0 start) and another shard first
# differ in leaf 1, and only a different start state shows in leaf 0. A defect that leaves every
# leaf honest (a forged delta, a lie in a committed hash) has no bad leaf: None.
TABLE: dict[str, int | None] = {
    "altered-weights-one-ulp": window_of(double_of("altered-weights-one-ulp", WorkerCheat).step),
    "altered-weights-one-ulp-first-step": window_of(
        double_of("altered-weights-one-ulp-first-step", WorkerCheat).step
    ),
    "forged-delta-valid-hash": None,
    "copied-peer-upload-resigned": 1,
    "skipped-optimizer-step": window_of(double_of("skipped-optimizer-step", WorkerCheat).step),
    "wrong-data-shard": 1,
    "wrong-seed-forged-rng-ctr": double_of("wrong-seed-forged-rng-ctr", ForgedRngCtr).leaf,
    "wrong-initial-theta": 0,
    "commit-lie-final-theta-hash": None,
    "commit-lie-ef-out-hash": None,
    "commit-lie-leaves-root": None,
    "commit-lie-delta-hash": None,
}


def test_honest_miners_are_matched(table: SimpleNamespace) -> None:
    """Defence: an honest miner is never flagged, with or without a neighbour in the round."""
    assert table.honest == {"a": ("MATCH", None), "b": ("MATCH", None)}


@pytest.mark.parametrize("name", TABLE)
def test_cheat_is_mismatch_at_the_expected_leaf(table: SimpleNamespace, name: str) -> None:
    """Defence: the independent replay refuses each cheat, at the first leaf it corrupts."""
    base, world = table.base["a"], table.world
    product = mine(world, base, BY_NAME[name], table.root, name)
    assert judge(world, base, product, table.root, name) == ("MISMATCH", TABLE[name])


@pytest.mark.parametrize(
    "name,neutral",
    [("forged-delta-valid-hash", {"scale": 1.0}), ("wrong-data-shard", {"shift": 0})],
    ids=["forged-delta-scale-1", "wrong-shard-shift-0"],
)
def test_a_double_with_its_alteration_switched_off_is_an_honest_miner(
    table: SimpleNamespace, name: str, neutral: dict[str, Any]
) -> None:
    """Control of the table: these two doubles rebuild a payload or a job by hand (a codec delta,
    shifted rows and Merkle proofs). With the alteration set to nothing (scale 1, shift 0) the
    rebuilt product is the honest one, byte for byte, and the audit MATCHes it, so the MISMATCH
    of the table is caused by the alteration and not by how the double builds its inputs."""
    base, world = table.base["a"], table.world
    control = replace(BY_NAME[name], name=name + "-neutral", **neutral)
    product = mine(world, base, control, table.root, control.name)
    assert product == world.product(base.start, base.art)
    assert judge(world, base, product, table.root, control.name) == table.honest["a"]


def _step_cheat(kind: str, step: int) -> Cheat:
    return WorkerCheat(
        cheat=1 if kind == "ulp" else 5,
        name=f"{kind}-at-step-{step}",
        summary=f"{kind} at step {step}",
        seam="worker",
        expected="audit",
        kind=kind,
        step=step,
    )


@pytest.mark.parametrize("kind,step", [("ulp", 3), ("skip", 3), ("skip", 4), ("skip", 2)])
def test_the_first_bad_leaf_is_the_window_of_the_altered_step(
    table: SimpleNamespace, kind: str, step: int
) -> None:
    """Defence: the audit localises a step-level cheat to the window that contains the step
    (J=2: steps 1-2 are leaf 1, steps 3-4 are leaf 2), not just somewhere in the round."""
    base, world = table.base["a"], table.world
    cheat = _step_cheat(kind, step)
    product = mine(world, base, cheat, table.root, cheat.name)
    assert judge(world, base, product, table.root, cheat.name) == ("MISMATCH", window_of(step))


def test_state_file_only_edit_changes_nothing_the_network_consumes(table: SimpleNamespace) -> None:
    """No harm done, not a detection. Editing state.safetensors after the commitments are
    computed changes no leaf and no commit field, and the audit replays to the same MATCH:
    the file is hashed into WorkProof.artifact_refs[0] and never read back (core.py:1282)."""
    base, world, label = table.base["a"], table.world, "altered-weights-state-file-only"
    product = mine(world, base, BY_NAME[label], table.root, label)
    edited = (table.root / label / "published/rank-0/state.safetensors").read_bytes()
    assert edited != base.art.state.read_bytes()
    assert product == world.product(base.start, base.art)
    assert judge(world, base, product, table.root, label) == table.honest["a"]


def test_truncated_step_count_is_refused_by_commit_validation(table: SimpleNamespace) -> None:
    """Defence: a commit with fewer leaves than H//J + 1 fails CommitV2.validate_assignment,
    the check POST /commit runs (store.py:6183); the honest commit passes it."""
    base, world = table.base["a"], table.world
    manifest, batch = world.manifest, len(base.start.ids)
    honest = world.product(base.start, base.art).commit
    CommitV2.model_validate(honest).validate_assignment(manifest, batch)
    truncated = mine(world, base, BY_NAME["truncated-step-count"], table.root, "truncated")
    assert truncated.commit["n_leaves"] == honest["n_leaves"] - 1
    with pytest.raises(ValueError, match="leaf count differs"):
        CommitV2.model_validate(truncated.commit).validate_assignment(manifest, batch)


def test_replayed_round_is_mismatch_at_the_start_state(
    table: SimpleNamespace, round1: SimpleNamespace
) -> None:
    """Defence: round w-1 leaves relabelled to round w (w and rng_ctr rewritten, root recomputed,
    old delta) fail the replay at leaf 0, because the round-w start state differs from round
    w-1's: the relabelled leaf 0 differs from the honest round-w leaf 0 in its stages only."""
    world, base, label = table.world, round1.base, "replay-previous-round-relabelled"
    assert round1.honest == ("MATCH", None)
    product = mine(world, base, BY_NAME[label], table.root, label)
    relabelled, honest = product.preimages[0], json.loads(base.art.leaves.read_bytes())[0]
    assert {k for k in relabelled if relabelled[k] != honest[k]} == {"stages"}
    assert judge(world, base, product, table.root, label) == ("MISMATCH", 0)


def test_verbatim_replay_carries_leaves_of_the_wrong_round(
    table: SimpleNamespace, round1: SimpleNamespace
) -> None:
    """Defence: leaves still labelled round w-1 cannot be built into a round-w audit job
    (AuditJobV2 refuses a preimage of another round); POST /leaves refuses them earlier
    (store.py:6282), so the master never reaches this build for such a miner."""
    world, base, label = table.world, round1.base, "replay-previous-round-verbatim"
    product = mine(world, base, BY_NAME[label], table.root, label)
    assert {p["w"] for p in product.preimages} == {0}
    with pytest.raises(ValidationError, match="run/round mismatch"):
        world.audit_job(base.start, product)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AC-METRICS-ROOT (MINEUR, inert): CommitV2.metrics_root is signed but the live audit "
    "never compares it (auditor/worker.py:202-209 checks leaves_root, delta_hash, "
    "final_theta_hash, ef_out_hash and ef_in_hash) and no round route reads it; only the trial "
    "publication custody checks it (admission.py:575-578). Loss and norm are verified through "
    "leaves_root, so a lie earns nothing today; it would mislead a future consumer of the field",
)
def test_a_lie_in_metrics_root_is_refused(table: SimpleNamespace) -> None:
    """Desired defence: a commit whose metrics_root is not the Merkle root over the loss and
    norm of its own leaves (the equation admission.py:575-578 already enforces, and the honest
    commit satisfies) is not MATCH. Today it is: the field is free to forge."""
    base, world, label = table.base["a"], table.world, "commit-lie-metrics-root"
    product = mine(world, base, BY_NAME[label], table.root, label)
    honest = world.product(base.start, base.art).commit
    if product.commit["metrics_root"] == honest["metrics_root"]:
        pytest.fail("setup: the commit-lie-metrics-root double did not change metrics_root")
    assert judge(world, base, product, table.root, label)[0] != "MATCH"


# --- behaviour of the wire and stage doubles (their first refusal is the master's) --------------


@pytest.mark.parametrize("field", ["delta_bytes", "metrics_root"])
def test_a_wire_commit_lie_changes_one_field_and_rebinds_the_follow_up_calls(
    table: SimpleNamespace, field: str
) -> None:
    """The lie is re-signed and the WorkProof and upload-grant subjects follow the new commit
    digest, so the master's first refusal is the field's own check, not a digest mismatch."""
    world, base = table.world, table.base["a"]
    key, url = world.key_of(base.start.hotkey), f"/v2/runs/{world.run_id}"
    honest = world.product(base.start, base.art).commit
    api = EchoApi()
    miner = SimpleNamespace(kp=key, run_id=world.run_id, api=api)
    BY_NAME["commit-lie-" + field.replace("_", "-")].wire(miner, Ctx(0, key.ss58, None, None))
    commit = miner.api.call(
        "POST", url + "/commit", json=seal(key, "CommitV2", world.run_id, honest, 100)
    )
    proof = WorkProof(
        admission_id="cd" * 32,
        challenge_hash=body_digest(honest),
        leaves_root=honest["leaves_root"],
        delta_hash=honest["delta_hash"],
        artifact_refs=[ArtifactRef(sha256="ef" * 32, size=1)],
    )
    leaves = miner.api.call(
        "POST", url + "/leaves", json=seal(key, "WorkProof", world.run_id, proof, 100)
    )
    miner.api.call("POST", url + "/upload-grant?commit_hash=" + body_digest(honest), json={})
    lied = body_digest(commit["body"])
    assert {k for k in honest if commit["body"][k] != honest[k]} == {field}
    assert verify_envelope(commit) and commit["signer"] == key.ss58
    assert verify_envelope(leaves) and leaves["body"]["challenge_hash"] == lied != body_digest(
        honest
    )
    assert api.log[-1][1] == url + "/upload-grant?commit_hash=" + lied


POSTS = ("accept", "commit", "leaves", "upload-grant", "delta")
WITHDRAWALS = {
    "no-show-before-commit": "commit",
    "commit-without-leaves": "leaves",
    "commit-without-delta": "upload-grant",
}


@pytest.mark.parametrize("name,stage", WITHDRAWALS.items())
def test_a_stage_double_performs_every_stage_before_its_own(name: str, stage: str) -> None:
    """The miner posts everything up to the stage it withdraws from, and nothing at or after it."""
    miner = SimpleNamespace(api=EchoApi())
    BY_NAME[name].wire(miner, Ctx(0, "", None, None))
    reached = []
    with pytest.raises(Withdrawn) as stopped:
        for post in POSTS:
            miner.api.call("POST", f"/v2/runs/r/{post}", json={})
            reached.append(post)
    assert stopped.value.stage == stage
    assert reached == list(POSTS[: POSTS.index(stage)])


def test_late_commit_moves_the_clock_to_d_commit_just_before_the_commit() -> None:
    """The accept still lands inside the window; the commit is sent after the clock reached
    d_commit, so the master (not the double) refuses it as late."""
    api = EchoApi({"/v2/runs/r/rounds/0": {"round_open": {"body": {"d_commit": 31}}}})
    ctx, miner = Ctx(0, "", None, None), SimpleNamespace(api=api)
    BY_NAME["late-commit"].wire(miner, ctx)
    miner.api.call("GET", "/v2/runs/r/rounds/0")
    miner.api.call("POST", "/v2/runs/r/accept", json={})
    assert ctx.beacons == []
    miner.api.call("POST", "/v2/runs/r/commit", json={})
    assert ctx.beacons == [31]
    assert api.log[-1] == ("POST", "/v2/runs/r/commit")


# --- false-positive control: honest miners are never flagged ------------------------------------
#
# Scope of this control. Miner and master replay run on ONE host, in one process family, with the
# environment this suite inherits: CI pins ATEN_CPU_CAPABILITY=avx2, MKL_CBWR=AVX2 and
# ONEDNN_MAX_CPU_ISA=AVX2 (hypertrain-ci.yml), this suite pins nothing, and each sweep records
# the values it ran under. The sweeps prove that honest products replay bitwise across slots,
# rounds, state policies, geometries, seeds, work directories and hash seeds. They do not prove
# cross-host, cross-ISA, driver or CUDA stability (finding DET-01: honest miners on another CPU
# class can get a MISMATCH), and the tiny model does not exhibit the AVX2 versus AVX-512
# divergence measured elsewhere. Power: if the audits were independent, zero flags in N bounds
# the false-positive rate below about 3/N (95%): 0.6% for the 540 fast audits, 25% for the 12
# real ones. A mutation of audit_island that flags 1 audit in 200 fails the fast sweep; the real
# sweep is a smoke test of the launch path, not a rate estimate.

CPU_ENV = ("ATEN_CPU_CAPABILITY", "MKL_CBWR", "ONEDNN_MAX_CPU_ISA")


def cpu_env() -> dict[str, str | None]:
    return {name: os.environ.get(name) for name in CPU_ENV}


FAST_POLICIES = ("reset", "carry", "derived")
FAST_GEOMETRIES = ((2, 1, 1), (2, 1, 2), (4, 2, 2), (4, 1, 2), (4, 4, 1), (8, 2, 1))  # H, J, ranks
FAST_SEEDS = range(5)  # dataset, model init, hotkeys, slot rotation
FAST_ROUNDS = 3  # chained: round w starts from the audited anchor of round w-1
FAST_SWEEP_N = len(FAST_POLICIES) * len(FAST_GEOMETRIES) * len(FAST_SEEDS) * FAST_ROUNDS * 2


@pytest.mark.parametrize("policy", FAST_POLICIES)
@pytest.mark.parametrize("H,J,n_gpus", FAST_GEOMETRIES)
def test_fast_sweep_never_flags_an_honest_miner(policy: str, H: int, J: int, n_gpus: int) -> None:
    """Defence: across slots, rounds, policies, geometries and seeds the production replay
    (audit_island under emulate; this case audits 30, FAST_SWEEP_N = 540 over all cases) says
    MATCH for an honest product. Single host and environment: see the scope note above."""
    flagged, audited = [], 0
    for seed in FAST_SEEDS:
        world = World(Geometry(H, J, n_gpus, policy, init_seed=seed), seed)
        for w in range(FAST_ROUNDS):
            for who in (0, 1):
                s = world.start(who, w, rotate=seed)
                verdict = world.audit_fast(world.audit_job(s, world.trained(s)), s)
                audited += 1
                if verdict != ("MATCH", None):
                    flagged.append((seed, w, who, verdict))
    assert audited == len(FAST_SEEDS) * FAST_ROUNDS * 2
    record(
        {
            "test": "honest-sweep-fast",
            "policy": policy,
            "geometry": [H, J, n_gpus],
            "audited": audited,
            "flagged": len(flagged),
            "cpu_env": cpu_env(),
        }
    )
    assert flagged == []


def test_the_fast_audit_path_flags_a_cheat() -> None:
    """The sweep above would pass vacuously if audit_island accepted everything: the same
    pipeline refuses a commit that claims a wrong final_theta_hash."""
    world = World(Geometry(2, 1, 2, "carry"))
    s = world.start(0, 0)
    product = world.trained(s)
    product.commit["final_theta_hash"] = lie(product.commit["final_theta_hash"])
    assert world.audit_fast(world.audit_job(s, product), s) == ("MISMATCH", None)


# Real torchrun on both sides: the miner's production launch, the master's production audit.
REAL_CASES = [
    (policy, geometry, rounds)
    for policy in FAST_POLICIES
    for geometry, rounds in (((2, 1, 1), 1), ((4, 1, 2), 1), ((4, 2, 2), 2))
]
REAL_SWEEP_N = sum(rounds for _, _, rounds in REAL_CASES)


@pytest.mark.parametrize(
    "policy,geometry,rounds",
    REAL_CASES,
    ids=[f"{p}-H{g[0]}J{g[1]}n{g[2]}-{r}rounds" for p, g, r in REAL_CASES],
)
def test_real_torchrun_sweep_never_flags_an_honest_miner(
    tmp_path: Path, policy: str, geometry: tuple[int, int, int], rounds: int
) -> None:
    """Defence: a real torchrun miner launch is MATCHed by the real torchrun audit, for every
    state policy and geometry, in the first round and (chained) in the next (this case audits
    `rounds`, REAL_SWEEP_N = 12 over all cases). A smoke test of the launch path on one host and
    environment, one seed per case: see the scope note above."""
    H, J, n_gpus = geometry
    world = World(Geometry(H, J, n_gpus, policy, init_seed=H + n_gpus), seed=H + J + n_gpus)
    verdicts = []
    for w in range(rounds):
        s, directory = world.start(0, w), tmp_path / str(w)
        art = launch_island(world.stage(directory, s), directory, backend="cpu", trace=True)
        verdicts.append(judge(world, Base(s, art), world.product(s, art), tmp_path, str(w)))
        if verdicts[-1] != ("MATCH", None):
            break  # the next round would start from an anchor that was never verified
    record(
        {
            "test": "honest-sweep-real",
            "policy": policy,
            "geometry": [H, J, n_gpus],
            "audited": len(verdicts),
            "flagged": sum(v != ("MATCH", None) for v in verdicts),
            "cpu_env": cpu_env(),
        }
    )
    assert verdicts == [("MATCH", None)] * rounds


def _fingerprint(art: IslandArtifacts) -> dict[str, Any]:
    """Everything a miner commits to, uploads or serves in a dispute; no timing fields."""
    summary = json.loads((art.directory / "rank-0/summary.json").read_bytes())
    return {
        "commitments": summary["commitments"],
        "files": {
            p.name: sha256_hex(p.read_bytes())
            for p in (art.state, art.ef, art.delta, art.leaves, art.directory / "rank-0/trace.json")
        },
    }


def test_benign_variations_do_not_change_the_product(
    table: SimpleNamespace, tmp_path: Path
) -> None:
    """Defence against false positives: the same job gives byte-identical artifacts in another
    work directory, in a second interpreter with a different hash seed, and a restart is served
    the validated publication unchanged (launch_island does not recompute it); the master's
    audit MATCHes the second interpreter's product."""
    world, base = table.world, table.base["a"]
    s, honest = base.start, _fingerprint(base.art)
    elsewhere = tmp_path / "deeper" / "elsewhere"
    moved = launch_island(world.stage(elsewhere, s), elsewhere, backend="cpu", trace=True)
    before_restart = _fingerprint(moved)
    restarted = launch_island(world.stage(elsewhere, s), elsewhere, backend="cpu", trace=True)
    other = tmp_path / "second-process"
    job = world.stage(other, s)
    script = (
        "import sys; import hypertrain.trainer; from pathlib import Path\n"
        "from hypertrain.miner.island_launch import launch_island\n"
        "from hypertrain.protocol.messages_v2 import IslandJobV1\n"
        "job = IslandJobV1.model_validate_json(Path(sys.argv[1]).read_text())\n"
        "launch_island(job, Path(sys.argv[2]), backend='cpu', trace=True)\n"
    )
    (tmp_path / "job.json").write_text(job.model_dump_json())
    hash_seed = "321" if os.environ.get("PYTHONHASHSEED") == "123" else "123"
    subprocess.run(
        [sys.executable, "-c", script, str(tmp_path / "job.json"), str(other)],
        check=True,
        timeout=600,
        env={**os.environ, "PYTHONHASHSEED": hash_seed},
    )
    assert before_restart == _fingerprint(restarted) == honest
    second = artifacts_of(other / "published")
    assert _fingerprint(second) == honest
    assert judge(world, Base(s, second), world.product(s, second), tmp_path, "second") == (
        "MATCH",
        None,
    )


# --- the table stays honest -----------------------------------------------------------------


def test_every_cheat_of_the_brief_has_a_double() -> None:
    assert {c.cheat for c in ALL} == set(range(9))
    assert [c.cheat for c in ALL] == sorted(c.cheat for c in ALL)


def test_names_are_unique_kebab_case_and_documented() -> None:
    names = [c.name for c in ALL]
    assert len(set(names)) == len(names)
    assert all(re.fullmatch(r"[a-z0-9]+(-[a-z0-9]+)*", n) for n in names)
    assert all(c.summary and c.seam in SEAMS and c.expected in EXPECTED for c in ALL)


def test_every_audit_expectation_is_exercised_by_this_file() -> None:
    audited = {c.name for c in ALL if c.expected == "audit"}
    assert audited <= set(TABLE) | {"replay-previous-round-relabelled"}


def test_a_double_changes_only_the_seam_it_declares() -> None:
    real, ctx = object(), Ctx(1, "", None, None)
    for c in ALL:
        miner = SimpleNamespace(kp=None, run_id="", api=EchoApi())
        api = miner.api
        c.wire(miner, ctx)
        assert (c.launch(real, ctx) is not real) == (c.seam in {"worker", "artifacts", "job"}), c
        assert (miner.api is not api) == (c.seam in {"wire", "stage", "time"}), c
    assert HONEST.seam == "none" and HONEST.cheat == 0


def test_worker_doubles_name_a_kind_the_worker_shim_installs() -> None:
    kinds = {c.kind for c in ALL if isinstance(c, WorkerCheat)}
    assert kinds == set(ac_cheat_worker.INSTALL)


def test_replay_doubles_declare_that_they_need_a_previous_round() -> None:
    assert {c.name for c in ALL if c.needs_previous_round} == {
        "replay-previous-round-relabelled",
        "replay-previous-round-verbatim",
    }
