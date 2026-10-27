"""Helpers private to the expiry package: a funded ACTIVE roster and a round driven over HTTP.

Nothing here trains a model. A commit carries the hashes of a leaf set the test builds itself:
the store verifies those at publication and at audit, and the intake and scheduling paths the
expiry tests exercise never open the delta. Not collected: no test_ prefix.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import LeafPreimage, StageState, f32hex
from hypertrain.protocol.messages_v2 import (
    AcceptV2,
    ArtifactRef,
    CommitV2,
    EscrowLock,
    PolicyHashes,
    RankWorkResult,
    RoundOpenV2,
    StartStateV2,
    WorkProof,
    WorkScreenV2,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

from .support import World, load_fixture_module

SERVICE = ("li_service_fixture", "challenge/test_service_network_v2.py")


class ControlFailed(Exception):
    """The scenario no longer reaches the point a test is about: not an outcome of that test."""


def control(condition: bool, what: str) -> None:
    if not condition:
        raise ControlFailed(what)


def service() -> Any:
    return load_fixture_module(*SERVICE)


def hot(index: int) -> Keypair:
    keys: list[Keypair] = service().HOT
    return keys[index]


def cold(index: int) -> Keypair:
    keys: list[Keypair] = service().COLD
    return keys[index]


def activate(world: World, count: int) -> None:
    """`count` identities ACTIVE by SQL after a real join and a real funded lock.

    The 12-trial ladder has its own e2e; what an expiry test needs is a roster the store
    accepts, which is a funded, ACTIVE admission with a current screen.
    """
    network = world.network
    spec = network.manifest.training.reference_spec
    for i in range(count):
        if i % 2 == 0 and i:
            network.push(network.now + 101)
        response = network.join(i)
        control(response.status_code == 200, f"join {i}: {response.text}")
        identity = response.json()["admission_id"]
        lock = EscrowLock(
            operation_id=f"{i + 1:064x}",
            owner=cold(i).ss58,
            units=1000,
            origin_ids=[network.origin_ids[i]],
            admission_id=identity,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        response = network.client.post(
            network.url + "/escrow/lock", json=network.signed(cold(i), "EscrowLock", lock)
        )
        control(response.status_code == 200, f"lock {i}: {response.text}")
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
        with world.store._tx():
            world.store._db.execute(
                "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,screen_json=?,"
                "screen_until=1000000000 WHERE admission_id=?",
                (screen.model_dump_json(), identity),
            )


@dataclass
class Round:
    """Round `w` of a hosted run, opened for `members` with deadlines relative to `base`."""

    world: World
    members: list[int]
    w: int
    base: int
    offsets: tuple[int, int, int, int, int, int]
    opening: RoundOpenV2
    starts: dict[str, StartStateV2] = field(default_factory=dict)
    commits: dict[str, CommitV2] = field(default_factory=dict)

    @property
    def d_assign(self) -> int:
        return self.base + self.offsets[1]

    @property
    def d_audit(self) -> int:
        return self.base + self.offsets[3]

    @property
    def d_final(self) -> int:
        return self.base + self.offsets[5]


def round_opening(
    world: World, members: list[int], offsets: tuple[int, int, int, int, int, int], w: int = 0
) -> tuple[Round, dict[str, Any]]:
    """The coordinator's RoundOpenV2 of round `w` for ACTIVE `members`, not yet posted."""
    network = world.network
    manifest = network.manifest
    base = network.now
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    cache = AnchorCache()
    admission = network.store._services(manifest.run_id())[1]
    starts: dict[str, StartStateV2] = {}
    roster = []
    for slot, index in enumerate(members):
        key = hot(index)
        record = admission.status(key.ss58, now=base).record
        anchor = cache.genesis(manifest, key.ss58, theta)
        starts[key.ss58] = StartStateV2(
            run_id=manifest.run_id(),
            w=w,
            hotkey=key.ss58,
            theta_hash=state_hash(theta),
            state_object_sha256=sha256_hex(pack_state(theta, anchor.state)),
            opt_state_hash=optimizer_hash(anchor.state),
            ef_object_sha256=sha256_hex(pack_state(anchor.ef)),
            ef_hash=state_hash(anchor.ef),
            parent_anchor_hash=anchor.anchor_hash,
            global_step0=anchor.state.step,
            anchor_verdict_hash=anchor.proof_hash,
        )
        roster.append(
            {
                "hotkey": key.ss58,
                "slot": slot,
                "q_i": f32hex(1),
                "admission_id": record.admission_id,
                "coldkey_group": record.coldkey,
                "state": "ACTIVE",
                "eligible_weight": 4194304,
            }
        )
    d = [base + offset for offset in offsets]
    opening = RoundOpenV2(
        w=w,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=d[0],
        d_assign=d[1],
        d_commit=d[2],
        d_audit=d[3],
        d_upload=d[4],
        d_final=d[5],
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(manifest.network, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts.values()])),
        audit_mode="anchored-full",
        roster=roster,
    )
    rnd = Round(world, members, w, base, offsets, opening, starts)
    envelope = network.signed(service().COORD, "RoundOpenV2", opening)
    return rnd, envelope


