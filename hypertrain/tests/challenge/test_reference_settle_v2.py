"""The three steps of a trusted reference (docs/adr/0012 section 2): claim, run, settle.

The run reads no database, because the authority connection is shared and a read from a thread that
holds no lock could see another thread's uncommitted rows. The settlement writes only into the state
its claim read: whatever moved while the launch ran, the result is dropped and nothing is written.
`ChallengeStore.trial_reference_v2` puts the lock around the first and the last step only
(tests/challenge/test_reference_lock_v2.py); these tests drive the steps on `Admission` itself.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from hypertrain.challenge.admission import ReferenceClaim
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.ledger.escrow_v2 import digest

from . import test_admission_v2 as fx


@pytest.fixture(scope="module")
def reference(tmp_path_factory: pytest.TempPathFactory) -> Any:
    """One claimed and run reference; each test works inside a transaction that is rolled back."""
    s = fx.setup()
    a = fx.service(s, tmp_path_factory.mktemp("settle"))
    admission = fx.apply(a, s)
    a.challenge(admission, now=2)
    claim = a.claim_reference(admission, now=2)
    assert isinstance(claim, ReferenceClaim)
    return SimpleNamespace(
        a=a, admission=admission, claim=claim, result=a.run_reference(claim, now=2)
    )


@pytest.fixture
def within(reference: Any) -> Any:
    reference.a.store.db.execute("BEGIN IMMEDIATE")
    yield reference
    reference.a.store.db.execute("ROLLBACK")


def written(r: Any) -> tuple[Any, ...]:
    db = r.a.store.db
    row = db.execute(
        "SELECT reference_commitment,reference_json,reference_beacon FROM admissions_v2 "
        "WHERE admission_id=?",
        (r.admission,),
    ).fetchone()
    trial = db.execute(
        "SELECT reference FROM admission_trial_results WHERE epoch=?", (r.claim.trial_epoch,)
    ).fetchone()
    return (*row, trial[0])


def settle(r: Any, *, now: int = 2, result: Any = None) -> str:
    return r.a.settle_reference(r.claim, r.result if result is None else result, now=now)


def test_the_run_reads_no_database_and_asks_for_no_qualification(tmp_path: Path) -> None:
    s = fx.setup()
    a = fx.service(s, tmp_path)
    admission = fx.apply(a, s)
    a.challenge(admission, now=2)
    claim = a.claim_reference(admission, now=2)
    assert isinstance(claim, ReferenceClaim)

    class NoDatabase:
        def execute(self, *args: Any, **kwargs: Any) -> Any:
            raise AssertionError(f"the run read the database: {args[:1]}")

    def refused(*args: Any) -> Any:
        raise AssertionError("the run asked the store, which needs its lock")

    real = (a.store.db, a.escrow.db, a.qualified, a.beacon)
    a.store.db = a.escrow.db = NoDatabase()  # type: ignore[assignment]
    a.qualified, a.beacon = refused, refused
    try:
        result = a.run_reference(claim, now=2)
    finally:
        a.store.db, a.escrow.db, a.qualified, a.beacon = real
    assert a.settle_reference(claim, result, now=2) == digest(result.proof)


@pytest.mark.parametrize(
    "other",
    [
        # derived from the real job: the samples of a trial follow a random nonce, and a fixed
        # "other" list was the real one in about one run of eight
        lambda job: {"sample_ids": [sample + 1 for sample in job.sample_ids]},
        lambda job: {"w": job.w + 99},
    ],
    ids=["other samples", "other trial epoch"],
)
def test_a_staging_for_another_assignment_never_reaches_the_launch(
    tmp_path: Path, other: Callable[[Any], dict[str, Any]]
) -> None:
    s = fx.setup()
    a = fx.service(s, tmp_path)
    admission = fx.apply(a, s)
    a.challenge(admission, now=2)
    claim = a.claim_reference(admission, now=2)
    assert isinstance(claim, ReferenceClaim)
    job, directory = claim.stage()
    update = other(job)
    assert all(getattr(job, name) != value for name, value in update.items())
    wrong = replace(claim, stage=lambda: (job.model_copy(update=update), directory))

    def launched(*args: Any) -> Any:
        raise AssertionError("the job reached the launch")

    with pytest.raises(AdmissionError, match="REFERENCE_ASSIGNMENT"):
        a.run_reference(wrong, now=2, trusted_launch=launched)


def test_the_claim_of_an_open_trial_that_has_its_commitment_runs_nothing(tmp_path: Path) -> None:
    s = fx.setup()
    a = fx.service(s, tmp_path)
    admission = fx.apply(a, s)
    a.challenge(admission, now=2)
    commitment = a.prepare_reference(admission, now=2)

    def never(*args: Any) -> Any:
        raise AssertionError("a stored commitment needs no job")

    a.stage_reference = never  # type: ignore[assignment]
    assert a.claim_reference(admission, now=3) == commitment


def test_a_settlement_commits_at_its_own_round_and_a_repeat_changes_nothing(within: Any) -> None:
    commitment = settle(within, now=5)
    assert commitment == digest(within.result.proof)
    stored = written(within)
    assert stored[0] == commitment and stored[2] == 5
    assert settle(within, now=7) == commitment
    assert written(within) == stored  # the round of the first commitment stays


def test_a_settlement_that_finds_its_own_commitment_still_retains_the_custody_it_is_asked_for(
    within: Any,
) -> None:
    db = within.a.store.db
    shadow = "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-execution'"
    within.a.settle_reference(within.claim, within.result, now=5, retain_shadow_custody=True)
    assert db.execute(shadow).fetchone()[0] == 1
    db.execute("DELETE FROM records_v2 WHERE kind='shadow-execution'")
    within.a.settle_reference(within.claim, within.result, now=6, retain_shadow_custody=True)
    assert db.execute(shadow).fetchone()[0] == 1  # the same trial, a second flight asked for it


@pytest.mark.parametrize(
    "tamper",
    [
        lambda r: replace(r, proof=r.proof.model_copy(update={"admission_id": "ab" * 32})),
        lambda r: replace(r, proof=r.proof.model_copy(update={"challenge_hash": "ab" * 32})),
        lambda r: replace(r, screen=r.screen.model_copy(update={"admission_id": "ab" * 32})),
        lambda r: replace(r, screen=r.screen.model_copy(update={"challenge_hash": "ab" * 32})),
        lambda r: replace(r, screen=r.screen.model_copy(update={"nonce": "ab" * 32})),
    ],
    ids=["proof-admission", "proof-challenge", "screen-admission", "screen-challenge", "nonce"],
)
def test_a_result_bound_to_another_challenge_is_refused(within: Any, tamper: Any) -> None:
    before = written(within)
    with pytest.raises(AdmissionError, match="REFERENCE_RESULT_BINDING"):
        settle(within, result=tamper(within.result))
    assert written(within) == before


def moves_to_a_new_challenge(r: Any) -> None:
    r.a.suspend(fx.HOT.ss58, reason="infrastructure", now=3)
    r.a.resume(fx.HOT.ss58, now=4)
    r.a.challenge(r.admission, now=5)


def update(sql: str) -> Any:
    return lambda r: r.a.store.db.execute(sql, (r.admission,))


@pytest.mark.parametrize(
    "move",
    [
        moves_to_a_new_challenge,
        lambda r: r.a.suspend(fx.HOT.ss58, reason="infrastructure", now=3),
        update("UPDATE admissions_v2 SET pending_dispute=1 WHERE admission_id=?"),
        update("UPDATE admissions_v2 SET proof_digest='aa' WHERE admission_id=?"),
        update("UPDATE admissions_v2 SET trial_epoch=trial_epoch+1 WHERE admission_id=?"),
        update("UPDATE admissions_v2 SET reference_commitment='ff' WHERE admission_id=?"),
        update("UPDATE admissions_v2 SET challenge=NULL WHERE admission_id=?"),
    ],
    ids=["new-challenge", "suspended", "dispute", "proof", "epoch", "other-commitment", "cleared"],
)
def test_a_result_for_a_row_that_moved_while_the_launch_ran_is_dropped(
    within: Any, move: Any
) -> None:
    move(within)
    before = written(within)
    with pytest.raises(AdmissionError, match="REFERENCE_STATE_CHANGED"):
        settle(within, now=6)
    assert written(within) == before


def test_a_result_for_a_closed_trial_is_dropped(within: Any) -> None:
    within.a.store.db.execute(
        "UPDATE admission_trial_results SET outcome='EXPIRED' WHERE epoch=?",
        (within.claim.trial_epoch,),
    )
    before = written(within)
    with pytest.raises(AdmissionError, match="REFERENCE_TRIAL_CLOSED"):
        settle(within)
    assert written(within) == before


def test_a_result_that_arrives_after_the_deadline_round_is_dropped(within: Any) -> None:
    deadline = within.claim.challenge.deadline_beacon
    before = written(within)
    with pytest.raises(AdmissionError, match="REFERENCE_TRIAL_CLOSED"):
        settle(within, now=deadline + 1)  # the reaper has not run yet; the window is over
    assert written(within) == before
    assert settle(within, now=deadline)  # the deadline round itself is still inside the window


def test_a_result_the_run_could_not_qualify_is_dropped(within: Any, monkeypatch: Any) -> None:
    monkeypatch.setattr(within.a, "qualified", lambda screen: False)
    before = written(within)
    with pytest.raises(AdmissionError, match="UNQUALIFIED_REFERENCE"):
        settle(within)
    assert written(within) == before
