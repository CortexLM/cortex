"""Builders private to the finality gap tests (test_gaps_finality*.py). Not collected.

Everything here is a world or a stand-in for a state a scenario needs; the claims live in the
tests.
"""

from __future__ import annotations

import json
from collections import Counter
from dataclasses import replace
from pathlib import Path
from types import ModuleType
from typing import Any

import httpx

from hypertrain.aggregator.tape_v2 import VerifiedInput, make_tape, replay_tape
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger.escrow_v2 import FinalityEvidence
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, LeafPreimage
from hypertrain.protocol.messages_v2 import CommitV2, WorkProof
from hypertrain.protocol.relay_messages import MasterAcceptanceV2

from . import support_exit_roster as X
from . import support_expiry as E
from .support import Real, load_fixture_module

WEIGHTED_FIXTURE = ("li_weighted", "aggregator/test_weighted_v2.py")
ZERO = "0" * 64
ISSUED = 1_000_000


# -- T-FIN-01: the signer gate of a Finalize --------------------------------------------------


def finalize_post(network: Any, signer: Keypair, w: int = 0) -> httpx.Response:
    """POST a syntactically valid Finalize for round `w` signed by `signer` on the admin route.

    Nothing about the body is right: no tape was applied, so what the answer shows is which
    check the master runs first.
    """
    final = Finalize(
        w=w,
        final_theta_hash_w1=sha256_hex(b"a theta nobody applied"),
        included=[X.hot(0).ss58],
        entitlements_root=ZERO,
    )
    return network.client.post(
        f"{network.url}/admin/rounds/{w}/finalize",
        json=network.signed(signer, "Finalize", final),
        headers=X.service().admin(),
    )


def answer(response: httpx.Response) -> tuple[int, str]:
    return response.status_code, str(response.json()["detail"])


# -- T-FIN-03 / T-FIN-17: a four-miner tape input kit -----------------------------------------


class TapeKit:
    """The aggregator fixture's four verified inputs, a store and the arguments of make_tape."""

    def __init__(self, root: Path) -> None:
        self.fixture: ModuleType = load_fixture_module(*WEIGHTED_FIXTURE)
        self.store = LocalFSStore(root)
        self.manifest, self.works, self.previous = self.fixture.fixture(self.store, 4)

    def args(self, works: list[VerifiedInput] | None = None) -> dict[str, Any]:
        return {
            "w": 0,
            "prev_state": self.previous,
            "predecessor_tape_hash": self.fixture.H,
            "inputs": self.works if works is None else works,
            "reference_reward_units": 100,
        }

    def economics(self) -> bytes:
        return canonicalize(self.fixture.economics().body())

    def make(self, works: list[VerifiedInput] | None = None) -> Any:
        return make_tape(
            self.store,
            self.manifest,
            self.fixture.policy(),
            self.economics(),
            self.fixture.KEY,
            **self.args(works),
        )

    def replay(self, tape: Any) -> Any:
        return replay_tape(
            self.store,
            tape,
            self.manifest,
            self.fixture.policy(),
            self.economics(),
            signer=self.fixture.KEY.ss58,
            **self.args(),
        )

    def with_replay(self, **changes: Any) -> list[VerifiedInput]:
        """The inputs, the first one's replay evidence changed in exactly `changes`."""
        first = replace(self.works[0], replay=replace(self.works[0].replay, **changes))
        return [first, *self.works[1:]]

    def commit(self) -> CommitV2:
        return self.works[0].commit


# -- T-FIN-23: a ledger envelope sealed again to a chosen expiry -------------------------------


def resealed(li_escrow: ModuleType, real: Real, which: str, expiry: int) -> FinalityEvidence:
    """The evidence of `real` with the commit, the verdict or the coordinator's audit challenge
    sealed again, body unchanged, by its own signer to expire at `expiry`."""
    setup, evidence, _ = real
    item = evidence.works[0]
    key, kind = {
        "commit": (li_escrow.HOT, "CommitV2"),
        "verdict": (li_escrow.AUDITOR, "ReplayVerdict"),
        "challenge": (li_escrow.COORD, "AuditChallengeV2"),
    }[which]
    envelope = EnvelopeV2.model_validate(
        seal(key, kind, setup.manifest.run_id(), getattr(item, which).body, expiry)
    )
    changes: dict[str, Any] = {which: envelope}
    return replace(evidence, works=(replace(item, **changes),))


# -- T-FIN-12: intake of a commit, its leaves and its delta -------------------------------------


def leaves_root_of(leaves: list[LeafPreimage]) -> str:
    return MerkleTree([bytes.fromhex(p.digest()) for p in leaves]).root.hex()


def commit_over(
    rnd: X.Round, index: int, leaves: list[LeafPreimage] | None = None, **changes: Any
) -> CommitV2:
    """The forged commit of member `index`, its leaves root taken from `leaves` when given."""
    commit = X.forged_commit(rnd, index)
    if leaves is not None:
        commit = commit.model_copy(update={"leaves_root": leaves_root_of(leaves)})
    return commit.model_copy(update=changes)


