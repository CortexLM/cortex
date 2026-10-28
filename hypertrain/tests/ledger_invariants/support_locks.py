"""Builders private to the `locks` package: admission stake, its settlement and its eligibility.

Not collected: no test_ prefix. Everything runs in-process on the v2 `network` fixture through
`service_world`, with the fixture's test keys. Identities are activated by SQL, as
tests/e2e/test_dispute_exit_v2_e2e.py does, because the twelve-trial ladder has its own e2e.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any

import httpx
from pydantic import JsonValue

from env_pin_fixtures import honest_env
from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.challenge.admission import Admission, AdmissionStatus
from hypertrain.challenge.disputes_v2 import DisputesV2
from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2, Settlement
from hypertrain.miner import admission as miner_admission
from hypertrain.miner.admission import sign_join, sign_rotation
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    DisputeV2,
    EscrowLock,
    EscrowRelease,
    HardwareHint,
    PolicyHashes,
    RankWorkResult,
    ResolutionV2,
    RotateRequest,
    RoundOpenV2,
    StartStateV2,
    WorkScreenV2,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

from .support import SERVICE_FIXTURE, World, load_fixture_module

STAKE = 1000
BOND = 100  # the contest bond: DisputePolicyV2.max_referee_cost_units of the fixture
VERDICT = sha256_hex(b"locks-verdict")
EVIDENCE = sha256_hex(b"locks-evidence")


def service_module() -> ModuleType:
    return load_fixture_module(*SERVICE_FIXTURE)


def ident(label: str) -> str:
    """A 64-hex operation id derived from a label."""
    return sha256_hex(label.encode())


def rotation_envelope(
    run_id: str,
    cold: Keypair,
    old_hotkey: str,
    new_hot: Keypair,
    operation_id: str,
    expires_beacon: int = 10_000,
) -> dict[str, JsonValue]:
    """The coldkey's sealed request to move the admission of `old_hotkey` to `new_hot`.

    ADR 0010 part 1 asks the new hotkey's consent in the body (`new_hot_sig`); the signature is
    added when the tree has it, so the one request is valid on a tree without it too.
    """
    consent = getattr(miner_admission, "sign_rotation_consent", None)
    if consent is None:
        request = RotateRequest.model_validate(
            {"hotkey": old_hotkey, "new_hotkey": new_hot.ss58, "operation_id": operation_id}
        )
    else:
        request = consent(
            new_hot,
            run_id=run_id,
            coldkey=cold.ss58,
            hotkey=old_hotkey,
            operation_id=operation_id,
            expires_beacon=expires_beacon,
        )
    return sign_rotation(cold, run_id, request, expires_beacon)


def ask_as_owner(net: Any, miner: Miner, route: str) -> httpx.Response:
    """The owner of `miner`'s admission asks for its trial `route` ("challenge" or "job").

    ADR 0010 part 3: the request is a POST whose body is the envelope of `ChallengeRequest` or
    `JobRequest`, signed by the admission's hotkey; the unsigned GET answers 405. A tree before
    that part has no `owner_request` in its fixture and answers the plain GET.
    """
    url = net.url + f"/join/{miner.admission_id}/{route}"
    sign = getattr(net, "owner_request", None)
    response: httpx.Response
    if sign is None:
        response = net.client.get(url)
    else:
        message = {"challenge": "ChallengeRequest", "job": "JobRequest"}[route]
        response = net.client.post(url, json=sign(message, miner.admission_id, miner.hot))
    return response


@dataclass(frozen=True)
class Miner:
    """An admitted identity: slot `index` of the fixture (its coldkey and genesis origin)."""

    index: int
    hot: Keypair
    cold: Keypair
    admission_id: str

    @property
    def hotkey(self) -> str:
        return self.hot.ss58

    @property
    def coldkey(self) -> str:
        return self.cold.ss58


@dataclass(frozen=True)
class Contest:
    """A funded, opened contest: its dispute id and the bond lock that backs it."""

    dispute_id: str
    bond: EscrowLock


class Rig:
    """One run of the v2 network fixture with a few admitted miners and their escrow."""

    def __init__(self, world: World) -> None:
        self.world = world
        self.service = service_module()
        self.net = world.network
        self.run_id = world.run_id

    @property
    def escrow(self) -> EscrowV2:
        escrow: EscrowV2 = self.world.store._services(self.run_id)[0]
        return escrow

    @property
    def admission(self) -> Admission:
        admission: Admission = self.world.store._services(self.run_id)[1]
        return admission

    @property
    def disputes(self) -> DisputesV2:
        disputes: DisputesV2 = self.world.store._services(self.run_id)[2]
        return disputes

    def join_request(
        self, index: int = 0, *, hot: Keypair | None = None, request: str = ""
    ) -> httpx.Response:
        """A dual-signed join over HTTP; `hot` replaces the slot's hotkey (a pinned role key)."""
        hot = hot or self.service.HOT[index]
        signed = sign_join(
            hot,
            self.service.COLD[index],
            run_id=self.run_id,
            request_id=ident(f"join-{index}-{hot.ss58}-{request}"),
            expires_beacon=10000,
            policy_hash=self.net.manifest.network.admission_policy_hash,
            hardware_hint=HardwareHint(device_name="cpu", device_count=999, driver="advisory"),
            env=honest_env(),
        )
        return self.net.client.post(self.net.url + "/join", content=canonicalize(signed.body()))

    def join(self, index: int = 0, *, hot: Keypair | None = None) -> Miner:
        """An admitted miner: the join must be accepted."""
        hot = hot or self.service.HOT[index]
        response = self.join_request(index, hot=hot)
        assert response.status_code == 200, response.text
        return Miner(index, hot, self.service.COLD[index], str(response.json()["admission_id"]))

    def status(self, miner: Miner) -> AdmissionStatus:
        return self.admission.status(miner.hotkey, now=self.net.now)

    def stake_lock(
        self, miner: Miner, *, units: int = STAKE, operation: str | None = None
    ) -> EscrowLock:
        """The lock event with which the miner's coldkey stakes `units` of its own origin."""
        return EscrowLock(
            operation_id=operation or ident(f"stake-{miner.index}"),
            owner=miner.coldkey,
            units=units,
            origin_ids=[self.net.origin_ids[miner.index]],
            admission_id=miner.admission_id,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )

    def lock_stake(
        self, miner: Miner, *, units: int = STAKE, operation: str | None = None
    ) -> EscrowLock:
        """The miner's coldkey locks `units` of its own origin as admission stake, over HTTP."""
        lock = self.stake_lock(miner, units=units, operation=operation)
        response = self.net.client.post(
            self.net.url + "/escrow/lock", json=self.net.signed(miner.cold, "EscrowLock", lock)
        )
        assert response.status_code == 200, response.text
        return lock

    def screen(self, miner: Miner) -> WorkScreenV2:
        """The work screen a graduated miner holds: only its digest is read by an accept."""
        spec = self.net.manifest.training.reference_spec
        return WorkScreenV2(
            env=honest_env(),
            evidence_kind="work-proof-v1",
            admission_id=miner.admission_id,
            nonce=sha256_hex(b"locks-nonce"),
            policy_hash=self.net.manifest.network.admission_policy_hash,
            image_digest=spec.image_digest,
            layout=spec.layout,
            challenge_hash=sha256_hex(b"locks-challenge"),
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

    def activate(self, miner: Miner) -> None:
        """ACTIVE with a screen that outlives the test, as a graduated miner would hold."""
        with self.world.store._tx():
            self.world.store._db.execute(
                "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,canary_blocks='1,2,3',"
                "screen_json=?,screen_until=1000000000 WHERE admission_id=?",
                (self.screen(miner).model_dump_json(), miner.admission_id),
            )

    def accept(self, miner: Miner, assignment_hash: str) -> httpx.Response:
        """The miner's signed AcceptV2 of its round-0 assignment."""
        spec = self.net.manifest.training.reference_spec
        row = self.world.store._db.execute(
            "SELECT screen_json FROM admissions_v2 WHERE hotkey=?", (miner.hotkey,)
        ).fetchone()
        body = {
            "w": 0,
            "hotkey": miner.hotkey,
            "assignment_hash": assignment_hash,
            "image_digest": spec.image_digest,
            "driver_version": spec.driver_allowlist[0],
            "n_gpus": 1,
            "work_screen_hash": sha256_hex(canonicalize(json.loads(row[0]))),
        }
        return self.net.client.post(
            self.net.url + "/accept", json=self.net.signed(miner.hot, "AcceptV2", body)
        )

    def assignment_hash(self, miner: Miner) -> str:
        view = self.net.client.get(self.net.url + "/rounds/0")
        assert view.status_code == 200, view.text
        return str(
            next(a for a in view.json()["assignment"] if a["hotkey"] == miner.hotkey)[
                "assignment_hash"
            ]
        )

    def admit(self, index: int, *, stake: int = STAKE, hot: Keypair | None = None) -> Miner:
        """Join, stake `stake` units (none when 0) and graduate by SQL."""
        miner = self.join(index, hot=hot)
        if stake:
            self.lock_stake(miner, units=stake)
        self.activate(miner)
        return miner

    def release(
        self,
        miner: Miner,
        lock: EscrowLock,
        finality: str,
        closed: str,
        *,
        attempt: str = "",
    ) -> httpx.Response:
        """The owner's signed release of `lock`, posted to the public route."""
        release = EscrowRelease(
            operation_id=ident(f"release-{lock.operation_id}-{attempt}"),
            owner=lock.owner,
            units=lock.units,
            origin_ids=lock.origin_ids,
            admission_id=lock.admission_id,
            dispute_id=lock.dispute_id,
            lock_id=lock.operation_id,
            finality_hash=finality,
            closed_dispute_root=closed,
        )
        return self.net.client.post(
            self.net.url + "/escrow/release",
            json=self.net.signed(miner.cold, "EscrowRelease", release),
        )

    def open_trial(self, miner: Miner) -> httpx.Response:
        """The owner's signed request for the challenge of its admission: opens its trial."""
        return ask_as_owner(self.net, miner, "challenge")

    def read_job(self, miner: Miner) -> httpx.Response:
        """The owner's signed request for the job of its open trial."""
        return ask_as_owner(self.net, miner, "job")

    def bond_entry(self, miner: Miner, lock_id: str) -> dict[str, Any]:
        """What GET /admission/{hotkey} tells the owner about its lock `lock_id` (ADR 0010 6.5):
        whether it is releasable and the finality hash and closed-dispute root to sign."""
        status = self.net.client.get(self.net.url + "/admission/" + miner.hotkey).json()
        entry: dict[str, Any] = next(e for e in status["bond"]["locks"] if e["lock_id"] == lock_id)
        return entry

    def open_contest(self, miner: Miner, *, verdict: str = VERDICT) -> Contest:
        """The audit's contest record is seeded; the funding and the dispute go through the
        public routes."""
        store, inner = self.world.store, self.net.manifest.training.inner
        contest = {
            "verdict_hash": verdict,
            "challenge_hash": sha256_hex(b"locks-challenge"),
            "w": 0,
            "miner": miner.hotkey,
            "auditor": self.service.AUDITORS[0].ss58,
            "referee": self.service.REFEREE.ss58,
            "step_span": inner.H // inner.J,
            "layer_span": 2,
            "op_span": 2,
            "admission_id": miner.admission_id,
            "coldkey": miner.coldkey,
            "evidence_hash": EVIDENCE,
            "auditor_coldkey": None,
        }
        with store._lock:
            store._put_record_v2(self.run_id, "contest", verdict, contest)
            store._put_record_v2(self.run_id, "finality", "0", {"w": 0})
            store._put_record_v2(
                self.run_id, "excluded", f"0:{miner.hotkey}", {"reason": "UNVERIFIED"}
            )
        dispute_id = sha256_hex(canonicalize([self.run_id, verdict, miner.hotkey]))
        origin = store._db.execute(
            "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=?",
            (miner.coldkey, BOND),
        ).fetchone()[0]
        bond = EscrowLock(
            operation_id=ident(f"bond-{miner.index}"),
            owner=miner.coldkey,
            units=BOND,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=dispute_id,
            kind="LOCK_CONTEST",
        )
        response = self.net.client.post(
            self.net.url + "/escrow/lock", json=self.net.signed(miner.cold, "EscrowLock", bond)
        )
        assert response.status_code == 200, response.text
        body = DisputeV2(
            hotkey=miner.hotkey, verdict_hash=verdict, action="contest", bond_lock=bond.operation_id
        )
        response = self.net.client.post(
            self.net.url + "/dispute", json=self.net.signed(miner.hot, "DisputeV2", body)
        )
        assert response.status_code == 200, response.text
        return Contest(dispute_id, bond)

    def refereed(self, dispute_id: str, reason: str, loser: str | None) -> httpx.Response:
        """The referee's signed resolution, with the replay evidence the store would have kept."""
        turn = self.disputes.get(dispute_id)
        evidence = RefereeEvidence(
            dispute_id=dispute_id,
            transcript_hash=turn.transcript_hash,
            reason=reason,  # type: ignore[arg-type]
            loser=loser,
            predecessor_hash="11" * 32,
            inputs_hash="22" * 32,
            output_hash="33" * 32,
            op_spec="verified.actual.op",
        )
        with self.world.store._tx():
            self.disputes.register_evidence(evidence)
        resolution = ResolutionV2(
            dispute_id=dispute_id,
            transcript_hash=turn.transcript_hash,
            reason=reason,  # type: ignore[arg-type]
            loser=loser,
            evidence_hash=evidence.digest(),
        )
        return self.net.client.post(
            self.net.url + "/resolution",
            json=self.net.signed(self.service.REFEREE, "ResolutionV2", resolution, expiry=10**6),
        )

    def opening(self, miners: list[Miner], **first_entry: Any) -> RoundOpenV2:
        """The round-0 opening whose roster lists `miners` with the standing they hold now.

        Every other field is the honest one, so a refusal of this opening is a refusal of its
        roster and nothing else. `first_entry` overrides fields of the first roster entry.
        """
        manifest = self.net.manifest
        theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
        cache = AnchorCache()
        starts, roster = [], []
        for slot, miner in enumerate(miners):
            record = self.status(miner).record
            anchor = cache.genesis(manifest, miner.hotkey, theta)
            starts.append(
                StartStateV2(
                    run_id=self.run_id,
                    w=0,
                    hotkey=miner.hotkey,
                    theta_hash=state_hash(theta),
                    state_object_sha256=sha256_hex(pack_state(theta, anchor.state)),
                    opt_state_hash=optimizer_hash(anchor.state),
                    ef_object_sha256=sha256_hex(pack_state(anchor.ef)),
                    ef_hash=state_hash(anchor.ef),
                    parent_anchor_hash=anchor.anchor_hash,
                    global_step0=0,
                    anchor_verdict_hash=anchor.proof_hash,
                )
            )
            roster.append(
                {
                    "hotkey": miner.hotkey,
                    "slot": slot,
                    "q_i": "0000803f",
                    "admission_id": record.admission_id,
                    "coldkey_group": record.coldkey,
                    "state": record.state,
                    "eligible_weight": 4194304,
                }
            )
        roster[0].update(first_entry)
        now = self.net.now
        return RoundOpenV2(
            w=0,
            prev_final_hash="0" * 64,
            theta_hash=state_hash(theta),
            outer_state_hash="0" * 64,
            center_hash="0" * 64,
            roster_hash=sha256_hex(canonicalize(roster)),
            honeypot_commit="0" * 64,
            d_open=now + 1,
            d_assign=now + 2,
            d_commit=now + 30,
            d_audit=now + 31,
            d_upload=now + 200,
            d_final=now + 300,
            contract_version=2,
            policy_hashes=PolicyHashes.model_validate(
                {name: getattr(manifest.network, name) for name in PolicyHashes.model_fields}
            ),
            registry_epoch=0,
            start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
            audit_mode="anchored-full",
            roster=roster,
        )

    def open_round(self, opening: RoundOpenV2) -> httpx.Response:
        """The coordinator's signed opening, posted to the operator route."""
        return self.net.client.post(
            self.net.url + "/admin/rounds",
            json=self.net.signed(self.service.COORD, "RoundOpenV2", opening),
            headers=self.world.admin,
        )


def seed_live_origin(
    rig: Rig,
    miner: Miner,
    *,
    units: int = 500,
    mature_at: int = 5,
    **settlement: Any,
) -> str:
    """What `finalize_v2` leaves for one miner of round 0: the issued origin pending in the
    miner's coldkey, the finalized round, and the accepted settlement record of the origin.

    `settlement` overrides the fields of the record (default: a MATCH vesting at `mature_at`).
    The rows are written directly because a real finalize needs a whole audited round.
    """
    origin, final = ident("live-origin"), ident("live-finalize")
    fields: dict[str, Any] = {
        "finality_hash": final,
        "closed_dispute_root": sha256_hex(canonicalize([])),
        "release_beacon": mature_at,
    }
    record = settled_status(**{**fields, **settlement})
    store, db = rig.world.store, rig.escrow.db
    with store._tx():
        db.execute(
            "INSERT INTO escrow_origins VALUES(?,?,?,0,?)", (origin, units, mature_at, miner.hotkey)
        )
        db.execute(
            "INSERT INTO escrow_units VALUES(?,?,'reward_pending','',?)",
            (origin, miner.coldkey, units),
        )
        db.execute("INSERT INTO escrow_finalized VALUES(0,?)", (final,))
        store._put_record_v2(rig.run_id, "settlement", origin, record.body())
    return origin


def vested(rig: Rig, origin: str) -> int:
    """Units of `origin` that have left reward_pending for the owner's free balance."""
    return int(
        rig.escrow.db.execute(
            "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE origin=? AND bucket='available'",
            (origin,),
        ).fetchone()[0]
    )


def units_of(escrow: EscrowV2, owner: str) -> dict[str, int]:
    """Bucket -> units held by `owner`: the ledger as one comparable value."""
    balances = escrow.balances(owner)
    return {
        "available": balances.available,
        "reward_pending": balances.reward_pending,
        "admission_locked": balances.admission_locked,
        "dispute_locked": balances.dispute_locked,
        "paid": balances.paid,
        "burned": balances.burned,
    }


def event_count(escrow: EscrowV2) -> int:
    return int(escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0])


