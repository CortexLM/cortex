"""Disputes behind the real routes: who may call each one, and what its transaction commits.

A `service_world` is the v2 network fixture with the three bearers configured. The accepted
contest record is seeded where the audit would have written it; the funding, the dispute, the
answers and the resolution all go through the routes. A refusal pins the status and detail a
client sees and leaves every row of every table as it was.
"""

from __future__ import annotations

import threading
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.disputes_v2 import EventAck, signed_message
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import EscrowError
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import BisectV2, DisputeV2, ResolutionV2

from .support import enforce_mismatch_sanction, service_world
from .support_disputes import (
    AGREE,
    BOND,
    WATCH_NEEDS_SIGNATURE,
    Contested,
    contested,
    header_signed_watch_feed,
    heavy_modules,
    rollback_raw,
    seed_rounds,
    watch_feed,
    witnesses,
)


def verdict_of(response) -> tuple[int, str]:
    """Status and detail of a refusal: the pair a client sees."""
    return response.status_code, response.json()["detail"]


def stranger_key() -> Keypair:
    return Keypair(b"\x55" * 32)


def bisect_body(miner: Contested, key: Keypair, hashes: list[str] = AGREE) -> BisectV2:
    turn = miner.turn()
    return BisectV2(
        dispute_id=miner.dispute_id,
        seq=turn.seq,
        level=turn.level,
        ctx=turn.ctx,
        interval=turn.interval,
        N=2,
        hashes=hashes,
        party=key.ss58,
        previous_transcript_hash=turn.transcript_hash,
    )


def referee_resolution(miner: Contested, reason: str = "MATCH", loser: str | None = None):
    """The evidence the service registers after its own replay, and the referee's resolution."""
    turn = miner.turn()
    evidence = RefereeEvidence(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason=reason,  # type: ignore[arg-type]
        loser=loser,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="verified.actual.op",
    )
    resolution = ResolutionV2(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason=reason,  # type: ignore[arg-type]
        loser=loser,
        evidence_hash=evidence.digest(),
    )
    return evidence, resolution


def test_only_the_audited_miner_can_open_a_contest_over_the_service_route(tmp_path: Path) -> None:
    # Covers DS-01, DIA-03, DIB-16, DIB-25.
    with service_world(tmp_path) as world:
        # Given: two joined miners, each with the contest record its own audit produced
        miner, other = contested(world, 0), contested(world, 1)
        bond = miner.fund()
        small = miner.fund("c1" * 32, units=BOND - 1)
        unfunded = miner.lock("c2" * 32)
        before = world.db_digest()

        def contest(key, hotkey, verdict, lock, action="contest"):
            return miner.post(
                "/dispute",
                key,
                "DisputeV2",
                DisputeV2(
                    hotkey=hotkey.ss58,
                    verdict_hash=verdict,
                    action=action,  # type: ignore[arg-type]
                    bond_lock=None if action == "accept" else lock.operation_id,
                ),
            )

        unknown = "77" * 32
        attempts = {
            "another miner for itself": (
                contest(other.hot, other.hot, miner.verdict, bond),
                (409, "CONTEST_IDENTITY"),
            ),
            "a hotkey off the subnet": (
                contest(stranger_key(), stranger_key(), miner.verdict, bond),
                (403, "the hotkey is not registered on the subnet"),
            ),
            "another miner signing the miner's name": (
                contest(other.hot, miner.hot, miner.verdict, bond),
                (409, "signer is not authorized for message role"),
            ),
            "an accept": (
                contest(miner.hot, miner.hot, miner.verdict, bond, "accept"),
                (409, "FUNDED_CONTEST_REQUIRED"),
            ),
            "a verdict the service never recorded": (
                contest(miner.hot, miner.hot, unknown, bond),
                (409, f"missing accepted contest: {unknown}"),
            ),
            "a bond below the referee cost": (
                contest(miner.hot, miner.hot, miner.verdict, small),
                (409, "CONTEST_LOCK_BINDING_OR_COST"),
            ),
            "a bond that was never funded": (
                contest(miner.hot, miner.hot, miner.verdict, unfunded),
                (409, "UNFUNDED_CONTEST"),
            ),
        }
        # Then: each is refused as a client sees it, and the service state is as it was
        assert {name: verdict_of(r) for name, (r, _) in attempts.items()} == {
            name: expected for name, (_, expected) in attempts.items()
        }
        assert world.db_digest() == before
        assert world.count("disputes_v2") == 0
        # When the audited miner signs for itself with an adequate bond, the contest is its own
        assert miner.open(bond).status_code == 200
        assert (world.count("disputes_v2"), miner.turn().contest.miner) == (1, miner.hot.ss58)
        assert miner.turn().lock_id == bond.operation_id