def open_round(
    world: World, members: list[int], offsets: tuple[int, int, int, int, int, int], w: int = 0
) -> tuple[Round, Any]:
    rnd, envelope = round_opening(world, members, offsets, w)
    response = world.network.client.post(
        world.network.url + "/admin/rounds", json=envelope, headers=world.admin
    )
    return rnd, response


def assignment_hash(rnd: Round, index: int) -> str:
    response = rnd.world.network.client.get(rnd.world.network.url + f"/rounds/{rnd.w}")
    control(response.status_code == 200, f"round view: {response.text}")
    for entry in response.json()["assignment"]:
        if entry["hotkey"] == hot(index).ss58:
            return str(entry["assignment_hash"])
    raise ControlFailed("member has no assignment")


def accept_body(rnd: Round, index: int) -> AcceptV2:
    manifest = rnd.world.network.manifest
    screen = rnd.world.store._db.execute(
        "SELECT screen_json FROM admissions_v2 WHERE hotkey=?", (hot(index).ss58,)
    ).fetchone()[0]
    return AcceptV2(
        w=rnd.w,
        hotkey=hot(index).ss58,
        assignment_hash=assignment_hash(rnd, index),
        image_digest=manifest.training.reference_spec.image_digest,
        driver_version=manifest.training.reference_spec.driver_allowlist[0],
        n_gpus=1,
        work_screen_hash=sha256_hex(canonicalize(json.loads(screen))),
    )


def leaf_set(rnd: Round, index: int) -> list[LeafPreimage]:
    """Leaf preimages the store accepts for the member's commit: shape only, no training."""
    manifest = rnd.world.network.manifest
    inner = manifest.training.inner
    seed = sha256_hex(f"leaf|{hot(index).ss58}".encode())
    return [
        LeafPreimage(
            run_id=manifest.run_id(),
            w=rnd.w,
            t=t,
            stages=[StageState(theta=seed, m=seed, v=seed)],
            batch_ids_sha256=seed,
            rng_ctr=t,
            loss_f32=f32hex(1),
            norm_f32=f32hex(1),
        )
        for t in range(0, inner.H + 1, inner.J)
    ]


def commit_body(rnd: Round, index: int) -> CommitV2:
    manifest = rnd.world.network.manifest
    leaves = leaf_set(rnd, index)
    root = MerkleTree([bytes.fromhex(p.digest()) for p in leaves]).root.hex()
    return CommitV2(
        w=rnd.w,
        hotkey=hot(index).ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=len(leaves),
        leaves_root=root,
        metrics_root=sha256_hex(b"metrics"),
        final_theta_hash=sha256_hex(b"theta"),
        ef_in_hash=rnd.starts[hot(index).ss58].ef_hash,
        ef_out_hash=sha256_hex(b"ef-out"),
        delta_hash=sha256_hex(b"delta"),
        delta_bytes=1,
        tokens=manifest.training.batch_samples() * manifest.training.model.seq_len,
    )


def post_accept(rnd: Round, index: int) -> Any:
    network = rnd.world.network
    return network.client.post(
        network.url + "/accept",
        json=network.signed(hot(index), "AcceptV2", accept_body(rnd, index)),
    )


