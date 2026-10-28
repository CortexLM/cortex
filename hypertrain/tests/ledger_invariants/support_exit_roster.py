"""Builders private to the exit_roster package. Not collected: no test_ prefix.

The world is the v2 service of `tests/e2e/test_rollback_service_v2.py`, loaded once under a
private module name so that nothing here touches the copy other packages build on. Identities are
made ACTIVE by SQL, as `activated_network` of tests/e2e/test_dispute_exit_v2_e2e.py does: the
12-trial ladder has its own e2e. A rostered miner is either REAL (the tiny CPU island runs, the
audit replays it and says MATCH) or FORGED (a commit that is well formed, with leaves that hash
to its root and no training behind them: enough for everything that happens before an audit
executes). All state changes go through the routes a miner, an auditor and the operator use; the
exceptions are named where they happen (the relay's acceptance record, a completed job).

Every lease of this fixture's round schedule has to be issued at the d_audit beacon itself: a
lease asked one round later cannot be built (AuditJobV2 refuses a lease that outlives its
creation round by more than 100 rounds, and d_final - d_audit is 169). The helpers therefore
never move the beacon between scheduling and leasing.
"""

from __future__ import annotations

import json
from collections.abc import Iterator, Sequence
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from fastapi.testclient import TestClient

from hypertrain.challenge.store import ChallengeStore
from hypertrain.miner import admission as miner_admission
from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import (
    Finalize,
    LeafPreimage,
    StageState,
)
from hypertrain.protocol.messages_v2 import (
    AuditPolicyV2,
    CommitV2,
    DisputeV2,
    EscrowLock,
    IslandJobV1,
    RankWorkResult,
    RotateRequest,
    RoundOpenV2,
    RunManifestV2,
    WorkProof,
    WorkScreenV2,
)
from hypertrain.protocol.relay_messages import MasterAcceptanceV2

from .support import World, load_fixture_module

ROLLBACK_FIXTURE = ("li_exit_roster_rollback_fixture", "e2e/test_rollback_service_v2.py")
LAUNCH_SECONDS = 300  # a bound on a tiny CPU launch, never a threshold of a test
MINERS = 6  # five real members and one that goes quiet
SPARE = MINERS  # a registered hotkey that never joined: the only legal rotation target
UNROSTERED = MINERS - 1  # funded and ACTIVE, and listed by none of the rounds the scenarios open
# A tree with the exit reaper closes a round that nobody can carry by retiring it as a whole; an
# older one excludes each member that left it.
RETIRES = hasattr(ChallengeStore, "_retire_round_v2")
FUNDED = ("ACTIVE", 0, 1000, 0)  # the standing of an untouched member: see `standing`


class ScenarioError(Exception):
    """A control failed: the scenario did not reach the point a strict xfail test is about.

    Not an AssertionError, so that an xfail(raises=AssertionError) cannot swallow it. Pytest turns
    an AssertionError raised in a fixture or in a helper into an expected failure as well, so every
    step that builds a scenario checks with `control`, never with `assert`.
    """


def control(condition: object, what: str) -> None:
    """Require `what` to hold before the assertion under test; its failure is a broken scenario."""
    if not condition:
        raise ScenarioError(what)


def rollback() -> ModuleType:
    return load_fixture_module(*ROLLBACK_FIXTURE)


def service() -> ModuleType:
    """The network fixture module of this package's private copy (HOT, COLD, AUDITORS, ...)."""
    fixtures: ModuleType = rollback().fixtures
    return fixtures


def hot(index: int) -> Keypair:
    key: Keypair = service().HOT[index]
    return key


def cold(index: int) -> Keypair:
    key: Keypair = service().COLD[index]
    return key


@contextmanager
def activated_network(root: Path) -> Iterator[Any]:
    """The v2 `network` with six ACTIVE, funded identities and a seventh registered hotkey.

    The manifest uses the carry state policy (a second round starts from the first one's state)
    and the genesis allocates to the six coldkeys.
    """
    fixtures = service()
    patch = pytest.MonkeyPatch()
    fixtures.HOT = [Keypair(bytes([80 + i]) * 32) for i in range(MINERS + 1)]
    fixtures.COLD = [Keypair(bytes([90 + i]) * 32) for i in range(MINERS)]
    original = fixtures.fixture.setup

    def carry_setup() -> Any:
        setup = original()
        body = setup.manifest.body()
        body["training"]["inner"].update(state_policy="carry", rewarmup_steps=0)
        return fixtures.fixture.Setup(
            RunManifestV2.model_validate(body),
            setup.policy,
            setup.admission_policy,
            setup.rows,
            setup.tree,
        )

    patch.setattr(fixtures.fixture, "setup", carry_setup)
    original_genesis = fixtures.Genesis

    def full_genesis(**fields: Any) -> Any:
        fields["total_units"] = sum(origin.units for origin in fields["origins"])
        return original_genesis(**fields)

    patch.setattr(fixtures, "Genesis", full_genesis)
    root.mkdir(parents=True, exist_ok=True)
    generator = fixtures.network.__wrapped__(root, type("Request", (), {"param": None})())
    try:
        network = next(generator)
        for index in range(MINERS):
            if index:
                network.push(network.now + 101)
            activate(network, index)
        yield network
    finally:
        generator.close()
        patch.undo()


