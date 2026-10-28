"""Shadow-reward credit at the service boundary (SHD-03, 05 to 13, 18 to 24).

The service reserves an ordinal only for a settled first trial whose two executions it retained,
and credits it only on evidence bound to that reservation. These tests drive the real signed HTTP
routes of the v2 network fixture over the real CPU reference and miner executions of
`shadow_reservation_work`, change exactly one thing, and assert the exact refusal and that the
whole database (every table) is unchanged. A defect candidate of the Lens report is a strict xfail
with a passing control beside it.
"""

from __future__ import annotations

import copy
import shutil
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import replace
from pathlib import Path
from typing import Any

import httpx
import pytest
from fastapi.testclient import TestClient

from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.app import Config, create_app
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.protocol.envelope_v2 import parse_envelope
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import (
    IslandJobV1,
    WorkProof,
    WorkScreenV2,
)

from .support import SERVICE_FIXTURE, Route, load_fixture_module, routes
from .support_shadow import (
    UNIT,
    add_due_origin,
    db_digest,
    open_challenge,
    push_round,
    reauthenticate,
    rotate_admission,
)

service = load_fixture_module(*SERVICE_FIXTURE)
# Fixtures of the service module, bound here so pytest discovers them for this module.
network = service.network
shadow_reservation_work = service.shadow_reservation_work
reserved_trial = service.reserved_trial
shadow_acceptance = service.shadow_acceptance
HOT, COLD, REFEREE, AUDITORS = service.HOT, service.COLD, service.REFEREE, service.AUDITORS

SETTLED = "shadow new reservation requires settled first trial"
BINDING = "shadow authenticated evidence binding differs"

# Findings of the Lens report that the code still has: strict xfails, each with the passing
# control beside it. The reason ends with the fix lot that closes it, so that the lot flips the
# marker mechanically: remove the marker of every test the lot makes XPASS.
LOST_RESERVATION_STRANDED = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "SHD-02+SHD-13+CRD-D02: a lost reservation reply is recovered by an identical retry "
        "whatever expiry the miner chose for its commit [fix-lot: none (no design yet)]"
    ),
)
ROTATION_STRANDS = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "SHD-21+SHD-D4: rotating an admission's hotkey never strands the evidence frozen "
        "under its old hotkey [fix-lot: none (no design yet)]"
    ),
)
# The Lens property is "credited or released". No release operation exists to assert, so the
# test pins the credited half; a fix that releases the reservation instead must rewrite it.
NEXT_TRIAL_STRANDS = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "SHD-20+SHD-D2: a reservation whose trial is still the admission's first finalized "
        "MATCH can always be credited [fix-lot: decision (product choice pending)]"
    ),
)


def post_shadow(
    n: Any, route: str, identity: str, body: Any, headers: Any = None
) -> httpx.Response:
    """POST to an admin shadow route; a `bytes` body is sent as it is, not as JSON."""
    content = {"content": body} if isinstance(body, bytes) else {"json": body}
    return n.client.post(
        n.url + f"/admin/join/{identity}/{route}",
        headers=service.admin() if headers is None else headers,
        **content,
    )


def reserve(n: Any, identity: str, body: Any, headers: Any = None) -> httpx.Response:
    return post_shadow(n, "shadow-reservation", identity, body, headers)


def credit(n: Any, identity: str, body: Any, headers: Any = None) -> httpx.Response:
    return post_shadow(n, "shadow-reward", identity, body, headers)


def refusal(response: httpx.Response, status: int, detail: str) -> None:
    assert response.status_code == status, response.text
    assert response.json()["detail"].startswith(detail), response.text


def counts(n: Any, kind: str) -> int:
    return int(
        n.store._db.execute("SELECT COUNT(*) FROM records_v2 WHERE kind=?", (kind,)).fetchone()[0]
    )


def escrow_of(n: Any) -> Any:
    return n.store._services(n.manifest.run_id())[0]


def admission_of(n: Any) -> Any:
    return n.store._services(n.manifest.run_id())[1]


# --- SHD-03, SHD-02: a new reservation needs a settled first trial ----------------------------


def spoil_first_trial(n: Any, identity: str, fault: str) -> None:
    db, admission = n.store._db, admission_of(n)
    if fault == "a second clean trial":
        db.execute("UPDATE admissions_v2 SET clean_count=2 WHERE admission_id=?", (identity,))
    elif fault == "an earlier finalized trial":
        # Trial epochs are one global counter; an earlier trial of the same admission is a row
        # with a smaller epoch, so the reserved epoch is no longer the admission's first.
        db.execute(
            "INSERT INTO admission_trials VALUES(?,?,?,?,?,?)",
            (-1, identity, "00" * 32, 1, 1, "00" * 32),
        )
    elif fault == "a pending dispute flag":
        db.execute("UPDATE admissions_v2 SET pending_dispute=1 WHERE admission_id=?", (identity,))
    elif fault == "an unresolved pending reference":
        with admission.escrow.tx():
            admission.store.bind_pending(admission.store.by_id(identity), "fa" * 32, "fb" * 32)
    elif fault == "a mismatched original trial":
        db.execute("UPDATE admission_trial_results SET outcome='MISMATCH'")