def leaves_post(
    rnd: X.Round,
    index: int,
    commit: CommitV2,
    *,
    leaves: list[LeafPreimage] | None = None,
    proof: dict[str, str] | None = None,
) -> httpx.Response:
    """POST /leaves for the leaf object `leaves` (the forged leaves of the member by default);
    `proof` replaces fields of the signed WorkProof (leaves_root, delta_hash, challenge_hash)."""
    net = rnd.network
    objects = X.forged_leaves(rnd, index) if leaves is None else leaves
    raw = json.dumps([p.model_dump(mode="json") for p in objects]).encode()
    admission = net.store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (rnd.hotkey(index),)
    ).fetchone()[0]
    fields = {
        "admission_id": admission,
        "challenge_hash": sha256_hex(canonicalize(commit.model_dump(mode="json"))),
        "leaves_root": commit.leaves_root,
        "delta_hash": commit.delta_hash,
        "artifact_refs": [{"sha256": net.store.objects.put(raw), "size": len(raw)}],
    }
    body = WorkProof(**(fields | (proof or {})))
    return rnd.post("/leaves", net.signed(X.hot(index), "WorkProof", body))


def delta_post(rnd: X.Round, index: int, delta: bytes) -> httpx.Response:
    """POST /delta of a delta the member committed to; the master's acceptance record that the
    relay receipt route would write is written with the same sealed type, and next to it the
    receipt it names, reduced to the fields a delta is checked against (as X.deliver_delta)."""
    net, store = rnd.network, rnd.store
    delta_hash = sha256_hex(delta)
    grant_hash = sha256_hex(b"gaps-finality-grant|" + rnd.identity(index).encode())
    acceptance = MasterAcceptanceV2(
        w=rnd.w,
        hotkey=rnd.hotkey(index),
        delta_hash=delta_hash,
        receipt_hash=sha256_hex(b"gaps-finality-receipt|" + rnd.identity(index).encode()),
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
    return rnd.post("/delta", net.signed(X.hot(index), "DeltaManifestV2", body))


def commit_with_delta(rnd: X.Round, index: int, delta: bytes) -> CommitV2:
    """A forged commit that names `delta` as its delta (the leaves stay forged)."""
    return commit_over(rnd, index, delta_hash=sha256_hex(delta), delta_bytes=len(delta))


# -- T-FIN-21 / T-FIN-26: the audit queue of a cheap world --------------------------------------


def record_deltas(rnd: E.Round) -> None:
    """The delta manifest of every member of `rnd`, stored as a delivered upload is. The expiry
    world commits to a delta nobody can serve, and a member that committed and delivered nothing
    by d_upload has left the round (ADR 0011 Part 8), so a round that has to stay open past
    d_upload needs these records, written as tests/challenge/audit_lease_fixture_v2 writes them."""
    network, store = rnd.world.network, rnd.world.store
    for index in rnd.members:
        key = E.hot(index)
        commit = rnd.commits[key.ss58]
        manifest = {
            "w": rnd.w,
            "hotkey": key.ss58,
            "delta_hash": commit.delta_hash,
            "size": commit.delta_bytes,
            "uri": "sha256:" + commit.delta_hash,
            "format": "ht-sparse-v1",
            "chunks": [{"off": 0, "len": commit.delta_bytes, "sha256": commit.delta_hash}],
            "grant_hash": ZERO,
            "master_acceptance_hash": ZERO,
        }
        with store._tx():
            store._put_record_v2(
                rnd.world.run_id,
                "delta",
                f"{rnd.w}:{key.ss58}",
                network.signed(key, "DeltaManifestV2", manifest),
            )


def complete_job(rnd: X.Round, job_id: str) -> None:
    """What the lease logic reads of a finished audit: its job no longer runs, so its auditor
    holds no lease. A forged member has no verdict to sign and the tests that call this do not
    look at the verdict or at the audit counter (test_exit_roster does the same)."""
    with rnd.store._tx():
        rnd.store._db.execute("UPDATE audit_leases_v2 SET state='completed' WHERE id=?", (job_id,))


# -- the staged service (support_finality.Stage): effects and stand-ins -------------------------


def credits(stage: Any) -> Counter[tuple[int, str]]:
    """(round, hotkey) -> number of reward origins issued."""
    return Counter(
        (int(row[0]), str(row[1]))
        for row in stage.rows(
            "SELECT reward_round, hotkey FROM escrow_origins WHERE reward_round IS NOT NULL"
        )
    )


def finalize_effects(stage: Any) -> list[tuple[str, str, str]]:
    return stage.records(
        "finality", "finalize", "replayed-finality", "finalize-result", "settlement"
    )


def lapse_screen(stage: Any, hotkey: str) -> None:
    """The work screen of `hotkey` has lapsed: its admission is no longer eligible (the same
    stand-in as support_exit_roster.screen_expired, without the restore)."""
    with stage.store._tx():
        stage.store._db.execute("UPDATE admissions_v2 SET screen_until=1 WHERE hotkey=?", (hotkey,))


def finalize_of(stage: Any, w: int, theta: str) -> dict[str, Any]:
    """The coordinator's Finalize of round `w` (not one of the two staged rounds) for the four
    honest miners, naming `theta` as the resulting model state."""
    final = Finalize(
        w=w,
        final_theta_hash_w1=theta,
        included=sorted(key.ss58 for key in stage.honest),
        entitlements_root=ZERO,
    )
    signed: dict[str, Any] = stage.network.signed(stage.coordinator, "Finalize", final)
    return signed