def activate(network: Any, index: int) -> str:
    """Join identity `index` and make it ACTIVE and funded; returns its admission id."""
    response = network.join(index)
    control(response.status_code == 200, response.text)
    identity = str(response.json()["admission_id"])
    lock = EscrowLock(
        operation_id=f"{index + 1:064x}",
        owner=cold(index).ss58,
        units=1000,
        origin_ids=[network.origin_ids[index]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(cold(index), "EscrowLock", lock)
    )
    control(response.status_code == 200, response.text)
    spec = network.manifest.training.reference_spec
    screen = WorkScreenV2(
        evidence_kind="work-proof-v1",
        admission_id=identity,
        nonce=sha256_hex(b"nonce"),
        policy_hash=network.manifest.network.admission_policy_hash,
        image_digest=spec.image_digest,
        layout=spec.layout,
        challenge_hash=sha256_hex(b"challenge"),
        rank_results=[
            RankWorkResult(
                rank=rank,
                fingerprint=sha256_hex(b"rank"),
                elapsed_ns=1,
                allocated_bytes=1,
                reserved_bytes=1,
                total_bytes=1 << 30,
            )
            for rank in range(spec.layout.n_gpus)
        ],
        artifact_hashes=[sha256_hex(b"artifact")],
    )
    with network.store._tx():
        network.store._db.execute(
            "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,screen_json=?,"
            "screen_until=1000000000 WHERE admission_id=?",
            (screen.model_dump_json(), identity),
        )
    return identity


def world_of(network: Any) -> World:
    """The shared `World` view (row counts, store digest) over this package's network."""
    return World(network, network.client.app, network.client, service().admin(), {}, {})


@dataclass
class Round:
    """One opened round of `members` (indices into HOT)."""

    network: Any
    w: int
    members: list[int]
    opening: RoundOpenV2
    assignments: dict[str, str] = field(default_factory=dict)
    quiet: int | None = None  # the member that stopped answering, if the scenario has one

    @property
    def store(self) -> Any:
        return self.network.store

    @property
    def run_id(self) -> str:
        return str(self.network.manifest.run_id())

    @property
    def absolute(self) -> int:
        """The round the audit jobs of this round are exhausted at."""
        return min(self.opening.d_final, self.opening.d_audit + 200)

    def hotkey(self, index: int) -> str:
        return hot(index).ss58

    def identity(self, index: int) -> str:
        return f"{self.w}:{self.hotkey(index)}"

    def job_id(self, index: int) -> str:
        return sha256_hex(f"ht-audit-job/2|{self.run_id}|{self.w}|{self.hotkey(index)}".encode())

    def queue_order(self) -> list[int]:
        """Members in the order lease_v2 serves their jobs: `created` is shared, so by job id."""
        return sorted(self.members, key=self.job_id)

    def post(self, path: str, body: Any = None, **kwargs: Any) -> Any:
        return self.network.client.post(self.network.url + path, json=body, **kwargs)

    def record(self, kind: str, identity: str) -> dict[str, Any]:
        return dict(self.store._record_v2(self.run_id, kind, identity))

    def has_record(self, kind: str, identity: str) -> bool:
        row = self.store._db.execute(
            "SELECT 1 FROM records_v2 WHERE run_id=? AND kind=? AND id=?",
            (self.run_id, kind, identity),
        ).fetchone()
        return row is not None

    def finality(self) -> dict[str, Any]:
        return self.record("finality", str(self.w))

    def lease_rows(self) -> list[dict[str, Any]]:
        rows = self.store._db.execute(
            "SELECT id,hotkey,state,attempts,auditor,result,expires,absolute FROM audit_leases_v2 "
            "WHERE run_id=? AND w=? ORDER BY created,id",
            (self.run_id, self.w),
        ).fetchall()
        return [dict(row) for row in rows]

    def excluded(self) -> dict[str, str]:
        """hotkey -> reason of every member excluded from this round."""
        rows = self.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE run_id=? AND kind='excluded' AND id LIKE ?",
            (self.run_id, f"{self.w}:%"),
        ).fetchall()
        return {row["id"].split(":", 1)[1]: json.loads(row["data"])["reason"] for row in rows}

    def paid(self) -> dict[str, int]:
        """hotkey -> units issued as the reward of this round."""
        rows = self.store._db.execute(
            "SELECT hotkey,issued FROM escrow_origins WHERE reward_round=?", (self.w,)
        ).fetchall()
        return {row["hotkey"]: int(row["issued"]) for row in rows}

    def disabled(self, auditor: Keypair) -> bool:
        """The master refuses to lease to `auditor`: the permanent record of the old audit path or
        the bounded cooldown record that replaced it (ADR 0011 part 2)."""
        return self.has_record("auditor-disabled", auditor.ss58) or self.has_record(
            "auditor-cooldown", auditor.ss58
        )