def test_each_dispute_route_accepts_only_its_intended_principal(tmp_path: Path) -> None:
    # Covers DIB-25, DIA-17, DIB-12, DIB-01, DS-05.
    with service_world(tmp_path) as world:
        # Given: an open dispute of miner 0 whose turn is the miner's, and a second joined miner
        miner, other = contested(world, 0), contested(world, 1)
        service, outsider = miner.service, stranger_key()
        lock = miner.fund()
        before_escrow = world.db_digest()
        # Escrow routes need the owning coldkey: neither the hotkey nor a stranger stands in
        for key in (miner.hot, outsider):
            r = miner.post("/escrow/lock", key, "EscrowLock", miner.lock("cf" * 32))
            assert verdict_of(r) == (409, "signer is not authorized for message role")
        assert world.db_digest() == before_escrow
        assert miner.open(lock).status_code == 200
        opened = world.db_digest()
        # Bisect needs the expected party: not the auditor, not another subscriber, not a stranger
        for key, expected in (
            (service.AUDITORS[0], (409, "TURN_SEQUENCE_CONTEXT_PARTY_OR_HASH")),
            (other.hot, (409, "TURN_SEQUENCE_CONTEXT_PARTY_OR_HASH")),
            (outsider, (403, "the hotkey is not registered on the subnet")),
        ):
            r = miner.post("/bisect", key, "BisectV2", bisect_body(miner, key))
            assert verdict_of(r) == expected
        # Resolution needs the referee of this dispute: nobody else, however pinned or registered
        _, resolution = referee_resolution(miner)
        for key in (service.AUDITORS[0], miner.hot, other.hot, service.COORD, outsider):
            r = miner.post("/resolution", key, "ResolutionV2", resolution)
            assert verdict_of(r) == (409, "signer is not authorized for message role")
        # ... and even the referee needs evidence the service registered
        r = miner.post("/resolution", service.REFEREE, "ResolutionV2", resolution)
        assert verdict_of(r) == (409, "UNVERIFIED_REFEREE_EVIDENCE")
        # State serving needs a request addressed to the signer
        from hypertrain.protocol.messages import StateServe

        serve = StateServe(
            hotkey=other.hot.ss58,
            challenge_hash=miner.turn().contest.challenge_hash,
            t=1,
            uri="https://objects.invalid/checkpoint",
            tensor_root="33" * 32,
            merkle_proof_leaf_in_leaves_root=[],
        )
        assert verdict_of(miner.post("/state-serve", other.hot, "StateServe", serve)) == (
            409,
            "UNREQUESTED_STATE_SERVE",
        )
        # Nothing above moved the dispute or the bond
        assert world.db_digest() == opened
        # Operator routes need the operator bearer and no other credential
        base, did = world.network.url, miner.dispute_id
        routes = {
            f"/admin/disputes/{did}/referee": {},
            f"/admin/disputes/{did}/timeout": {"content": b"{}"},
            f"/admin/disputes/{did}/state/0": {},
            "/admin/rollback": {"content": b"{}"},
            "/admin/rollback-preview": {"content": b"{}"},
        }
        for path, extra in routes.items():
            for credential in ({}, world.worker, world.internal):
                r = world.client.post(base + path, headers=credential, **extra)
                assert verdict_of(r) == (401, "unauthorized"), path
        assert world.db_digest() == opened
        assert {
            path: world.client.post(base + path, headers=world.admin, **extra).status_code
            for path, extra in routes.items()
        } == {
            f"/admin/disputes/{did}/referee": 409,
            f"/admin/disputes/{did}/timeout": 422,
            f"/admin/disputes/{did}/state/0": 200,
            "/admin/rollback": 409,
            "/admin/rollback-preview": 409,
        }
        # The watch feed needs the party's own signature, in the body of its request
        needs_signature = (403, WATCH_NEEDS_SIGNATURE)
        assert verdict_of(watch_feed(world, miner.hot, None)) == needs_signature
        assert verdict_of(watch_feed(world, miner.hot, other.hot)) == needs_signature
        own = watch_feed(world, miner.hot, miner.hot)
        assert own.status_code == 200 and {e["party"] for e in own.json()} == {miner.hot.ss58}
        off_subnet = watch_feed(world, outsider, outsider)
        assert verdict_of(off_subnet) == (403, "the hotkey is not registered on the subnet")
        # ... and a signature in a header opens nothing: the party's own is refused as retired
        polled = world.db_digest()
        status, detail = verdict_of(header_signed_watch_feed(world, miner.hot, miner.hot))
        assert status == 405 and detail.startswith("retired: POST"), (status, detail)
        assert world.db_digest() == polled


def ack_of(miner: Contested, key: Keypair, **override) -> EventAck:
    """The EventAck `key` signs for the latest event addressed to the miner's hotkey."""
    event = miner.disputes().events(miner.hot.ss58, 0)[-1]
    fields = dict(
        run_id=miner.world.run_id,
        cursor=event.cursor,
        event_hash=event.digest(),
        party=miner.hot.ss58,
        received_beacon=miner.world.network.now,
        sig="",
    )
    fields.update(override)
    made = EventAck.model_validate(fields)
    return made.model_copy(update={"sig": key.sign(signed_message(made)).hex()})