def post_commit(rnd: Round, index: int, expiry: int, *, accepted: bool = False) -> Any:
    """Accept (unless `accepted`) then commit for `index`, the commit expiring at `expiry`."""
    network = rnd.world.network
    if not accepted:
        response = post_accept(rnd, index)
        control(response.status_code == 200, f"accept {index}: {response.text}")
    commit = commit_body(rnd, index)
    rnd.commits[hot(index).ss58] = commit
    return network.client.post(
        network.url + "/commit",
        json=network.signed(hot(index), "CommitV2", commit, expiry=expiry),
    )


def publish_leaves(rnd: Round, index: int) -> Any:
    network = rnd.world.network
    commit = rnd.commits[hot(index).ss58]
    raw = canonicalize([p.model_dump(mode="json") for p in leaf_set(rnd, index)])
    reference = ArtifactRef(sha256=network.store.objects.put(raw), size=len(raw))
    proof = WorkProof(
        admission_id=network.store._services(network.manifest.run_id())[1]
        .status(hot(index).ss58, now=network.now)
        .record.admission_id,
        challenge_hash=sha256_hex(canonicalize(commit.model_dump(mode="json"))),
        leaves_root=commit.leaves_root,
        delta_hash=commit.delta_hash,
        artifact_refs=[reference],
    )
    return network.client.post(
        network.url + "/leaves", json=network.signed(hot(index), "WorkProof", proof)
    )


# --- one signed request per route type ---------------------------------------------------------


@dataclass(frozen=True)
class Signed:
    """A signed v2 request, ready to post: the envelope for `run_id` and the way to send it."""

    method: str
    path: str
    payload: Any
    admin: bool

    def post(
        self, world: World, *, payload: Any = None, headers: dict[str, str] | None = None
    ) -> Any:
        network = world.network
        kwargs: dict[str, Any] = {"json": self.payload if payload is None else payload}
        merged = dict(world.admin if self.admin else {})
        merged.update(headers or {})
        if merged:
            kwargs["headers"] = merged
        return network.client.request(self.method, network.url + self.path, **kwargs)


def foreign_run_id(world: World) -> str:
    """The id of the run whose manifest differs from the hosted one by its beacon genesis."""
    from hypertrain.protocol.messages_v2 import RunManifestV2

    body = world.network.manifest.body()
    body["training"]["beacon"]["genesis_time"] += 1
    other = RunManifestV2.model_validate(body).run_id()
    control(other != world.run_id, "the foreign manifest must be another run")
    return str(other)