def open_round(network: Any, w: int, members: Sequence[int]) -> Round:
    """Open round `w` for `members` through the coordinator-signed route and move to d_assign."""
    response = rollback().execute_round(
        network, w, open_only=True, hotkeys=[hot(i) for i in members]
    )
    control(response.status_code == 200, response.text)
    return enter_round(network, w, members)


def enter_round(network: Any, w: int, members: Sequence[int]) -> Round:
    """Move to d_assign of the round the master just accepted and read the members' assignments."""
    opening = RoundOpenV2.model_validate(
        network.store._record_v2(network.manifest.run_id(), "round", str(w))["body"]
    )
    network.push(opening.d_assign)
    answer = network.client.get(network.url + f"/rounds/{w}")
    control(answer.status_code == 200, answer.text)
    rnd = Round(network, w, list(members), opening)
    rnd.assignments = {a["hotkey"]: a["assignment_hash"] for a in answer.json()["assignment"]}
    return rnd


def open_after(rnd: Round, members: Sequence[int]) -> tuple[Any, Round | None]:
    """Open round w+1 for `members` once round w is behind, as the operator does: the master's
    answer, and the opened round when it is 200. An opening is built on the state round w applied;
    when none exists the master's verdict on the missing state is asked of an opening that repeats
    round w (nothing but the schedule and the signature can be right in it)."""
    net = rnd.network
    if not rnd.has_record("applied", str(rnd.w)):
        return next_opening(rnd, members), None
    response = rollback().execute_round(
        net, rnd.w + 1, open_only=True, hotkeys=[hot(i) for i in members]
    )
    if response.status_code != 200:
        return response, None
    return response, enter_round(net, rnd.w + 1, members)


def accept(rnd: Round, index: int) -> Any:
    """POST /accept for `index`; returns the response unchecked."""
    net = rnd.network
    screen = json.loads(
        net.store._db.execute(
            "SELECT screen_json FROM admissions_v2 WHERE hotkey=?", (rnd.hotkey(index),)
        ).fetchone()[0]
    )
    spec = net.manifest.training.reference_spec
    body = {
        "w": rnd.w,
        "hotkey": rnd.hotkey(index),
        "assignment_hash": rnd.assignments[rnd.hotkey(index)],
        "image_digest": spec.image_digest,
        "driver_version": spec.driver_allowlist[0],
        "n_gpus": 1,
        "work_screen_hash": sha256_hex(canonicalize(screen)),
    }
    return rnd.post("/accept", net.signed(hot(index), "AcceptV2", body))


def forged_leaves(rnd: Round, index: int) -> list[LeafPreimage]:
    """The leaf preimages of a commit that has no training behind it (H // J + 1 of them)."""
    inner = rnd.network.manifest.training.inner
    leaves = []
    for step in range(0, inner.H + 1, inner.J):
        seed = sha256_hex(f"exit-roster|{rnd.hotkey(index)}|{rnd.w}|{step}".encode())
        leaves.append(
            LeafPreimage(
                run_id=rnd.run_id,
                w=rnd.w,
                t=step,
                stages=[StageState(theta=seed, m=seed, v=seed)],
                batch_ids_sha256=seed,
                rng_ctr=step,
                loss_f32="0000803f",
                norm_f32="0000803f",
            )
        )
    return leaves


def forged_commit(rnd: Round, index: int) -> CommitV2:
    """A commit whose leaves hash to its root, for a member that did not train."""
    leaves = forged_leaves(rnd, index)
    seed = sha256_hex(f"exit-roster-commit|{rnd.hotkey(index)}|{rnd.w}".encode())
    sample_ids = rnd.store._assignment_v2(rnd.run_id, rnd.w, rnd.hotkey(index))
    return CommitV2(
        w=rnd.w,
        hotkey=rnd.hotkey(index),
        leaf_scheme="ht-leaf-v1",
        n_leaves=len(leaves),
        leaves_root=MerkleTree([bytes.fromhex(p.digest()) for p in leaves]).root.hex(),
        metrics_root=seed,
        final_theta_hash=seed,
        ef_in_hash=rnd.record("start", rnd.identity(index))["ef_hash"],
        ef_out_hash=seed,
        delta_hash=seed,
        delta_bytes=1,
        tokens=len(sample_ids) * rnd.network.manifest.training.model.seq_len,
    )


def post_commit(rnd: Round, index: int, commit: CommitV2, *, expiry: int | None = None) -> Any:
    """POST /commit. `expiry` is the round the member seals its commit to (an honest miner seals
    d_final); the default is the far expiry of the shared fixture."""
    envelope = rnd.network.signed(
        hot(index), "CommitV2", commit, **({} if expiry is None else {"expiry": expiry})
    )
    return rnd.post("/commit", envelope)