def test_an_ack_over_the_route_binds_the_addressed_party_and_stores_one_row(tmp_path: Path) -> None:
    # Covers SG-14, DIA-18.
    with service_world(tmp_path) as world:
        # Given: miner 0's dispute is open, so an event is addressed to it; miner 1 is a subscriber
        miner, other = contested(world, 0), contested(world, 1)
        assert miner.open(miner.fund()).status_code == 200
        url = world.network.url + "/disputes/ack"
        before = world.db_digest()
        forged = ack_of(miner, miner.hot).model_copy(update={"sig": "00" * 64})
        # When the other subscriber signs the miner's ack, acks the miner's event as itself, or
        # the signature is forged
        by_other = ack_of(miner, other.hot)
        as_other = ack_of(miner, other.hot, party=other.hot.ss58)
        # Then: every one is refused and no ack row exists
        refusals = {
            name: verdict_of(world.client.post(url, json=ack.body()))
            for name, ack in (
                ("signed by another", by_other),
                ("as another", as_other),
                ("forged", forged),
            )
        }
        assert refusals == {
            "signed by another": (409, "ACK_AUTHORITY_OR_TIME"),
            "as another": (409, "ACK_UNKNOWN_EVENT"),
            "forged": (409, "ACK_AUTHORITY_OR_TIME"),
        }
        assert (world.count("dispute_acks_v2"), world.db_digest()) == (0, before)
        # The addressed party over the exact event is stored once, however often it is sent
        good = ack_of(miner, miner.hot)
        for _ in range(2):
            accepted = world.client.post(url, json=good.body())
            assert accepted.status_code == 200 and accepted.json() == {"accepted": good.cursor}
        assert world.count("dispute_acks_v2") == 1


def test_a_receipt_signed_for_another_purpose_cannot_recover_a_suspended_miner(
    tmp_path: Path,
    # Covers SG-12.
) -> None:
    with service_world(tmp_path) as world:
        # Given: miner 0 is suspended on the pending dispute D; miner 1 is a healthy member
        miner, other = contested(world, 0), contested(world, 1)
        assert miner.open(miner.fund()).status_code == 200
        service, net = miner.service, world.network
        dispute = miner.dispute_id

        def recover(key, commit_hash, w):
            receipt = {"w": w, "commit_hash": commit_hash, "received_round": net.now}
            return world.client.post(
                net.url + "/admission/recover", json=net.signed(key, "Receipt", receipt)
            )

        def state():
            row = world.store._db.execute(
                "SELECT state,pending_dispute FROM admissions_v2 WHERE hotkey=?", (miner.hot.ss58,)
            ).fetchone()
            return tuple(row)

        suspended = state()
        assert suspended == ("SUSPENDED", 1)
        before = world.db_digest()
        differs = (403, "recovery reference differs from exact current pending dispute")
        # When a receipt cites another reference, or names D but is signed by a key that has no
        # pending dispute (a member, a pinned referee) or no standing on the subnet (coordinator)
        attempts = [
            (recover(miner.hot, "ec" * 32, 0), differs),
            (recover(other.hot, dispute, 1), differs),
            (recover(service.REFEREE, dispute, 2), differs),
            (
                recover(service.COORD, dispute, 3),
                (403, "the hotkey is not registered on the subnet"),
            ),
            # D itself, from the right miner, is not enough while the dispute is open
            (recover(miner.hot, dispute, 4), (409, "PENDING_SETTLEMENT")),
        ]
        # Then: every one is refused as such and the suspension and every row stand
        assert [verdict_of(r) for r, _ in attempts] == [expected for _, expected in attempts]
        assert (state(), world.db_digest()) == (suspended, before)
        # And once the referee has closed D without fraud, the miner's own receipt for D recovers it
        evidence, resolution = referee_resolution(miner)
        miner.disputes().register_evidence(evidence)
        closed = miner.post("/resolution", service.REFEREE, "ResolutionV2", resolution)
        assert closed.status_code == 200
        assert state() == ("SUSPENDED", 1)
        assert recover(miner.hot, dispute, 5).status_code == 200
        assert state() == ("PROBATION", 0)


def test_v1_dispute_routes_never_reach_a_v2_run(tmp_path: Path) -> None:
    # Covers DIB-23.
    from hypertrain.protocol import envelope
    from hypertrain.protocol.messages import Bisect, Dispute, Resolution, StateServe

    with service_world(tmp_path) as world:
        # Given: a v2 run with an open dispute, and v1 envelopes well formed for each v1 route
        miner = contested(world, 0)
        assert miner.open(miner.fund()).status_code == 200
        hot, run = miner.hot, world.run_id
        before = world.db_digest()
        dispute = Dispute(hotkey=hot.ss58, verdict_hash=miner.verdict, action="contest")
        move = Bisect(
            dispute_id=miner.dispute_id,
            level="step",
            interval=(0, 2),
            N=2,
            hashes=AGREE,
            party=hot.ss58,
        )
        serve = StateServe(
            hotkey=hot.ss58,
            challenge_hash="22" * 32,
            t=1,
            uri="https://objects.invalid/checkpoint",
            tensor_root="33" * 32,
            merkle_proof_leaf_in_leaves_root=[],
        )
        resolution = Resolution(
            dispute_id=miner.dispute_id,
            op_spec="op",
            inputs_hash="22" * 32,
            output_hash="33" * 32,
            loser=hot.ss58,
        )
        # When each v1 route is called with them (the resolution route with the worker bearer)
        routes = [
            ("dispute", "Dispute", dispute, {}),
            ("bisect", "Bisect", move, {}),
            ("state", "StateServe", serve, {}),
            ("resolution", "Resolution", resolution, world.worker),
        ]
        # Then: each is refused for the contract version and nothing of v1 or v2 is written
        for path, kind, body, headers in routes:
            sealed = envelope.seal(hot, kind, run, body, 10_000)
            r = world.client.post(f"/v1/runs/{run}/{path}", json=sealed, headers=headers)
            assert verdict_of(r) == (409, "route and stored contract version differ"), path
        assert world.db_digest() == before
        for table in ("disputes", "bisects", "forfeits", "state_serves"):
            assert world.count(table) == 0
        assert world.count("disputes_v2") == 1