def signed_requests(
    world: World,
    rnd: Round,
    run_id: str,
    expiry: int = 10_000,
    job: dict[str, Any] | None = None,
) -> dict[str, Signed]:
    """One correctly signed request per route type, every envelope sealed for `run_id`.

    The bodies are well formed and the signers are the roles each route expects, so the only
    thing that differs from a request of the hosted run is the run the signature names. `job`
    is a live lease (the body of a leased AuditJobV2): execute and complete look the lease up
    before they read the envelope, so without it they never reach the signature.
    """
    from hypertrain.ledger.escrow_v2 import authority_message, genesis_origin_id
    from hypertrain.protocol.envelope_v2 import seal
    from hypertrain.protocol.messages import Finalize, Receipt, ReplayEnv, ReplayVerdict, StateServe
    from hypertrain.protocol.messages_v2 import (
        BisectV2,
        DeltaManifestV2,
        DisputeV2,
        EscrowRelease,
        EscrowTransfer,
        OriginAllocation,
        ResolutionV2,
        RotateRequest,
        TestGenesis,
    )

    fixture = service()
    network = world.network
    spec = network.manifest.training.reference_spec
    h0, c0, c1 = hot(0), cold(0), cold(1)
    coord = fixture.COORD
    auditor = fixture.AUDITORS[0]
    admission = network.store._services(world.run_id)[1].status(h0.ss58, now=network.now)
    one, two, three = "11" * 32, "22" * 32, "33" * 32

    def sealed(key: Keypair, kind: str, body: Any) -> dict[str, Any]:
        envelope: dict[str, Any] = seal(key, kind, run_id, body, expiry)
        return envelope

    lock = EscrowLock(
        operation_id=one,
        owner=c0.ss58,
        units=1,
        origin_ids=[network.origin_ids[0]],
        admission_id=admission.record.admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    transfer = EscrowTransfer(
        operation_id=two,
        owner=c0.ss58,
        units=1,
        origin_ids=[network.origin_ids[0]],
        admission_id=None,
        dispute_id=None,
        recipient=c1.ss58,
    )
    release = EscrowRelease(
        operation_id=three,
        owner=c0.ss58,
        units=1,
        origin_ids=[network.origin_ids[0]],
        admission_id=None,
        dispute_id=None,
        lock_id=one,
        finality_hash=two,
        closed_dispute_root=three,
    )
    rotate = RotateRequest(hotkey=h0.ss58, new_hotkey=fixture.REFEREE.ss58, operation_id=two)
    dispute = DisputeV2(hotkey=h0.ss58, verdict_hash=one, action="accept", bond_lock=None)
    bisect = BisectV2(
        dispute_id=one,
        seq=0,
        level="step",
        ctx=[],
        interval=(0, 2),
        N=2,
        hashes=[one, two, three],
        party=h0.ss58,
        previous_transcript_hash=one,
    )
    resolution = ResolutionV2(
        dispute_id=one, transcript_hash=two, reason="MATCH", loser=None, evidence_hash=three
    )
    serve = StateServe(
        hotkey=h0.ss58,
        challenge_hash=one,
        t=0,
        uri="sha256:" + one,
        tensor_root=two,
        merkle_proof_leaf_in_leaves_root=[],
    )
    receipt = Receipt(w=rnd.w, commit_hash=one, received_round=1)
    job_id = one if job is None else str(job["job_id"])
    held = (
        receipt
        if job is None
        else Receipt(w=rnd.w, commit_hash=str(job["lease_nonce"]), received_round=1)
    )
    verdict = ReplayVerdict(
        challenge_hash=one,
        first_bad_leaf=None,
        result="MATCH",
        recomputed_leaves_root=two,
        replay_env=ReplayEnv(
            image_digest=spec.image_digest, driver="cpu", gpu_uuid_sha256=three, sm_count=1
        ),
    )
    commit = commit_body(rnd, 0)
    delta = DeltaManifestV2(
        w=rnd.w,
        hotkey=h0.ss58,
        delta_hash=one,
        uri="sha256:" + one,
        size=1,
        format="ht-sparse-v1",
        chunks=[{"off": 0, "len": 1, "sha256": one}],
        grant_hash=two,
        master_acceptance_hash=three,
    )
    proof = WorkProof(
        admission_id=admission.record.admission_id,
        challenge_hash=one,
        leaves_root=commit.leaves_root,
        delta_hash=commit.delta_hash,
        artifact_refs=[ArtifactRef(sha256=one, size=1)],
    )
    screen = WorkScreenV2.model_validate_json(
        world.store._db.execute(
            "SELECT screen_json FROM admissions_v2 WHERE hotkey=?", (h0.ss58,)
        ).fetchone()[0]
    )
    final = Finalize(w=rnd.w, final_theta_hash_w1=one, included=[h0.ss58], entitlements_root=two)
    allocation = [(c.ss58, 10_000, 1) for c in (cold(i) for i in range(4))]
    genesis_hash = fixture.genesis_allocation_hash(allocation)
    unsigned = TestGenesis(
        run_id=run_id,
        allocation_hash=genesis_hash,
        total_units=40_000,
        origins=[
            OriginAllocation(
                origin_id=genesis_origin_id(run_id, genesis_hash, i),
                owner=owner,
                units=units,
                mature_at=at,
            )
            for i, (owner, units, at) in enumerate(allocation)
        ],
        authority_sig="0" * 128,
    )
    genesis = unsigned.model_copy(
        update={"authority_sig": coord.sign(authority_message(unsigned)).hex()}
    )
    base = "/admission/"
    routes: dict[str, tuple[str, str, Any, bool]] = {
        "lock": ("POST", "/escrow/lock", sealed(c0, "EscrowLock", lock), False),
        "transfer": ("POST", "/escrow/transfer", sealed(c0, "EscrowTransfer", transfer), False),
        "release": ("POST", "/escrow/release", sealed(c0, "EscrowRelease", release), False),
        "rotate": (
            "POST",
            base + h0.ss58 + "/rotate",
            sealed(c0, "RotateRequest", rotate),
            False,
        ),
        "recover": ("POST", base + "recover", sealed(h0, "Receipt", receipt), False),
        "dispute": ("POST", "/dispute", sealed(h0, "DisputeV2", dispute), False),
        "bisect": ("POST", "/bisect", sealed(h0, "BisectV2", bisect), False),
        "resolution": (
            "POST",
            "/resolution",
            sealed(fixture.REFEREE, "ResolutionV2", resolution),
            False,
        ),
        "state-serve": ("POST", "/state-serve", sealed(h0, "StateServe", serve), False),
        "accept": (
            "POST",
            "/accept",
            sealed(h0, "AcceptV2", accept_body(rnd, 0)),
            False,
        ),
        "commit": ("POST", "/commit", sealed(h0, "CommitV2", commit), False),
        "delta": ("POST", "/delta", sealed(h0, "DeltaManifestV2", delta), False),
        "leaves": ("POST", "/leaves", sealed(h0, "WorkProof", proof), False),
        "join-proof": (
            "POST",
            "/join/proof",
            {
                "proof": sealed(h0, "WorkProof", proof),
                "screen": sealed(h0, "WorkScreenV2", screen),
            },
            False,
        ),
        "lease": ("POST", "/worker/lease", sealed(auditor, "Receipt", receipt), False),
        "execute": (
            "POST",
            "/worker/jobs/" + job_id + "/execute",
            sealed(auditor, "Receipt", held),
            False,
        ),
        "complete": (
            "POST",
            "/worker/jobs/" + job_id + "/complete",
            sealed(auditor, "ReplayVerdict", verdict),
            False,
        ),
        "genesis": ("POST", "/admin/genesis", sealed(coord, "TestGenesis", genesis), True),
        "open": ("POST", "/admin/rounds", sealed(coord, "RoundOpenV2", rnd.opening), True),
        "finalize": (
            "POST",
            f"/admin/rounds/{rnd.w}/finalize",
            sealed(coord, "Finalize", final),
            True,
        ),
    }
    return {name: Signed(*spec_) for name, spec_ in routes.items()}


# --- states of the clock and of the store ------------------------------------------------------


def stale(world: World, rounds: int) -> None:
    """Move the wall clock `rounds` drand rounds past the latest pushed beacon."""
    genesis = world.network.manifest.training.beacon.genesis_time
    world.store.clock = lambda: genesis + (world.network.now - 1 + rounds) * 3


def tables(world: World) -> dict[str, str]:
    """Per-table digest of every store of the world: which tables a request touched."""
    import hashlib

    from .support import table_names

    digests: dict[str, str] = {}
    for index, store in enumerate(world.store.stores()):
        with store._lock:
            for name in table_names(store._db):
                rows = sorted(repr(tuple(r)) for r in store._db.execute(f'SELECT * FROM "{name}"'))
                digests[f"{index}:{name}"] = hashlib.sha256(repr(rows).encode()).hexdigest()
    return digests


def touched(before: dict[str, str], after: dict[str, str]) -> list[str]:
    return sorted(
        name for name in before.keys() | after.keys() if before.get(name) != after.get(name)
    )


def files(world: World) -> list[str]:
    root = world.store.state_dir
    return sorted(str(p.relative_to(root)) for p in root.rglob("*") if p.is_file())


def job_id_of(world: World, w: int, index: int) -> str:
    """The audit job id the store derives for a member (store.schedule_audits_v2)."""
    return sha256_hex(f"ht-audit-job/2|{world.run_id}|{w}|{hot(index).ss58}".encode())


def lease_request(world: World, auditor: Keypair, n: int = 1, w: int = 0) -> Any:
    network = world.network
    return network.client.post(
        network.url + "/worker/lease",
        json=network.signed(
            auditor,
            "Receipt",
            {"w": w, "commit_hash": f"{n:064x}", "received_round": network.now},
        ),
    )


def last_audit_beacon(world: World, w: int = 0) -> int:
    """The last beacon at which the store's own schedule still lets a lease be granted."""
    with world.store._lock:
        row = world.store._db.execute(
            "SELECT MIN(absolute) FROM audit_leases_v2 WHERE run_id=? AND w=?", (world.run_id, w)
        ).fetchone()
    return int(row[0]) - 1


def audits_request(world: World, w: int = 0) -> Any:
    return world.network.client.post(
        world.network.url + f"/admin/rounds/{w}/audits", headers=world.admin
    )


def commit_all(rnd: Round, expiries: dict[int, int] | None = None, default: int = 10_000) -> None:
    """Accept, commit and publish leaves for every member, at the beacon d_assign."""
    rnd.world.network.push(rnd.d_assign)
    for index in rnd.members:
        committed = post_commit(rnd, index, (expiries or {}).get(index, default))
        control(committed.status_code == 200, f"commit {index}: {committed.text}")
        published = publish_leaves(rnd, index)
        control(published.status_code == 200, f"leaves {index}: {published.text}")


OFFSETS = (1, 2, 10, 11, 12, 40)  # d_open, d_assign, d_commit, d_audit, d_upload, d_final


def audited(
    world: World, offsets: tuple[int, int, int, int, int, int] = OFFSETS, early: int | None = None
) -> Round:
    """Four ACTIVE members, an open round, every commit and leaf set in, audits scheduled.

    Member `early` signs a commit that expires one round before d_audit: valid when accepted,
    expired when the audit job embeds it.
    """
    activate(world, 4)
    rnd, opened = open_round(world, [0, 1, 2, 3], offsets)
    control(opened.status_code == 200, f"open: {opened.text}")
    expiries = {} if early is None else {early: rnd.d_audit - 1}
    commit_all(rnd, expiries)
    world.network.push(rnd.d_audit)
    scheduled = audits_request(world)
    control(scheduled.status_code == 200, f"audits: {scheduled.text}")
    expected = sorted(job_id_of(world, rnd.w, i) for i in rnd.members)
    control(sorted(scheduled.json()["jobs"]) == expected, "the job ids are not the derived ones")
    return rnd


def prepared_write(world: World, route: str) -> Callable[[], Any]:
    """The pending write of `route` with every step before it done; calling it sends it."""
    network = world.network
    activate(world, 4)
    rnd, opening = round_opening(world, [0, 1, 2, 3], OFFSETS)

    def post(path: str, **kwargs: Any) -> Callable[[], Any]:
        return lambda: network.client.post(network.url + path, **kwargs)

    if route == "open":
        return post("/admin/rounds", json=opening, headers=world.admin)
    opened = network.client.post(network.url + "/admin/rounds", json=opening, headers=world.admin)
    control(opened.status_code == 200, f"open: {opened.text}")
    if route == "aggregate":
        return post("/admin/rounds/0/aggregate", headers=world.admin)
    network.push(rnd.d_assign)
    if route == "accept":
        body = accept_body(rnd, 0)
        return post("/accept", json=network.signed(hot(0), "AcceptV2", body))
    accepted = post_accept(rnd, 0)
    control(accepted.status_code == 200, f"accept: {accepted.text}")
    if route == "commit":
        commit = commit_body(rnd, 0)
        return post("/commit", json=network.signed(hot(0), "CommitV2", commit))
    for index in (1, 2, 3):
        control(post_commit(rnd, index, 10_000).status_code == 200, f"commit {index}")
    control(post_commit(rnd, 0, 10_000, accepted=True).status_code == 200, "commit 0")
    for index in rnd.members:
        control(publish_leaves(rnd, index).status_code == 200, f"leaves {index}")
    network.push(rnd.d_audit)
    if route == "audits":
        return post("/admin/rounds/0/audits", headers=world.admin)
    control(audits_request(world).status_code == 200, "audits")
    auditor = service().AUDITORS[0]
    receipt = {"w": 0, "commit_hash": f"{1:064x}", "received_round": network.now}
    return post("/worker/lease", json=network.signed(auditor, "Receipt", receipt))