def publish_leaves(rnd: Round, index: int, leaves: list[LeafPreimage], commit: CommitV2) -> Any:
    """POST /leaves: the signed descriptor of the leaf object the commit's root commits to."""
    net = rnd.network
    raw = json.dumps([p.model_dump(mode="json") for p in leaves]).encode()
    admission = net.store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (rnd.hotkey(index),)
    ).fetchone()[0]
    proof = WorkProof(
        admission_id=admission,
        challenge_hash=sha256_hex(canonicalize(commit.model_dump(mode="json"))),
        leaves_root=commit.leaves_root,
        delta_hash=commit.delta_hash,
        artifact_refs=[{"sha256": net.store.objects.put(raw), "size": len(raw)}],
    )
    return rnd.post("/leaves", net.signed(hot(index), "WorkProof", proof))


def join_forged(
    rnd: Round, index: int, *, leaves: bool = True, commit_expiry: int | None = None
) -> CommitV2:
    """Accept, commit and (unless `leaves` is False) publish leaves for a member that did not
    train. Every step is checked: the member is a well behaved participant of the scenario."""
    response = accept(rnd, index)
    control(response.status_code == 200, response.text)
    commit = forged_commit(rnd, index)
    response = post_commit(rnd, index, commit, expiry=commit_expiry)
    control(response.status_code == 200, response.text)
    if leaves:
        response = publish_leaves(rnd, index, forged_leaves(rnd, index), commit)
        control(response.status_code == 200, response.text)
    return commit


def train(rnd: Round, indices: Sequence[int]) -> None:
    """Accept, train the tiny model on CPU, commit, publish leaves and deliver the delta of each
    member of `indices`. The CPU launches are independent subprocesses and run side by side."""
    net = rnd.network
    jobs = {}
    for index in indices:
        response = accept(rnd, index)
        control(response.status_code == 200, response.text)
        staged = rnd.store.island_job_v2(rnd.run_id, rnd.w, rnd.hotkey(index))
        jobs[index] = IslandJobV1.model_validate(staged["job"])
    with ThreadPoolExecutor(max_workers=len(jobs)) as pool:
        futures = {
            index: pool.submit(
                launch_island,
                job,
                rnd.store.state_dir / "jobs-v2" / str(rnd.w) / rnd.hotkey(index),
                backend="cpu",
            )
            for index, job in jobs.items()
        }
        published = {
            index: future.result(timeout=LAUNCH_SECONDS) for index, future in futures.items()
        }
    for index, artifacts in published.items():
        summary = json.loads((artifacts.directory / "rank-0/summary.json").read_bytes())
        leaves = [LeafPreimage.model_validate(p) for p in json.loads(artifacts.leaves.read_bytes())]
        commit = CommitV2(
            w=rnd.w,
            hotkey=rnd.hotkey(index),
            leaf_scheme="ht-leaf-v1",
            n_leaves=len(leaves),
            metrics_root=MerkleTree(
                [bytes.fromhex(p.loss_f32 + p.norm_f32) for p in leaves]
            ).root.hex(),
            tokens=len(jobs[index].sample_ids) * net.manifest.training.model.seq_len,
            ef_in_hash=rnd.record("start", rnd.identity(index))["ef_hash"],
            delta_bytes=artifacts.delta.stat().st_size,
            **{
                k: summary["commitments"][k]
                for k in ("leaves_root", "final_theta_hash", "ef_out_hash", "delta_hash")
            },
        )
        committed = post_commit(rnd, index, commit)
        control(committed.status_code == 200, committed.text)
        published_leaves = publish_leaves(rnd, index, leaves, commit)
        control(published_leaves.status_code == 200, published_leaves.text)
        deliver_delta(rnd, index, artifacts.delta.read_bytes())


def deliver_delta(rnd: Round, index: int, delta: bytes) -> None:
    """POST /delta. The relay upload is not under test: the master's acceptance record, which the
    relay receipt route would write, is written here with the same sealed type, and next to it the
    receipt it names, reduced to the fields a delta is checked against (round, hotkey, grant)."""
    net, store = rnd.network, rnd.store
    delta_hash = sha256_hex(delta)
    grant_hash = sha256_hex(b"exit-roster-grant|" + rnd.identity(index).encode())
    acceptance = MasterAcceptanceV2(
        w=rnd.w,
        hotkey=rnd.hotkey(index),
        delta_hash=delta_hash,
        receipt_hash=sha256_hex(b"exit-roster-receipt|" + rnd.identity(index).encode()),
        received_drand=net.now,
    )
    signed = relay_envelope.seal(
        store._coord(), "MasterAcceptanceV2", rnd.run_id, acceptance, 10_000
    )
    with store._tx():
        store.objects.put(delta)
        store._put_record_v2(rnd.run_id, "master-acceptance", acceptance.digest(), signed)
        store._put_record_v2(
            rnd.run_id,
            "relay-receipt",
            acceptance.receipt_hash,
            {"body": {"w": rnd.w, "hotkey": rnd.hotkey(index), "grant_hash": grant_hash}},
        )
    body = {
        "w": rnd.w,
        "hotkey": rnd.hotkey(index),
        "delta_hash": delta_hash,
        "size": len(delta),
        "uri": "sha256:" + delta_hash,
        "format": "ht-sparse-v1",
        "chunks": [{"off": 0, "len": len(delta), "sha256": delta_hash}],
        "grant_hash": grant_hash,
        "master_acceptance_hash": acceptance.digest(),
    }
    response = rnd.post("/delta", net.signed(hot(index), "DeltaManifestV2", body))
    control(response.status_code == 200, response.text)