def test_the_resolution_route_decides_and_settles_the_bond_in_one_transaction(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    # Covers DIA-13, DIB-10.
) -> None:
    with service_world(tmp_path) as world:
        # Given: an open contest and the service's own FRAUD evidence against the miner
        miner = contested(world, 0)
        service, net = miner.service, world.network
        assert miner.open(miner.fund()).status_code == 200
        escrow = world.store._services(world.run_id)[0]
        evidence, resolution = referee_resolution(miner, "FRAUD", miner.hot.ss58)
        miner.disputes().register_evidence(evidence)
        signed = net.signed(service.REFEREE, "ResolutionV2", resolution, expiry=10**6)
        before = world.db_digest()

        def fail(*args, **kwargs):
            raise EscrowError("OPERATION_CONFLICT")

        monkeypatch.setattr(miner.disputes(), "settle_lock", fail)
        # When the bond settlement fails after the decision was written
        refused = world.client.post(net.url + "/resolution", json=signed)
        # Then: the decision did not survive, the bond is still locked and no record was added
        assert verdict_of(refused) == (409, "OPERATION_CONFLICT")
        assert world.db_digest() == before
        assert miner.turn().resolution is None and escrow.balances().dispute_locked == BOND
        monkeypatch.undo()
        # When the settlement works, decision, burn and recount land together (production pays
        # the referee nothing, so the whole bond burns)
        accepted = world.client.post(net.url + "/resolution", json=signed)
        assert accepted.status_code == 200, accepted.text
        balances = escrow.balances()
        assert (balances.burned, balances.dispute_locked, balances.conserved()) == (BOND, 0, True)
        assert escrow.balances(miner.cold.ss58).available == 10_000 - BOND
        assert world.store._record_v2(world.run_id, "finality", "0")["unresolved_disputes"] == 0
        assert world.store._record_v2(world.run_id, "resolution", miner.dispute_id)["body"] == (
            resolution.model_dump(mode="json")
        )
        # A replay of the same decision changes nothing
        settled = world.db_digest()
        assert world.client.post(net.url + "/resolution", json=signed).status_code == 200
        assert world.db_digest() == settled


FINAL_OP = {"paused": True, "level": "op", "ctx": [1, 0], "interval": (1, 2)}


def test_the_referee_route_is_closed_to_a_resolved_dispute_and_to_the_absolute_horizon(
    tmp_path: Path,
    # Covers DIB-12, DIB-02.
) -> None:
    with service_world(tmp_path) as world:
        # Given: a dispute the referee has closed, and a live one frozen on its final op
        done, live = contested(world, 0), contested(world, 1)
        service, net = done.service, world.network
        for item in (done, live):
            assert item.open(item.fund()).status_code == 200
        evidence, resolution = referee_resolution(done)
        done.disputes().register_evidence(evidence)
        closed = done.post("/resolution", service.REFEREE, "ResolutionV2", resolution)
        assert closed.status_code == 200
        with world.store._tx():
            live.disputes()._save(live.turn().model_copy(update=FINAL_OP))
        horizon = live.turn().absolute_deadline

        def referee(item):
            return world.client.post(
                f"{net.url}/admin/disputes/{item.dispute_id}/referee", headers=world.admin
            )

        before = world.db_digest()
        # When / Then: inside the horizon the call gets past the gate (and stops at the missing
        # round, a different refusal); a closed dispute is refused at the gate
        inside = referee(live)
        assert verdict_of(inside) == (409, "missing accepted round: 0")
        gate = (409, "referee outside unresolved absolute horizon")
        assert verdict_of(referee(done)) == gate
        # At the horizon itself the referee's signature still counts but the replay no longer does
        net.push(horizon)
        assert live.turn().resolution is None
        assert verdict_of(referee(live)) == gate
        assert world.db_digest() != before
        assert world.count("dispute_evidence_v2") == 1
        assert world.count("records_v2", "kind='referee-job'") == 0