def settled_status(**fields: Any) -> Settlement:
    """A Settlement with the defaults of an accepted nonfraud record."""
    base: dict[str, Any] = {
        "finality_hash": "11" * 32,
        "closed_dispute_root": "11" * 32,
        "release_beacon": 2,
        "unresolved": False,
        "outcome": "MATCH",
    }
    return Settlement.model_validate({**base, **fields})


ADMISSION_FIXTURE = ("li_admission_fixture", "challenge/test_admission_v2.py")


class Lab:
    """A bare Admission service on a file ledger, as tests/challenge/test_admission_v2.py builds
    it: permissive ledger callbacks unless a test installs its own, `now` is a round number."""

    def __init__(self, path: Path, *, setup: Any = None) -> None:
        self.A = load_fixture_module(*ADMISSION_FIXTURE)
        path.mkdir(parents=True, exist_ok=True)
        self.s = setup if setup is not None else self.A.setup()
        self.a: Admission = self.A.service(self.s, path)
        self.funded = False

    def join(
        self,
        hot: Keypair | None = None,
        cold: Keypair | None = None,
        request: str | None = None,
        now: int = 1,
    ) -> str:
        signed = self.A.request(self.s, request or self.A.H, hot or self.A.HOT, cold or self.A.COLD)
        joined = self.a.join(canonicalize(signed.body()), now=now, ip_prefix="loopback")
        return str(joined.admission_id)

    def lock_op(self, admission_id: str, units: int, operation: str) -> EscrowLock:
        if not self.funded:
            self.origin = self.A.fund(self.a.escrow)[0]
            self.funded = True
        return EscrowLock(
            operation_id=operation,
            owner=self.A.COLD.ss58,
            units=units,
            origin_ids=[self.origin],
            admission_id=admission_id,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )

    def stake(self, admission_id: str, units: int, operation: str = "22" * 32) -> None:
        """The coldkey locks `units` as admission stake, through the call the route makes."""
        op = self.lock_op(admission_id, units, operation)
        signed = seal(self.A.COLD, "EscrowLock", self.a.escrow.run_id, op, 500)
        self.a.lock(canonicalize(signed), now=1)

    def enqueue(self, admission_id: str, units: int, operation: str) -> None:
        """A lock the service accepted but has not yet applied: an outbox row with no receipt."""
        self.a.store.enqueue_lock(self.lock_op(admission_id, units, operation), self.A.COLD.ss58)

    def set(self, admission_id: str, **columns: Any) -> None:
        """Test-only edit of the admission row (the standing a trial ladder would have left)."""
        names = ",".join(f"{name}=?" for name in columns)
        with self.a.escrow.tx():
            self.a.store.db.execute(
                f"UPDATE admissions_v2 SET {names} WHERE admission_id=?",  # noqa: S608
                (*columns.values(), admission_id),
            )

    def live(self, admission_id: str) -> None:
        """ACTIVE with a screen that outlives the test."""
        self.set(
            admission_id,
            state="ACTIVE",
            clean_count=12,
            canary_blocks="1,2,3",
            screen_until=1_000_000,
        )

    def settle(self, **fields: Any) -> None:
        """Install the settlement callback: the record `fields` for its own reference only."""
        status = settled_status(run_id=self.a.escrow.run_id, **fields)

        def callback(reference: str) -> Settlement:
            if reference != status.dispute_id:
                raise EscrowError("UNACCEPTED_SETTLEMENT")
            return status

        self.a.escrow.settlement = callback