def schedule(rnd: Round) -> Any:
    """Move to d_audit and POST the operator's audits route; returns the response unchecked."""
    rnd.network.push(rnd.opening.d_audit)
    return rnd.post(f"/admin/rounds/{rnd.w}/audits", headers=service().admin())


def lease_ask(rnd: Round, auditor: Keypair, tag: int, received: int | None = None) -> bytes:
    """The signed bytes of one lease ask; `tag` makes its (w, commit_hash) unique."""
    body = {
        "w": rnd.w,
        "commit_hash": f"{rnd.w * 1000 + tag:064x}",
        "received_round": rnd.network.now if received is None else received,
    }
    return canonicalize(rnd.network.signed(auditor, "Receipt", body))


def post_lease(rnd: Round, raw: bytes) -> Any:
    return rnd.network.client.post(rnd.network.url + "/worker/lease", content=raw)


def audit_queue(rnd: Round, jobs: int, first_tag: int = 1) -> None:
    """Audit the `jobs` jobs at the head of the queue, two at a time (two jobs may run at once),
    and require a MATCH verdict for each. The master replays the jobs of a pair side by side."""
    net = rnd.network
    auditors = service().AUDITORS
    tag, remaining = first_tag, jobs
    while remaining:
        leased = []
        for auditor in auditors[: min(2, remaining)]:
            response = post_lease(rnd, lease_ask(rnd, auditor, tag))
            tag += 1
            control(response.status_code == 200, response.text)
            leased.append((auditor, response.json()["body"]))

        def execute(auditor: Keypair, job: dict[str, Any]) -> Any:
            return rnd.post(
                f"/worker/jobs/{job['job_id']}/execute",
                net.signed(
                    auditor,
                    "Receipt",
                    {"w": rnd.w, "commit_hash": job["lease_nonce"], "received_round": net.now},
                ),
            )

        with ThreadPoolExecutor(max_workers=len(leased)) as pool:
            futures = [pool.submit(execute, auditor, job) for auditor, job in leased]
            executed = [future.result(timeout=LAUNCH_SECONDS) for future in futures]
        for (auditor, job), response in zip(leased, executed, strict=True):
            control(response.status_code == 200, response.text)
            verdict = response.json()["verdict"]
            control(verdict["result"] == "MATCH", str(verdict))
            completed = rnd.post(
                f"/worker/jobs/{job['job_id']}/complete",
                net.signed(auditor, "ReplayVerdict", verdict),
            )
            control(completed.status_code == 200, completed.text)
        remaining -= len(leased)


def aggregate(rnd: Round) -> Any:
    return rnd.post(f"/admin/rounds/{rnd.w}/aggregate", headers=service().admin())


def finalize_request(rnd: Round, included: Sequence[int], theta_hash: str) -> dict[str, Any]:
    """The keyword arguments of the operator's finalize POST for `included`."""
    final = Finalize(
        w=rnd.w,
        final_theta_hash_w1=theta_hash,
        included=sorted(hot(i).ss58 for i in included),
        entitlements_root="0" * 64,
    )
    return {
        "url": rnd.network.url + f"/admin/rounds/{rnd.w}/finalize",
        "json": rnd.network.signed(service().COORD, "Finalize", final),
        "headers": service().admin(),
    }


@contextmanager
def without_record(rnd: Round, kind: str, identity: str) -> Iterator[None]:
    """The store without one accepted record, put back exactly as it was."""
    with rnd.store._tx():
        row = rnd.store._db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind=? AND id=?",
            (rnd.run_id, kind, identity),
        ).fetchone()
        control(row is not None, f"no {kind} record {identity}")
        rnd.store._db.execute(
            "DELETE FROM records_v2 WHERE run_id=? AND kind=? AND id=?",
            (rnd.run_id, kind, identity),
        )
    try:
        yield
    finally:
        with rnd.store._tx():
            rnd.store._db.execute(
                "INSERT OR REPLACE INTO records_v2 VALUES(?,?,?,?)",
                (rnd.run_id, kind, identity, row[0]),
            )