@pytest.mark.xfail(
    strict=True,
    raises=ChallengeError,
    reason=(
        "DIB-15: a dispute closed by timeout leaves the signed 'resolution' record that a "
        "referee resolution leaves, so rollback and the ADR can use it "
        "[fix-lot: decision (product choice pending)]"
    ),
)
def test_a_timeout_leaves_the_same_durable_resolution_record_as_a_referee_resolution(
    tmp_path: Path,
    # Covers DIB-15.
) -> None:
    with service_world(tmp_path) as world:
        # Given: a contest whose miner acknowledged its turn and then fell silent
        miner = contested(world, 0)
        service, net = miner.service, world.network
        assert miner.open(miner.fund()).status_code == 200
        url = net.url + "/disputes/ack"
        assert world.client.post(url, json=ack_of(miner, miner.hot).body()).status_code == 200
        turn = miner.turn()
        net.push(turn.turn_deadline + 1)
        proofs = witnesses(world, turn, (service.COORD, service.REFEREE), net.now)
        # When the operator submits the two witnesses
        route = f"{net.url}/admin/disputes/{miner.dispute_id}/timeout"
        closed = world.client.post(
            route, content=canonicalize({"witnesses": proofs}), headers=world.admin
        )
        # Then (control): closed against the miner, bond back in full, the recount at 0
        assert closed.status_code == 200, closed.text
        resolution = miner.turn().resolution
        assert resolution is not None and resolution.reason == "PARTY_TIMEOUT"
        escrow = world.store._services(world.run_id)[0]
        assert (escrow.balances().burned, escrow.balances().dispute_locked) == (0, 0)
        assert world.store._record_v2(world.run_id, "finality", "0")["unresolved_disputes"] == 0
        # The desired defence: the same durable record a referee resolution leaves exists
        record = world.store._record_v2(world.run_id, "resolution", miner.dispute_id)
        assert record["body"] == resolution.model_dump(mode="json")


def rollback_attempt(world, raw: bytes):
    return world.client.post(
        world.network.url + "/admin/rollback", content=raw, headers=world.admin
    )


PAST_THE_GATE = (409, "missing accepted applied: 1")


@pytest.mark.parametrize(
    ("seed", "detail"),
    [
        pytest.param({"settled": (0,)}, "finalized late fraud is money-only", id="round-w-settled"),
        pytest.param(
            {"settled": (1,)}, "finalized late fraud is money-only", id="round-w1-settled"
        ),
        pytest.param(
            {"applied": False},
            "rollback requires applied closed audit/dispute work",
            id="not-applied",
        ),
        pytest.param(
            {"unresolved_audits": 1},
            "rollback requires applied closed audit/dispute work",
            id="audit-unresolved",
        ),
    ],
)
def test_a_rollback_is_refused_for_a_settled_unapplied_or_unaudited_round(
    tmp_path: Path,
    seed: dict,
    detail: str,
    # Covers DIB-19.
) -> None:
    with service_world(tmp_path) as world:
        # Given: rounds 0 and 1 in the state under test
        seed_rounds(world, **seed)
        miner = contested(world, 0)
        raw = rollback_raw(world, [miner.hot.ss58], ["dd" * 32])
        before = world.db_digest()
        # When / Then: the coordinator-signed rollback is refused and no row moves
        assert verdict_of(rollback_attempt(world, raw)) == (409, detail)
        assert world.db_digest() == before


def test_a_rollback_is_refused_while_a_dispute_of_the_round_is_unresolved(tmp_path: Path) -> None:
    # Covers DIB-19.
    with service_world(tmp_path) as world:
        # Given: applied, closed rounds 0 and 1 (control: the rollback gets past the gate)
        seed_rounds(world)
        miner = contested(world, 0)
        service = miner.service
        raw = rollback_raw(world, [miner.hot.ss58], ["dd" * 32])
        assert verdict_of(rollback_attempt(world, raw)) == PAST_THE_GATE
        # When the miner's contest is opened through the route, the recount pins round 0
        assert miner.open(miner.fund()).status_code == 200
        assert world.store._record_v2(world.run_id, "finality", "0")["unresolved_disputes"] == 1
        before = world.db_digest()
        # Then: the rollback is refused until the referee has closed the dispute
        gate = (409, "rollback requires applied closed audit/dispute work")
        assert verdict_of(rollback_attempt(world, raw)) == gate
        assert world.db_digest() == before
        evidence, resolution = referee_resolution(miner)
        miner.disputes().register_evidence(evidence)
        closed = miner.post("/resolution", service.REFEREE, "ResolutionV2", resolution)
        assert closed.status_code == 200
        assert verdict_of(rollback_attempt(world, raw)) == PAST_THE_GATE