FIRST_TRIAL_FAULTS = {
    "a second clean trial": SETTLED,
    "an earlier finalized trial": SETTLED,
    "a pending dispute flag": SETTLED,
    "an unresolved pending reference": SETTLED,
    "a mismatched original trial": "shadow new reservation requires original MATCH",
}


def test_a_settled_first_trial_is_reserved_with_its_custody_record_and_issues_nothing(
    reserved_trial: Any,
) -> None:
    # Given: the control of the refusals below
    n, identity, body = reserved_trial
    escrow, before = escrow_of(n), escrow_of(n).balances()

    # When
    response = reserve(n, identity, body)

    # Then: one reservation and its custody record, ordinal 0, and not a unit issued
    assert response.status_code == 200, response.text
    assert response.json()["shadow_ordinal"] == 0
    assert (counts(n, "shadow-reservation"), counts(n, "shadow-custody")) == (1, 1)
    assert escrow.balances() == before
    assert n.store._db.execute("SELECT COUNT(*) FROM escrow_shadow_finalized").fetchone()[0] == 0


def test_a_reservation_whose_custody_record_cannot_be_stored_takes_no_ordinal(
    reserved_trial: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: a request that is valid in every way, and a store that cannot write the custody record
    n, identity, body = reserved_trial
    write = n.store._put_record_v2

    def refuse_custody(run_id: str, kind: str, record_id: str, data: Any) -> None:
        if kind == "shadow-custody":
            raise ChallengeError(503, "custody record unavailable")
        write(run_id, kind, record_id, data)

    monkeypatch.setattr(n.store, "_put_record_v2", refuse_custody)
    before = db_digest(n.store._db)

    # When / Then: the reservation fails whole, so the ordinal it took is not left behind
    refusal(reserve(n, identity, body), 503, "custody record unavailable")
    assert db_digest(n.store._db) == before

    # And: with the record writable again, the same request is the first reservation (ordinal 0)
    monkeypatch.undo()
    retry = reserve(n, identity, body)
    assert retry.status_code == 200, retry.text
    assert retry.json()["shadow_ordinal"] == 0
    assert (counts(n, "shadow-reservation"), counts(n, "shadow-custody")) == (1, 1)


@pytest.mark.parametrize("fault", FIRST_TRIAL_FAULTS)
def test_a_new_reservation_needs_a_matching_clean_dispute_free_first_trial(
    reserved_trial: Any, fault: str
) -> None:
    # Given: an original trial that is not, any more, the settled first trial of its admission
    n, identity, body = reserved_trial
    spoil_first_trial(n, identity, fault)
    before = db_digest(n.store._db)

    # When / Then: the reservation is refused and the whole database is unchanged
    refusal(reserve(n, identity, body), 409, FIRST_TRIAL_FAULTS[fault])
    assert db_digest(n.store._db) == before


# --- SHD-05, 07, 08, 11: two distinct retained executions, the service's reference ------------


def spoil_custody(n: Any, body: dict[str, Any], fault: str, tmp_path: Path) -> None:
    store, run_id, db = n.store, n.manifest.run_id(), n.store._db
    custody = body["custody"]
    reference = store._record_v2(run_id, "shadow-execution", custody["reference_operation_hash"])
    if fault == "reference and miner swapped":
        # Every field of the two executions is exchanged, so only their roles are wrong.
        for field in ("operation", "publication", "result"):
            a, b = f"reference_{field}_hash", f"miner_{field}_hash"
            custody[a], custody[b] = custody[b], custody[a]
    elif fault == "retained only as private runtime custody":
        db.execute("UPDATE records_v2 SET kind='runtime-custody' WHERE kind='shadow-execution'")
    elif fault == "publication bytes changed":
        (Path(reference["directory"]) / "rank-0/delta.bin").write_bytes(b"changed original bytes")
    elif fault == "publication outside the store":
        outside = tmp_path / "outside"
        shutil.copytree(reference["directory"], outside)
        reference["directory"] = str(outside)
        db.execute(
            "UPDATE records_v2 SET data=? WHERE kind='shadow-execution' AND id=?",
            (canonicalize(reference).decode(), custody["reference_operation_hash"]),
        )
    elif fault == "special file in the publication":
        (Path(reference["directory"]) / "link").symlink_to("rank-0")
    elif fault == "nonce of the trial row":
        db.execute("UPDATE admission_trials SET nonce=?", ("ff" * 32,))
    elif fault == "accepted proof differs from the reference":
        row = db.execute("SELECT epoch,proof FROM admission_trial_results").fetchone()
        proof = WorkProof.model_validate(parse_envelope(row["proof"]).body)
        forged = n.signed(HOT[0], "WorkProof", proof.model_copy(update={"delta_hash": "ee" * 32}))
        db.execute(
            "UPDATE admission_trial_results SET proof=? WHERE epoch=?",
            (canonicalize(forged).decode(), row["epoch"]),
        )


CUSTODY_FAULTS = {
    "reference and miner swapped": "shadow accepted execution binding differs",
    "retained only as private runtime custody": "missing accepted shadow-execution: ",
    "publication bytes changed": "shadow original publication changed",
    "publication outside the store": "shadow accepted publication outside store custody",
    "special file in the publication": "shadow publication special file",
    "nonce of the trial row": "shadow original work binding differs",
    "accepted proof differs from the reference": "shadow original work binding differs",
}


@pytest.mark.parametrize("fault", CUSTODY_FAULTS)
def test_a_reservation_needs_two_byte_exact_retained_executions_of_the_service_reference(
    reserved_trial: Any, tmp_path: Path, fault: str
) -> None:
    # Given: retained custody that is wrong in exactly one way
    n, identity, body = reserved_trial
    spoil_custody(n, body, fault, tmp_path)
    before = db_digest(n.store._db)

    # When / Then: refused for that reason, nothing reserved, nothing journaled
    refusal(reserve(n, identity, body), 409, CUSTODY_FAULTS[fault])
    assert db_digest(n.store._db) == before


# --- SHD-09, 07, 08, 10: acceptance evidence binds anchor, auditor and receipt ----------------


def tamper_evidence(n: Any, body: dict[str, Any], fault: str) -> dict[str, Any]:
    if fault == "receipt signed by a key that is not a pinned auditor":
        return reauthenticate(n, body, receipt_signer=HOT[0])
    if fault == "verdict signed by another auditor than the receipt":
        return reauthenticate(n, body, verdict_signer=AUDITORS[1])
    if fault == "anchor of another genesis":
        return reauthenticate(n, body, audit={"anchor_hash": "ff" * 32})
    if fault == "recomputed leaves root of another run":
        return reauthenticate(n, body, verdict={"recomputed_leaves_root": "ff" * 32})
    if fault == "replay image that is not the pinned one":
        env = {**body["verdict"]["body"]["replay_env"], "image_digest": "sha256:" + "ee" * 32}
        return reauthenticate(n, body, verdict={"replay_env": env})
    if fault == "a verdict that is not a match":
        return reauthenticate(n, body, verdict={"result": "MISMATCH", "first_bad_leaf": 0})
    if fault == "assignment of the receipt":
        return reauthenticate(n, body, receipt={"assignment_hash": "ff" * 32})
    if fault == "sample ids of the receipt":
        return reauthenticate(n, body, receipt={"sample_ids_hash": "ff" * 32})
    if fault == "final state of the receipt":
        return reauthenticate(n, body, receipt={"final_state_hash": "ff" * 32})
    if fault == "backend authority of the receipt":
        return reauthenticate(n, body, receipt={"backend_qualification_authority_hash": "ff" * 32})
    if fault == "completion after the evidence time":
        return reauthenticate(n, body, receipt={"completed_beacon": n.now + 1})
    assert fault == "reward authority signature"
    changed = copy.deepcopy(body)
    changed["reward"]["authority_sig"] = "0" * 128
    return changed


EVIDENCE_FAULTS = {
    "receipt signed by a key that is not a pinned auditor": (
        403,
        "shadow distinct pinned auditor required",
    ),
    "verdict signed by another auditor than the receipt": (
        409,
        "signer is not authorized for message role",
    ),
    "anchor of another genesis": (409, BINDING),
    "recomputed leaves root of another run": (409, BINDING),
    "replay image that is not the pinned one": (409, BINDING),
    "a verdict that is not a match": (409, BINDING),
    "assignment of the receipt": (409, BINDING),
    "sample ids of the receipt": (409, BINDING),
    "final state of the receipt": (409, BINDING),
    "backend authority of the receipt": (409, BINDING),
    "completion after the evidence time": (409, BINDING),
    "reward authority signature": (409, "ORIGIN_AUTHORITY"),
}


def test_evidence_bound_to_the_reservation_is_credited_as_one_pending_origin(
    shadow_acceptance: Any,
) -> None:
    # Given: the control of the refusals below
    n, identity, body = shadow_acceptance
    escrow, before = escrow_of(n), escrow_of(n).balances()

    # When
    response = credit(n, identity, body)

    # Then: one round budget issued as pending collateral to the reserved coldkey
    assert response.status_code == 200, response.text
    after = escrow.balances()
    assert (after.issued, after.reward_pending) == (
        before.issued + UNIT,
        before.reward_pending + UNIT,
    )
    assert after.available == before.available and after.conserved()
    assert response.json()["authority_hash"] == body["reward"]["tape_hash"]
    assert counts(n, "shadow-evidence") == counts(n, "shadow-settlement") == 1


@pytest.mark.parametrize("fault", EVIDENCE_FAULTS)
def test_acceptance_refuses_evidence_that_is_not_bound_to_anchor_auditor_and_receipt(
    shadow_acceptance: Any, fault: str
) -> None:
    # Given: acceptance evidence whose hashes and signatures are all made again after the change
    n, identity, body = shadow_acceptance
    tampered = tamper_evidence(n, body, fault)
    before = db_digest(n.store._db)

    # When / Then
    status, detail = EVIDENCE_FAULTS[fault]
    refusal(credit(n, identity, tampered), status, detail)
    assert db_digest(n.store._db) == before


def test_credit_needs_accepted_execution_records_and_private_runtime_custody_is_not_one(
    shadow_acceptance: Any,
) -> None:
    # Given: a reservation whose two executions are, from now on, only private runtime custody
    n, identity, body = shadow_acceptance
    n.store._db.execute(
        "UPDATE records_v2 SET kind='runtime-custody' WHERE kind='shadow-execution'"
    )
    before = db_digest(n.store._db)

    # When / Then: acceptance re-reads the accepted records and finds none
    refusal(credit(n, identity, body), 409, "missing accepted shadow-execution: ")
    assert db_digest(n.store._db) == before


# --- SHD-22, SHD-18: disputed, fraud and banned identities ------------------------------------


def spoil_identity(n: Any, identity: str, fault: str) -> None:
    db, admission = n.store._db, admission_of(n)
    if fault == "banned":
        with admission.escrow.tx():
            admission.store.transition(identity, "BANNED", "FINALIZED_TRAINING_FRAUD", n.now)
    elif fault == "fraud outcome":
        db.execute("UPDATE admission_trial_results SET outcome='FRAUD'")
    elif fault == "infrastructure outcome":
        db.execute("UPDATE admission_trial_results SET outcome='INFRASTRUCTURE'")
    elif fault == "pending dispute flag":
        db.execute("UPDATE admissions_v2 SET pending_dispute=1 WHERE admission_id=?", (identity,))
    elif fault == "unresolved pending reference":
        with admission.escrow.tx():
            admission.store.bind_pending(admission.store.by_id(identity), "fa" * 32, "fb" * 32)


IDENTITY_FAULTS = [
    "banned",
    "fraud outcome",
    "infrastructure outcome",
    "pending dispute flag",
    "unresolved pending reference",
]


@pytest.mark.parametrize("fault", IDENTITY_FAULTS)
def test_a_banned_disputed_or_non_matching_identity_is_not_credited(
    shadow_acceptance: Any, fault: str
) -> None:
    # Given: a reservation, then the identity's first trial or standing turns bad
    n, identity, body = shadow_acceptance
    spoil_identity(n, identity, fault)
    before = db_digest(n.store._db)

    # When / Then
    refusal(credit(n, identity, body), 409, "shadow acceptance requires settled first trial")
    assert db_digest(n.store._db) == before


@pytest.mark.parametrize("fault", ["none", *IDENTITY_FAULTS])
def test_credit_of_a_banned_disputed_or_non_matching_identity_never_matures(
    shadow_acceptance: Any, fault: str
) -> None:
    # Given: a credited reservation, then the identity's standing turns bad before maturity
    n, identity, body = shadow_acceptance
    accepted = credit(n, identity, body).json()
    escrow = escrow_of(n)
    before = escrow.balances()
    spoil_identity(n, identity, fault)

    # When: the beacon reaches the reserved maturity round
    n.push(accepted["mature_at"])

    # Then: a clean identity vests, any other stays pending: neither spendable nor burned
    after = escrow.balances()
    vests = fault == "none"
    assert after.available == before.available + (UNIT if vests else 0)
    assert after.reward_pending == before.reward_pending - (UNIT if vests else 0)
    assert after.burned == before.burned and after.conserved()


# --- SHD-12, SHD-13: a restart does not re-issue ----------------------------------------------


@contextmanager
def served(store: ChallengeStore, root: Path) -> Iterator[tuple[TestClient, Path]]:
    """An app on `store` that knows the operator bearer, which lives in a file of `root`."""
    bearer_file = root / "operator-bearer-file"
    bearer_file.write_text("integration-admin")
    (root / "coord.key").write_bytes(bytes(range(32)))
    config = Config(
        "hypertrain",
        store.state_dir,
        "https://master.test",
        None,
        bearer_file,
        None,
        root / "coord.key",
        service.OWNER.ss58,
        store.params,
    )
    app = create_app(
        config,
        verify_beacon=store.verify_beacon,
        _store=store,
        clock=store.clock,
        transport=service.subnet(),
    )
    with TestClient(app) as client:
        yield client, bearer_file


@contextmanager
def restarted(n: Any, root: Path) -> Iterator[tuple[TestClient, ChallengeStore]]:
    """The service restarted on the state directory of `n`."""
    original = n.store
    original.close_v2_notifications()
    original._db.close()
    store = ChallengeStore(
        original.state_dir,
        original.params,
        service.COORD,
        service.OWNER.ss58,
        original.verify_beacon,
        original.objects,
    )
    store.clock = original.clock
    try:
        with served(store, root) as (client, _):
            yield client, store
    finally:
        store.close_v2_notifications()
        store._db.close()


def test_a_restart_between_acceptance_and_retry_returns_the_stored_response_and_issues_nothing(
    shadow_acceptance: Any, tmp_path: Path
) -> None:
    # Given: an accepted credit whose reply was lost, and a restart of the service
    n, identity, body = shadow_acceptance
    first = credit(n, identity, body)
    assert first.status_code == 200, first.text
    events = [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")]
    issued = escrow_of(n).balances().issued

    with restarted(n, tmp_path) as (client, store):
        # When: the very same acceptance is delivered to the restarted service
        retry = client.post(
            n.url + f"/admin/join/{identity}/shadow-reward", json=body, headers=service.admin()
        )

        # Then: the stored response comes back and the ledger is as it was
        assert retry.status_code == 200, retry.text
        assert retry.json() == first.json()
        assert [tuple(row) for row in store._db.execute("SELECT * FROM escrow_events")] == events
        reopened_escrow = store._services(n.manifest.run_id())[0]
        assert reopened_escrow.balances().issued == issued
        reopened_escrow.verify()


@pytest.mark.parametrize("lapsed", [False, pytest.param(True, marks=LOST_RESERVATION_STRANDED)])
def test_a_lost_reservation_reply_is_recovered_whatever_expiry_the_miner_chose(
    reserved_trial: Any, lapsed: bool
) -> None:
    # Given: a reservation, made while the miner's commit envelope was valid, and a lost reply
    n, identity, body = reserved_trial
    if lapsed:
        body["commit"] = n.signed(HOT[0], "CommitV2", body["commit"]["body"], expiry=n.now)
    first = reserve(n, identity, body)
    assert first.status_code == 200, first.text
    n.push(n.now + 1)

    # When: the operator delivers the identical request again, one beacon later
    retry = reserve(n, identity, body)

    # Then: the stored reservation comes back, not an expired-envelope refusal
    assert retry.status_code == 200, retry.text
    assert retry.json() == first.json()
    assert counts(n, "shadow-reservation") == 1


@pytest.fixture
def lapsing_commit(reserved_trial: Any) -> Any:
    """The miner's commit envelope, signed with the shortest expiry that still covers the
    reservation beacon. List it before `shadow_acceptance`: the reservation reads this body."""
    n, _, body = reserved_trial
    body["commit"] = n.signed(HOT[0], "CommitV2", body["commit"]["body"], expiry=n.now)
    return body["commit"]


def test_a_commit_envelope_that_lapses_after_the_reservation_cannot_strand_the_credit(
    lapsing_commit: Any, shadow_acceptance: Any
) -> None:
    # Given: a reservation made while the miner-chosen commit expiry still held, then a beacon
    # past it. The expiry is the miner's own choice, so it must not decide a later credit.
    n, identity, body = shadow_acceptance
    assert lapsing_commit["exp_drand"] == n.now
    n.push(n.now + 1)
    escrow = escrow_of(n)
    before = escrow.balances()

    # When: the credit is accepted and then reaches its maturity round
    accepted = credit(n, identity, body)
    assert accepted.status_code == 200, accepted.text
    n.push(accepted.json()["mature_at"])

    # Then: original custody was validated at the reservation beacon, so the credit vests
    after = escrow.balances()
    assert (after.issued, after.available) == (before.issued + UNIT, before.available + UNIT)


# --- SHD-24, SHD-06: the admin bearer, and no route to operator or runtime custody -----------

SHADOW_ROUTES = [
    Route("POST", "/v2/runs/{run_id}/admin/join/{admission_id}/shadow-reservation", "x"),
    Route("POST", "/v2/runs/{run_id}/admin/join/{admission_id}/shadow-reward", "x"),
]


def test_only_the_two_admin_routes_reach_shadow_credit_and_none_reaches_operator_custody(
    reserved_trial: Any,
) -> None:
    # Given
    n, _, _ = reserved_trial

    # When
    reaching = [
        (route.method, route.path)
        for route in routes(n.client.app)
        if any(word in route.path for word in ("shadow", "runtime", "custody", "operator"))
    ]

    # Then: exactly the reservation and the acceptance route, both behind the admin prefix
    assert reaching == [(route.method, route.path) for route in SHADOW_ROUTES]


CREDENTIALS = {
    "no authorization header": {},
    "a wrong bearer": {"Authorization": "Bearer not-the-operator-bearer"},
    "a basic scheme": {"Authorization": "Basic integration-admin"},
    "an empty bearer": {"Authorization": "Bearer "},
    "the bearer without its scheme": {"Authorization": "integration-admin"},
}


@pytest.mark.parametrize("credential", CREDENTIALS)
@pytest.mark.parametrize("route", ["reservation", "reward"])
def test_the_shadow_routes_need_the_operator_bearer_before_they_read_the_body(
    reserved_trial: Any, shadow_acceptance: Any, route: str, credential: str
) -> None:
    # Given: the valid body of each route (the reservation already exists, so a reservation sent
    # with the bearer would only return it), and a request that does not carry the bearer
    n, identity, reservation_body = reserved_trial
    send, valid = (
        (reserve, reservation_body) if route == "reservation" else (credit, shadow_acceptance[2])
    )
    before = db_digest(n.store._db)

    # When / Then: 401 before any parsing, so even a body that is no JSON gets the same answer
    for body in (valid, b"not json"):
        response = send(n, identity, body, headers=CREDENTIALS[credential])
        assert (response.status_code, response.json()) == (401, {"detail": "unauthorized"})
    assert db_digest(n.store._db) == before


@pytest.mark.parametrize("route", ["reservation", "reward"])
def test_the_shadow_routes_answer_503_while_the_operator_bearer_is_not_configured(
    shadow_acceptance: Any, tmp_path: Path, route: str
) -> None:
    # Given: an app whose operator bearer file is gone (it is read on every request)
    n, identity, body = shadow_acceptance
    send = reserve if route == "reservation" else credit
    with served(n.store, tmp_path) as (client, bearer_file):
        n.client = client
        assert send(n, identity, b"not json", headers=service.admin()).status_code != 503
        bearer_file.unlink()
        before = db_digest(n.store._db)

        # When / Then: not configured is 503, never an open route
        response = send(n, identity, body, headers=service.admin())
        assert response.status_code == 503, response.text
        assert db_digest(n.store._db) == before


@pytest.mark.parametrize("ledger", ["production", "cuda"])
def test_accepted_execution_custody_cannot_be_retained_outside_the_test_cpu_ledger(
    reserved_trial: Any, monkeypatch: pytest.MonkeyPatch, ledger: str
) -> None:
    # Given: a trial with retained custody, and an admission that is not a CPU test ledger (the
    # production one with a BLS-verified beacon, so the beacon gate cannot be what refuses)
    n, identity, body = reserved_trial
    admission, escrow = admission_of(n), escrow_of(n)
    if ledger == "production":
        monkeypatch.setattr(
            escrow, "policy", escrow.policy.model_copy(update={"ledger_mode": "production"})
        )
        beacon = admission.beacon
        monkeypatch.setattr(
            admission, "beacon", lambda now: replace(beacon(now), bls_verified=True)
        )
    else:
        monkeypatch.setattr(admission, "backend", "cuda")
    row = n.store._db.execute("SELECT * FROM admission_trial_results").fetchone()
    job = IslandJobV1.model_validate_json(n.store.objects.get(body["custody"]["job_hash"]))
    published = n.store._record_v2(
        n.manifest.run_id(), "shadow-execution", body["custody"]["miner_operation_hash"]
    )
    screen = WorkScreenV2.model_validate(parse_envelope(row["screen"]).body)
    proof = WorkProof.model_validate(parse_envelope(row["proof"]).body)
    before = db_digest(n.store._db)

    # When / Then: no writer of shadow-execution records works, and none was written
    with pytest.raises(AdmissionError) as refused, admission.escrow.tx():
        admission.retain_trial_publication(
            "probe",
            job,
            Path(published["directory"]).parent,
            screen,
            proof,
            n.now,
            commit_raw=canonicalize(body["commit"]),
        )
    assert refused.value.code == "SHADOW_ACCEPTED_RUNTIME_CUSTODY_UNAVAILABLE"
    assert db_digest(n.store._db) == before


# --- SHD-10: production credit needs the owner-signed operator authority ----------------------


def production_ledger(n: Any, monkeypatch: pytest.MonkeyPatch, authority: dict[str, Any]) -> None:
    escrow = escrow_of(n)
    monkeypatch.setattr(
        escrow, "policy", escrow.policy.model_copy(update={"ledger_mode": "production"})
    )
    monkeypatch.setattr(n.store, "_shadow_operator_v2", lambda _run_id, historical=False: authority)


def operator_authority(n: Any, *, roster: bool, expires: int) -> dict[str, Any]:
    row = n.store._db.execute("SELECT hotkey,coldkey FROM admissions_v2").fetchone()
    identities = [{"hotkey": row["hotkey"], "coldkey": row["coldkey"], "role": "miner"}]
    return {
        "workload": {"identities": identities if roster else []},
        "expires_beacon": expires,
        "hash": "aa" * 32,
        "graph_hash": "ab" * 32,
        "source_map_hash": "ac" * 32,
        "hosts": {"miner": {}},
        "cutoff_unix": 2_000_000_000,
    }


OPERATOR_FAULTS = {
    "the identity is not in the signed roster": (False, 10**6, "operator reservation roster/time"),
    "the trial finalized after the authority expired": (
        True,
        1,
        "operator reservation roster/time",
    ),
    "the retained executions carry no operator fields": (
        True,
        10**6,
        "operator original execution custody differs",
    ),
}


@pytest.mark.parametrize("fault", OPERATOR_FAULTS)
def test_a_production_reservation_is_refused_unless_the_operator_authority_covers_it(
    reserved_trial: Any, monkeypatch: pytest.MonkeyPatch, fault: str
) -> None:
    # Given: a production ledger whose owner-signed operator record is wrong in one way
    n, identity, body = reserved_trial
    roster, expires, detail = OPERATOR_FAULTS[fault]
    production_ledger(n, monkeypatch, operator_authority(n, roster=roster, expires=expires))
    before = db_digest(n.store._db)

    # When / Then
    refusal(reserve(n, identity, body), 403, detail)
    assert db_digest(n.store._db) == before


def test_a_production_reservation_without_any_operator_record_is_refused(
    reserved_trial: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: a production ledger and no owner-signed operator record at all
    n, identity, body = reserved_trial
    escrow = escrow_of(n)
    monkeypatch.setattr(
        escrow, "policy", escrow.policy.model_copy(update={"ledger_mode": "production"})
    )
    before = db_digest(n.store._db)

    # When / Then: fail closed
    refusal(reserve(n, identity, body), 409, "missing accepted shadow-operator")
    assert db_digest(n.store._db) == before


def test_a_test_ledger_reservation_binds_the_policy_digest_not_an_operator_record(
    reserved_trial: Any,
) -> None:
    # Given / When
    n, identity, body = reserved_trial
    reservation = reserve(n, identity, body).json()["reservation"]

    # Then: the synthetic test authority is the economics policy itself
    assert reservation["economic_admission_hash"] == escrow_of(n).policy.digest()


# --- SHD-19, SHD-21, SHD-20: a poisoned origin, a hotkey rotation, a next trial ---------------


def rotate(n: Any) -> httpx.Response:
    return rotate_admission(n, COLD[0], HOT[0], REFEREE)


# SHD-19+SHD-D3 (F6): one origin whose original custody no longer validates never rolls back the
# maturity of the run's other origins.
DRIFT = [
    "none",
    "the shadow-custody record is missing",
    "a retained publication changed",
    "the admission's hotkey was rotated",
]


def drift_custody(n: Any, body: dict[str, Any], fault: str) -> None:
    if fault == "the shadow-custody record is missing":
        n.store._db.execute("DELETE FROM records_v2 WHERE kind='shadow-custody'")
    elif fault == "a retained publication changed":
        reference = n.store._record_v2(
            n.manifest.run_id(),
            "shadow-execution",
            body["auditor_receipt"]["body"]["reference_operation_hash"],
        )
        (Path(reference["directory"]) / "rank-0/delta.bin").write_bytes(b"moved or edited later")
    elif fault == "the admission's hotkey was rotated":
        rotate(n)


@pytest.mark.parametrize("fault", DRIFT)
def test_an_origin_with_unvalidatable_custody_does_not_stop_the_other_due_origins(
    shadow_acceptance: Any, fault: str
) -> None:
    # Given: a credited shadow origin and another origin due at the same beacon
    n, identity, body = shadow_acceptance
    accepted = credit(n, identity, body).json()
    escrow = escrow_of(n)
    add_due_origin(escrow, COLD[1].ss58, 500, accepted["mature_at"])
    before = escrow.balances()
    drift_custody(n, body, fault)

    # When: the beacon reaches their common maturity round
    response = push_round(n, accepted["mature_at"])

    # Then: the beacon is accepted and the independent origin vests; the shadow origin vests when
    # its custody still validates and otherwise stays pending (after a rotation either is right:
    # the evidence is judged against the frozen identity, or it is held)
    assert response.status_code == 200, response.text
    after = escrow.balances()
    vested = after.available - before.available - 500
    assert vested in ({UNIT} if fault == "none" else {0, UNIT} if "rotated" in fault else {0})
    assert after.reward_pending == before.reward_pending - 500 - vested
    assert after.conserved()


ROTATION = [False, pytest.param(True, marks=ROTATION_STRANDS)]


@pytest.mark.parametrize("rotated", ROTATION)
def test_a_reservation_made_before_a_hotkey_rotation_can_still_be_credited(
    shadow_acceptance: Any, rotated: bool
) -> None:
    # Given: a reservation, and the owner rotating the admission to another registered hotkey
    n, identity, body = shadow_acceptance
    if rotated:
        rotate(n)  # a service that refuses the rotation instead is also a defence
    before = escrow_of(n).balances()

    # When
    response = credit(n, identity, body)

    # Then: the credit goes to the reserved coldkey
    assert response.status_code == 200, response.text
    assert escrow_of(n).balances().issued == before.issued + UNIT


@pytest.mark.parametrize("rotated", ROTATION)
def test_a_lost_response_is_recovered_after_a_hotkey_rotation(
    shadow_acceptance: Any, rotated: bool
) -> None:
    # Given: an accepted credit, a lost reply, and a rotation of the admission's hotkey
    n, identity, body = shadow_acceptance
    first = credit(n, identity, body)
    assert first.status_code == 200, first.text
    if rotated:
        rotate(n)  # a service that refuses the rotation instead is also a defence

    # When
    retry = credit(n, identity, body)

    # Then
    assert retry.status_code == 200, retry.text
    assert retry.json() == first.json()


@pytest.mark.parametrize("rotated", ROTATION)
def test_a_credit_still_matures_after_a_hotkey_rotation(
    shadow_acceptance: Any, rotated: bool
) -> None:
    # Given: an accepted credit and a rotation of the admission's hotkey
    n, identity, body = shadow_acceptance
    accepted = credit(n, identity, body).json()
    escrow = escrow_of(n)
    before = escrow.balances()
    if rotated:
        rotate(n)  # a service that refuses the rotation instead is also a defence

    # When: the beacon reaches the reserved maturity round
    response = push_round(n, accepted["mature_at"])

    # Then
    assert response.status_code == 200, response.text
    assert escrow.balances().available == before.available + UNIT


@pytest.mark.parametrize("second_trial", [False, pytest.param(True, marks=NEXT_TRIAL_STRANDS)])
def test_a_reserved_credit_is_not_lost_when_the_admission_finalizes_a_next_trial(
    shadow_acceptance: Any, second_trial: bool
) -> None:
    # Given: a reservation whose trial is still the admission's first finalized MATCH, and (the
    # row a second finalized trial leaves behind) a clean count of two
    n, identity, body = shadow_acceptance
    db = n.store._db
    epoch = body["reward"]["w"]
    assert (
        db.execute(
            "SELECT outcome FROM admission_trial_results WHERE epoch=?", (epoch,)
        ).fetchone()[0]
        == "MATCH"
    )
    assert (
        db.execute(
            "SELECT MIN(epoch) FROM admission_trials WHERE finalized_beacon IS NOT NULL"
        ).fetchone()[0]
        == epoch
    )
    if second_trial:
        db.execute("UPDATE admissions_v2 SET clean_count=2 WHERE admission_id=?", (identity,))
    before = escrow_of(n).balances()

    # When
    response = credit(n, identity, body)

    # Then: the reservation is credited rather than left to hold the single pending slot
    assert response.status_code == 200, response.text
    assert escrow_of(n).balances().issued == before.issued + UNIT


# --- SHD-23: staged trial inputs are immutable per nonce --------------------------------------


@pytest.fixture
def trial(network: Any) -> tuple[Any, str]:
    joined = network.join()
    assert joined.status_code == 200, joined.text
    identity = joined.json()["admission_id"]
    network.push(network.now + 1)
    opened = open_challenge(network, identity)
    assert opened.status_code == 200, opened.text
    return network, identity


def export(n: Any, identity: str) -> httpx.Response:
    return n.client.post(n.url + f"/admin/trial/{identity}/export", headers=service.admin())


@pytest.mark.parametrize("name", ["start_state", "ef_in", "v0", "samples", "sample_proofs"])
def test_staged_trial_inputs_are_created_once_and_never_overwritten(
    trial: tuple[Any, str], name: str
) -> None:
    # Given: an exported trial, and one staged input that someone else rewrote
    n, identity = trial
    first = export(n, identity)
    assert first.status_code == 200, first.text
    directory = n.store.state_dir / "trials-v2" / first.json()["nonce"]
    assert export(n, identity).json() == first.json(), "an untouched restage is the same answer"
    job = (directory / "job.json").read_bytes()
    (directory / name).write_bytes(b"rewritten by another writer")

    # When / Then: the export is refused and neither the input nor the job is rewritten
    refusal(export(n, identity), 409, "immutable trial inputs changed")
    assert (directory / name).read_bytes() == b"rewritten by another writer"
    assert (directory / "job.json").read_bytes() == job


def test_the_reference_the_service_runs_retains_no_accepted_execution_by_default(
    trial: tuple[Any, str],
) -> None:
    # Given: an open trial, as every real application has
    n, identity = trial
    before = counts(n, "shadow-execution")

    # When: the service runs the trusted reference through its route (a real CPU execution)
    response = n.client.post(n.url + f"/admin/join/{identity}/reference", headers=service.admin())

    # Then: the reference is committed, and no route-driven flow created accepted custody:
    # only an in-process trusted seam that asks for it writes shadow-execution records
    assert response.status_code == 200, response.text
    row = n.store._db.execute(
        "SELECT reference_commitment FROM admissions_v2 WHERE admission_id=?", (identity,)
    ).fetchone()
    assert row["reference_commitment"] == response.json()["reference_commitment"]
    assert before == counts(n, "shadow-execution") == 0