@contextmanager
def screen_expired(rnd: Round, index: int) -> Iterator[None]:
    """Member `index` with the work screen it was accepted on now lapsed."""
    with rnd.store._tx():
        saved = rnd.store._db.execute(
            "SELECT screen_until FROM admissions_v2 WHERE hotkey=?", (rnd.hotkey(index),)
        ).fetchone()[0]
        rnd.store._db.execute(
            "UPDATE admissions_v2 SET screen_until=1 WHERE hotkey=?", (rnd.hotkey(index),)
        )
    try:
        yield
    finally:
        with rnd.store._tx():
            rnd.store._db.execute(
                "UPDATE admissions_v2 SET screen_until=? WHERE hotkey=?",
                (saved, rnd.hotkey(index)),
            )


def eligible(rnd: Round, index: int) -> bool:
    answer = rnd.network.client.get(rnd.network.url + "/admission/" + rnd.hotkey(index))
    control(answer.status_code == 200, answer.text)
    return bool(answer.json()["eligible"])


def next_opening(rnd: Round, members: Sequence[int]) -> Any:
    """POST a coordinator-signed opening of round w+1 for `members`, scheduled from now on.

    The opening repeats the content of round w: the master reads the applied state of round w
    before it looks at anything else of an opening for w+1, so nothing but the schedule and the
    signature has to be right for the answer to be the master's verdict on the missing state.
    """
    net = rnd.network
    body = {
        **rnd.opening.body(),
        "w": rnd.w + 1,
        "d_open": net.now + 1,
        "d_assign": net.now + 2,
        "d_commit": net.now + 30,
        "d_audit": net.now + 31,
        "d_upload": net.now + 150,
        "d_final": net.now + 200,
    }
    opening = RoundOpenV2.model_validate(body)
    control(
        [e.hotkey for e in opening.roster] == [rnd.hotkey(i) for i in members],
        "the opening names the members it was built for",
    )
    return net.client.post(
        net.url + "/admin/rounds",
        json=net.signed(service().COORD, "RoundOpenV2", opening),
        headers=service().admin(),
    )


def lease_rounds(rnd: Round) -> int:
    """The lease length of the run's audit policy, in beacon rounds."""
    policy = AuditPolicyV2.model_validate_json(
        rnd.store.objects.get(rnd.network.manifest.network.audit_policy_hash)
    )
    return int(policy.lease_rounds)


def run_out_audit_windows(rnd: Round) -> None:
    """Move the beacon past every audit window of the round: the first one, and the second one
    that a job nobody served is given before its member leaves the round (ADR 0011 part 2). On a
    base without the second window the first push settles everything and the second is skipped."""
    rnd.network.push(rnd.absolute)
    waiting = [row["absolute"] for row in rnd.lease_rows() if row["state"] == "queued"]
    if waiting:
        rnd.network.push(max(waiting))


def rotation_request(rnd: Round, member: int, target: int, *, expires: int = 10_000) -> Any:
    """The coldkey-signed rotation of `member` onto the hotkey `target`, in the shape of the
    base: the new hotkey signs its consent first where the request has a field for it (RTA-01)."""
    request = RotateRequest.model_fields
    if "new_hot_sig" in request:
        body = miner_admission.sign_rotation_consent(
            hot(target),
            run_id=rnd.run_id,
            coldkey=cold(member).ss58,
            hotkey=rnd.hotkey(member),
            operation_id="df" * 32,
            expires_beacon=expires,
        )
    else:
        body = RotateRequest.model_validate(
            {
                "hotkey": rnd.hotkey(member),
                "new_hotkey": hot(target).ss58,
                "operation_id": "df" * 32,
            }
        )
    return miner_admission.sign_rotation(cold(member), rnd.run_id, body, expires)


def walk(rnd: Round, number: int) -> None:
    """Push every beacon round up to `number`, one by one, as the real pusher does. A jump is not
    a silence: a master that reads a gap in its beacon as its own pause blames nobody for it."""
    for round_number in range(rnd.network.now + 1, number + 1):
        rnd.network.push(round_number)


@contextmanager
def silent_round(root: Path) -> Iterator[Round]:
    """Round 0 of five members: four committed and published their leaves, the fifth accepted its
    assignment and went quiet. Four members are what an aggregation needs."""
    with activated_network(root) as network:
        rnd = open_round(network, 0, [0, 1, 2, 3, 4])
        rnd.quiet = 4
        for index in (0, 1, 2, 3):
            join_forged(rnd, index)
        control(accept(rnd, rnd.quiet).status_code == 200, "the quiet member accepts")
        yield rnd


@contextmanager
def queued_round(root: Path) -> Iterator[Round]:
    """Round 0 of four members who all committed and published, audits scheduled at d_audit:
    four queued jobs, nobody has leased any."""
    with activated_network(root) as network:
        rnd = open_round(network, 0, [0, 1, 2, 3])
        for index in rnd.members:
            join_forged(rnd, index)
        response = schedule(rnd)
        control(response.status_code == 200, response.text)
        yield rnd