def test_a_rollback_exclusion_needs_one_referee_signed_fraud_resolution_with_matching_causes(
    tmp_path: Path,
    # Covers DIB-20.
) -> None:
    from hypertrain.protocol.messages import Rollback

    with service_world(tmp_path) as world:
        # Given: rounds 0 and 1 applied and closed; miner 0 lost a FRAUD dispute decided by the
        # pinned referee, and miner 1's dispute of the round was decided against the auditor
        seed_rounds(world)
        store, run_id = world.store, world.run_id
        with store._lock:
            store._put_record_v2(run_id, "applied", "1", {"theta_hash": "aa" * 32})
        miner = contested(world, 0)
        service, hot = miner.service, miner.hot.ss58

        def decide(item: Contested, reason: str, loser: str | None) -> tuple[str, str]:
            opened = item.open(item.fund())
            assert opened.status_code == 200, opened.text
            evidence, resolution = referee_resolution(item, reason, loser)
            item.disputes().register_evidence(evidence)
            closed = item.post("/resolution", service.REFEREE, "ResolutionV2", resolution)
            assert closed.status_code == 200, closed.text
            return body_digest(resolution.model_dump(mode="json")), evidence.digest()

        cause, work = decide(miner, "FRAUD", hot)
        second = contested(world, 1)
        wrong, _ = decide(second, "FRAUD", service.AUDITORS[0].ss58)
        record = store._record_v2(run_id, "resolution", miner.dispute_id)
        saved = store._db.execute("SELECT body FROM dispute_evidence_v2 WHERE hash=?", (work,))
        saved = saved.fetchone()[0]

        def attempt(causes: list[str], who: str = hot) -> tuple[int, str]:
            before = world.db_digest()
            result = verdict_of(rollback_attempt(world, rollback_raw(world, [who], causes)))
            assert world.db_digest() == before
            return result

        # When / Then: a resolution that did not convict the miner, a cause that is not its own
        # or not alone, is refused ...
        assert attempt([wrong], second.hot.ss58) == (
            409,
            "rollback cause does not exclude this miner",
        )
        assert attempt(["ee" * 32]) == (
            409,
            "rollback exclusion lacks exact accepted referee cause",
        )
        assert attempt([cause, "ee" * 32]) == (409, "rollback cause set differs")
        # ... and so is a cause whose signed record, signer or independent work is missing
        with store._lock:
            store._db.execute(
                "DELETE FROM records_v2 WHERE run_id=? AND kind='resolution' AND id=?",
                (run_id, miner.dispute_id),
            )
        assert attempt([cause]) == (409, f"missing accepted resolution: {miner.dispute_id}")
        forged = envelope_v2.seal(stranger_key(), "ResolutionV2", run_id, record["body"], 10**6)
        with store._lock:
            store._put_record_v2(run_id, "resolution", miner.dispute_id, forged)
        assert attempt([cause]) == (409, "rollback cause lacks original referee signature")
        with store._lock:
            store._put_record_v2(run_id, "resolution", miner.dispute_id, record)
            store._db.execute("DELETE FROM dispute_evidence_v2 WHERE hash=?", (work,))
        assert attempt([cause]) == (409, "rollback cause lacks independent referee work")
        with store._lock:
            store._db.execute("INSERT INTO dispute_evidence_v2 VALUES(?,?)", (work, saved))
        # Control: with the signed record and the evidence in place the cause is the exclusion
        request = envelope_v2.parse_envelope(rollback_raw(world, [hot], [cause]))
        found = store._rollback_exclusions_v2(run_id, Rollback.model_validate(request.body))
        assert found == [{"hotkey": hot, "reason": "FRAUD", "evidence_hash": cause}]


# --- DIB-24: the rollback revision follows every dispute change; a push walks a stable registry


def test_any_change_of_a_dispute_changes_the_revision_a_rollback_repair_must_still_match(
    tmp_path: Path,
    # Covers DIB-24.
) -> None:
    with service_world(tmp_path) as world:
        # Given: the rollback authority revision of a run with a miner and an open dispute
        miner = contested(world, 0)
        store, run_id = world.store, world.run_id
        assert miner.open(miner.fund()).status_code == 200
        opened = store._rollback_snapshot_v2(run_id)
        assert store._rollback_snapshot_v2(run_id) == opened
        # When only the dispute row changes (a pause rewrites nothing else), the revision moves
        miner.disputes().pause(miner.dispute_id)
        paused = store._rollback_snapshot_v2(run_id)
        assert paused != opened
        # And so it does when the dispute is closed
        with store._tx():
            miner.disputes().exclude(miner.dispute_id, now=world.network.now)
        assert store._rollback_snapshot_v2(run_id) not in (opened, paused)


def test_a_beacon_push_survives_a_referee_call_registering_a_guard_meanwhile(
    tmp_path: Path,
    # Covers DIB-24, DIB-D12.
) -> None:
    # A push walks a snapshot of the lease-guard registry (d030605f, ADR 0011 Part 2): a call that
    # registers its guard while the push runs neither breaks the push nor is lost.
    with service_world(tmp_path) as world:
        store, net = world.store, world.network
        registered = threading.Event()

        class Late:
            def advance(self, _round: int) -> None:
                return None

        def register_like_a_referee_call() -> None:
            # referee_v2 registers its guard inside the store transaction, so under the lock
            with store._lock:
                store._lease_guards_v2["late-referee-call"] = Late()
            registered.set()

        class Racing:
            """A guard whose advance coincides with another call registering its own."""

            def advance(self, _round: int) -> None:
                racer.start()
                # Walking the live registry sees the registration at once. A push that holds the
                # store lock keeps it out, so this wait may run out without it.
                registered.wait(10)

        racer = threading.Thread(target=register_like_a_referee_call)
        # Control: with only well-behaved guards the push succeeds
        store._lease_guards_v2["steady"] = Late()
        net.push(net.now + 1)
        store._lease_guards_v2["racing"] = Racing()
        # When the registry changes size while the push walks it
        net.push(net.now + 1)
        # Then: the beacon is accepted, whatever another call registers, and the registration lands
        assert registered.wait(10)
        racer.join(10)
        assert not racer.is_alive()
        assert "late-referee-call" in store._lease_guards_v2


# --- a real round in which one of five miners forged its commit and then contested --------------


@dataclass
class Forged:
    """Five SQL-activated identities, one real round 0 in which HOT[4] lied about its delta hash
    (the audit says MISMATCH, so it is excluded), and the contest of that verdict paused where
    the two parties agree. About 45 s: the five CPU islands and their replays are real."""

    network: Any
    fixtures: ModuleType
    forger: Keypair
    dispute_id: str
    applied: dict[str, Any]

    @property
    def store(self) -> Any:
        return self.network.store

    @property
    def run_id(self) -> str:
        return str(self.network.manifest.run_id())

    def referee(self) -> Any:
        return self.network.client.post(
            f"{self.network.url}/admin/disputes/{self.dispute_id}/referee",
            headers=self.fixtures.admin(),
        )

    def count(self, table: str) -> int:
        return int(self.store._db.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0])


@pytest.fixture(scope="module")
def forged(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Forged]:
    rollback, exit_e2e = heavy_modules()
    generator = exit_e2e.activated_network.__wrapped__(tmp_path_factory)
    network = next(generator)
    enforce_mismatch_sanction(network.store)
    try:
        fixtures = rollback.fixtures
        forger = fixtures.HOT[4]
        applied = rollback.execute_round(network, 0, forge={forger.ss58: {"delta_hash": "aa" * 32}})
        dispute_id = exit_e2e.contest_and_pause(network, forger, fixtures.COLD[4])
        yield Forged(network, fixtures, forger, dispute_id, applied)
    finally:
        generator.close()


def final_op_entries(forged: Forged, miner_says: list[str], auditor_says: list[str]) -> None:
    """Store the two latest answers as both parties' signed outputs of the final op."""
    store, dispute = forged.store, forged.dispute_id
    turn = store._services(forged.run_id)[2].get(dispute)
    auditor = turn.contest.auditor
    with store._tx():
        store._db.execute("DELETE FROM dispute_entries_v2 WHERE id=? AND seq>=?", (dispute, 100))
        for seq, party, says in (
            (100, turn.contest.miner, miner_says),
            (101, auditor, auditor_says),
        ):
            answer = BisectV2(
                dispute_id=dispute,
                seq=seq,
                level="op",
                ctx=[1, 0],
                interval=(0, 1),
                N=2,
                hashes=says,
                party=party,
                previous_transcript_hash=turn.transcript_hash,
            )
            store._db.execute(
                "INSERT INTO dispute_entries_v2 VALUES(?,?,?,?)",
                (dispute, seq, answer.model_dump_json(), b""),
            )