@contextmanager
def audited_head(root: Path) -> Iterator[tuple[Round, int]]:
    """Round 0 of four members; the one whose job is first in the queue trained for real and its
    audit says MATCH, the other three committed forged leaves and their jobs wait. Returns the
    round and the index of the audited member."""
    with activated_network(root) as network:
        rnd = open_round(network, 0, [0, 1, 2, 3])
        head = rnd.queue_order()[0]
        train(rnd, [head])
        for index in rnd.members:
            if index != head:
                join_forged(rnd, index)
        response = schedule(rnd)
        control(response.status_code == 200, response.text)
        audit_queue(rnd, 1)
        yield rnd, head


@contextmanager
def audited_round(root: Path, size: int = 5) -> Iterator[Round]:
    """Round 0 of `size` members who all trained for real and whose audits say MATCH, before its
    aggregation. Four members are what an aggregation needs; a fifth lets one of them go."""
    with activated_network(root) as network:
        rnd = open_round(network, 0, range(size))
        train(rnd, rnd.members)
        response = schedule(rnd)
        control(response.status_code == 200, response.text)
        audit_queue(rnd, size)
        yield rnd


def admission_service(rnd: Round) -> Any:
    """The master's admission service of the run: the hooks its audit side calls."""
    return rnd.store._services(rnd.run_id)[1]


def admission_record(rnd: Round, index: int) -> dict[str, Any]:
    """The public record (state, reason, strikes, trial counters) of the admission of `index`."""
    net = rnd.network
    answer = net.client.get(net.url + "/admission/" + rnd.hotkey(index))
    control(answer.status_code == 200, answer.text)
    record: dict[str, Any] = answer.json()["record"]
    return record


def standing(rnd: Round, index: int) -> tuple[str, int, int, int]:
    """(state, strikes, bond units still locked, units burned) of the admission of member `index`,
    read from the public admission route and the owner's balances."""
    record = admission_record(rnd, index)
    balances = rnd.store._services(rnd.run_id)[0].balances(cold(index).ss58)
    return (
        str(record["state"]),
        int(record["strikes"]),
        balances.admission_locked,
        balances.burned,
    )


def faults_booked(rnd: Round) -> list[str]:
    """Every fault the books of the run hold against a member of round `rnd.w`: an exclusion whose
    reason is not the infrastructure's, an exit sanction that blames a miner (a base without an
    exit reaper has no such row), and a sanction the admission service applied. Empty when nobody
    is at fault, whichever way the exit records the members it gives up."""
    found = [
        f"excluded {hotkey}: {reason}"
        for hotkey, reason in rnd.excluded().items()
        if reason != "INFRASTRUCTURE"
    ]
    found += [
        f"exit-sanction {row['id']}"
        for row in rnd.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE run_id=? AND kind='exit-sanction'",
            (rnd.run_id,),
        )
        if json.loads(row["data"])["blame"] == "MINER"
    ]
    found += [
        f"sanction {reason} {admission_id}"
        for admission_id, reason in admission_service(rnd).store.db.execute(
            "SELECT admission_id,reason FROM admission_sanctions"
        )
    ]
    return found


def seize(rnd: Round, index: int, reason: str, evidence: bytes) -> int:
    """What the audit side asks of the admission side when it has a fault to book: sanction the
    admission of member `index` and burn its bond. Returns the units burned."""
    admission = admission_service(rnd)
    identity = admission.store.record(rnd.hotkey(index)).admission_id
    burned: int = admission.seize_bond(
        identity, reason=reason, evidence_hash=sha256_hex(evidence), now=rnd.network.now
    )
    return burned


def back_on_a_roster(rnd: Round, index: int) -> None:
    """Member `index` can be listed in a round again: its state is set to ACTIVE and nothing else
    changes. Its bond, its strikes and its trial counters stay as the sanction left them, so this
    is not a state the trials of a probation reach (the 12-trial ladder has its own e2e)."""
    with rnd.store._tx():
        rnd.store._db.execute(
            "UPDATE admissions_v2 SET state='ACTIVE' WHERE hotkey=?", (rnd.hotkey(index),)
        )


def screen_rounds(rnd: Round) -> int:
    """The length of a work screen of the run's admission policy, in beacon rounds."""
    return int(admission_service(rnd).policy.work_screen_epoch_rounds)


def lose_leaves(rnd: Round, index: int) -> None:
    """The master's object store loses the leaf object member `index` published; the record of
    the publication, which the master wrote when it answered 200, stays."""
    key = rnd.record("leaves", rnd.identity(index))["object"]
    path = rnd.store.objects._path(key)
    control(path.is_file(), "the leaf object is in the store before it is lost")
    path.unlink()


def post_lease_quietly(rnd: Round, raw: bytes) -> Any:
    """POST /worker/lease, answering 500 for a fault that escapes the route as a server does
    (the test client of the fixtures re-raises it instead)."""
    client = TestClient(rnd.network.client.app, raise_server_exceptions=False)
    return client.post(rnd.network.url + "/worker/lease", content=raw)