def referee_names_a_loser_only_when_the_replay_contradicts_one_claimant(
    forged: Forged,
) -> None:
    from hypertrain.auditor.island_bisect import IslandParty
    from hypertrain.protocol.messages_v2 import IslandJobV1

    store, run_id, dispute = forged.store, forged.run_id, forged.dispute_id
    disputes = store._services(run_id)[2]
    original = disputes.get(dispute)
    audit_miner = forged.forger.ss58
    # Given: the audit's own verdict on the contested commit is MISMATCH and the parties agreed
    # at step level, so the transcript paused off the final op
    assert store._record_v2(run_id, "excluded", f"0:{audit_miner}")["reason"] == "UNVERIFIED"
    assert (original.paused, original.level) == (True, "step")
    rows = forged.count("dispute_evidence_v2")
    # When / Then: the referee will not decide it, however often it is asked
    mismatch = (409, "mismatch contest must reach exact signed final-op outputs")
    assert verdict_of(forged.referee()) == mismatch
    first_job = store._record_v2(run_id, "referee-job", f"{dispute}:{original.transcript_hash}")
    assert verdict_of(forged.referee()) == mismatch
    assert (
        store._record_v2(run_id, "referee-job", f"{dispute}:{original.transcript_hash}")
        == first_job
    )
    assert forged.count("dispute_evidence_v2") == rows
    # The descriptor is immutable: another historical geometry would need another descriptor
    geometry = store._record_v2(run_id, "trace-geometry", original.contest.verdict_hash)
    with store._lock:
        store._put_record_v2(
            run_id, "trace-geometry", original.contest.verdict_hash, {**geometry, "note": 1}
        )
    changed = (409, "immutable referee descriptor conflicts")
    assert verdict_of(forged.referee()) == changed
    with store._lock:
        store._put_record_v2(run_id, "trace-geometry", original.contest.verdict_hash, geometry)
    # Given: the same transcript shaped as a paused final op, where the replay can convict
    job = IslandJobV1.model_validate(geometry["job"])
    party = IslandParty.published(audit_miner, job, Path(geometry["directory"]))
    truth = party.hashes("op", (1, 0), [0, 1, 1])
    wrong = [truth[0], "ab" * 32, "cd" * 32]
    final_op = {**FINAL_OP, "interval": (0, 1)}
    with store._tx():
        disputes._save(original.model_copy(update=final_op))
    try:
        # the stored answers are the step-level ones: not the two outputs of this op
        lacking = (409, "final op lacks both exact signed claimant outputs")
        assert verdict_of(forged.referee()) == lacking
        # an agreed input the replay does not reproduce
        final_op_entries(forged, ["aa" * 32, *truth[1:]], ["aa" * 32, *truth[1:]])
        assert verdict_of(forged.referee()) == (
            409,
            "independent full replay does not reproduce agreed op input",
        )
        # nobody wrong, or both wrong: the replay cannot identify one claimant
        unclear = (409, "independent op cannot identify one wrong claimant")
        final_op_entries(forged, truth, truth)
        assert verdict_of(forged.referee()) == unclear
        final_op_entries(forged, wrong, [truth[0], "ef" * 32, "01" * 32])
        assert verdict_of(forged.referee()) == unclear
        assert forged.count("dispute_evidence_v2") == rows
        # exactly one wrong: that claimant, and only that one, is the loser
        losers = {}
        for name, miner_says, auditor_says in (
            ("auditor", truth, wrong),
            ("miner", wrong, truth),
        ):
            final_op_entries(forged, miner_says, auditor_says)
            answer = forged.referee()
            assert answer.status_code == 200, answer.text
            evidence = answer.json()["evidence"]
            losers[name] = (evidence["reason"], evidence["loser"])
        auditor_key = original.contest.auditor
        assert losers == {
            "auditor": ("FRAUD", auditor_key),
            "miner": ("FRAUD", audit_miner),
        }
    finally:
        with store._tx():
            store._db.execute("DELETE FROM dispute_entries_v2 WHERE id=? AND seq>=100", (dispute,))
            disputes._save(original)


def forged_commit_earns_the_miner_no_reward_even_while_it_contests(forged: Forged) -> None:
    from hypertrain.protocol.messages import Finalize

    from .support import table_names

    n, fixtures, store, run_id = forged.network, forged.fixtures, forged.store, forged.run_id
    honest = sorted(k.ss58 for k in fixtures.HOT if k.ss58 != forged.forger.ss58)
    everyone = sorted(k.ss58 for k in fixtures.HOT)
    theta = forged.applied["theta_hash"]

    def finalize(included: list[str]) -> Any:
        body = Finalize(
            w=0, final_theta_hash_w1=theta, included=included, entitlements_root="0" * 64
        )
        return n.client.post(
            n.url + "/admin/rounds/0/finalize",
            json=n.signed(fixtures.COORD, "Finalize", body),
            headers=fixtures.admin(),
        )

    def digest() -> str:
        rows = [
            (name, sorted(map(tuple, store._db.execute(f'SELECT * FROM "{name}"'))))
            for name in table_names(store._db)
        ]
        return sha256_hex(repr(rows).encode())

    # Given: the audit excluded the forger, and the forger has since opened a funded contest
    assert store._record_v2(run_id, "excluded", f"0:{forged.forger.ss58}")["reason"] == "UNVERIFIED"
    assert store._services(run_id)[2].get(forged.dispute_id).resolution is None
    # Then: the verified inputs of the round are the four honest miners
    verified = store.verified_inputs_v2(run_id, 0)
    assert sorted(item.roster.hotkey for item in verified) == honest
    # A Finalize that names the forger is refused whole
    before = digest()
    refused = finalize(everyone)
    assert verdict_of(refused) == (
        422,
        "finalized included set differs from full accepted authority",
    )
    assert digest() == before
    # Once the next beacon has closed the dead-end contest, the honest set is minted, the forger not
    n.push(n.now + 1)
    accepted = finalize(honest)
    assert accepted.status_code == 200, accepted.text
    rows = store._db.execute(
        "SELECT hotkey,issued FROM escrow_origins WHERE reward_round=0"
    ).fetchall()
    assert sorted(row["hotkey"] for row in rows) == honest
    escrow = store._services(run_id)[0]
    assert sum(row["issued"] for row in rows) == escrow.policy.round_reward_units
    forger_cold = fixtures.COLD[4].ss58
    assert escrow.balances(forger_cold).reward_pending == 0


def test_a_forged_commit_is_never_decided_by_the_referee_and_earns_no_reward(
    forged: Forged,
) -> None:
    # Covers DIB-02, DIB-12, DIB-06.
    # One world serves both defences, in this order: the second closes the contest that the
    # first needs open. DIB-02 and DIB-12 first, then DIB-06.
    referee_names_a_loser_only_when_the_replay_contradicts_one_claimant(forged)
    forged_commit_earns_the_miner_no_reward_even_while_it_contests(forged)