def contest(rnd: Round, index: int) -> Any:
    """Member `index` funds a contest bond and contests its own audit verdict; returns the
    response of POST /dispute unchecked."""
    net = rnd.network
    verdicts = [
        (row["id"], json.loads(row["data"]))
        for row in rnd.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE run_id=? AND kind='contest'", (rnd.run_id,)
        )
    ]
    ours = [verdict for verdict, data in verdicts if data["miner"] == rnd.hotkey(index)]
    control(len(ours) == 1, "the audit of the member left one verdict it can contest")
    dispute_id = sha256_hex(canonicalize([rnd.run_id, ours[0], rnd.hotkey(index)]))
    origin = rnd.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (cold(index).ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id="ce" * 32,
        owner=cold(index).ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    funded = rnd.post("/escrow/lock", net.signed(cold(index), "EscrowLock", lock))
    control(funded.status_code == 200, funded.text)
    body = DisputeV2(
        hotkey=rnd.hotkey(index),
        verdict_hash=ours[0],
        action="contest",
        bond_lock=lock.operation_id,
    )
    return rnd.post("/dispute", net.signed(hot(index), "DisputeV2", body))


def disputes_opened(rnd: Round) -> int:
    return int(rnd.store._db.execute("SELECT COUNT(*) FROM disputes_v2").fetchone()[0])


def outage(rnd: Round, until: int) -> None:
    """The beacon pusher is down while the wall clock reaches the time of round `until`."""
    genesis = rnd.network.manifest.training.beacon.genesis_time
    rnd.store.clock = lambda: genesis + (until - 1) * 3


class Chain:
    """A two-round run, advanced one phase at a time and only as far as a test needs.

    Round 0 has six members: five train and one commits and goes quiet without publishing its
    leaves, whose job sorts last in the queue. The five audits complete; the quiet member's job
    cannot be leased, its commit is sealed to d_final as an honest miner seals it, so the job gets
    no second window and its member leaves the round when the beacon reaches d_final. The audits
    are leased as late as a lease can be, so that their verdicts expire at that same horizon: the
    operator settles round 0 at the horizon, the way a round with a lost member is settled when it
    is on time (a finalize after the verdicts expired is pinned by test_expiry.py, not here).
    Round 1 is opened on four of the five survivors (the quiet member has no anchor to start from).
    The tests are ordered by phase: a test of an earlier phase reads a state a later phase has
    consumed.
    """

    def __init__(self, network: Any):
        self.network = network
        self._r0: Round | None = None
        self._theta0: str | None = None
        self.tape0_inputs: list[str] = []
        self._finalize0: Any = None
        self._r1: Round | None = None
        self._theta1: str | None = None

    @property
    def r0(self) -> Round:
        """Round 0 at the horizon of its audit jobs: five audited, the quiet member left out."""
        if self._r0 is None:
            rnd = open_round(self.network, 0, range(MINERS))
            rnd.quiet = rnd.queue_order()[-1]
            join_forged(rnd, rnd.quiet, leaves=False, commit_expiry=rnd.opening.d_final)
            train(rnd, [i for i in rnd.members if i != rnd.quiet])
            response = schedule(rnd)
            control(response.status_code == 200, response.text)
            self.network.push(rnd.absolute - lease_rounds(rnd))
            audit_queue(rnd, MINERS - 1)
            run_out_audit_windows(rnd)
            self._r0 = rnd
        return self._r0

    @property
    def survivors(self) -> list[int]:
        return [i for i in self.r0.members if i != self.r0.quiet]

    def applied0(self) -> tuple[Round, str]:
        """Round 0 aggregated; its model state hash."""
        if self._theta0 is None:
            response = aggregate(self.r0)
            control(response.status_code == 200, response.text)
            self._theta0 = str(response.json()["theta_hash"])
            self.tape0_inputs = sorted(
                item["roster"]["hotkey"] for item in response.json()["tape"]["body"]["inputs"]
            )
        return self.r0, self._theta0

    def settled0(self) -> Any:
        """The answer to the operator's Finalize of round 0 for the survivors, posted once at the
        horizon (round 1 opens after it, and a later beacon would expire the audit verdicts)."""
        if self._finalize0 is None:
            rnd, theta = self.applied0()
            self._finalize0 = self.network.client.post(
                **finalize_request(rnd, self.survivors, theta)
            )
        return self._finalize0

    def applied1(self) -> tuple[Round, str]:
        """Round 1 (four survivors) opened, trained, audited and aggregated."""
        if self._theta1 is None:
            self.settled0()
            rnd = open_round(self.network, 1, self.survivors[:4])
            train(rnd, rnd.members)
            response = schedule(rnd)
            control(response.status_code == 200, response.text)
            audit_queue(rnd, len(rnd.members), first_tag=50)
            response = aggregate(rnd)
            control(response.status_code == 200, response.text)
            self._r1, self._theta1 = rnd, str(response.json()["theta_hash"])
        assert self._r1 is not None
        return self._r1, self._theta1
