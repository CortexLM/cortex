"""Real shadow trajectories plus durable identity, outbox and probation negatives."""

from __future__ import annotations

import importlib
import json
import sys
from dataclasses import replace
from pathlib import Path

import pytest

from hypertrain.beacon.core import FixtureBeacon
from hypertrain.challenge.admission import Admission
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.data.store import LocalFSStore, get_large
from hypertrain.ledger.escrow_v2 import Settlement, digest
from hypertrain.miner.admission import sign_join, sign_proof, sign_rotation
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    EscrowLock,
    HardwareHint,
    JoinChallenge,
    JoinRequest,
    RotateRequest,
    RunManifestV2,
    WorkProof,
    WorkScreenV2,
)

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ledger"))
fixtures = importlib.import_module("test_escrow_v2")
COLD, COORD, HOT, OTHER, H = fixtures.COLD, fixtures.COORD, fixtures.HOT, fixtures.OTHER, fixtures.H
Setup, fund, ledger, setup, stage = (
    fixtures.Setup,
    fixtures.fund,
    fixtures.ledger,
    fixtures.setup,
    fixtures.stage,
)


def service(s: Setup, path: Path) -> Admission:
    e = ledger(s, path / "ledger.db")

    def reference(c: JoinChallenge, samples: tuple[int, ...], epoch: int) -> tuple:
        directory = path / f"reference-{c.nonce}"
        return stage(s, directory, samples, epoch), directory

    return Admission(
        e,
        s.admission_policy,
        COORD,
        beacon=FixtureBeacon(current=100000).get,
        stage_reference=reference,
        objects=LocalFSStore(path / "objects"),
        ip_secret=b"private-prefix-key",
        qualified=lambda _: True,
        conservative_bound=lambda: True,
        backend="cpu",
    )


def request(s: Setup, request_id: str = H, hot=HOT, cold=COLD) -> JoinRequest:
    return sign_join(
        hot,
        cold,
        run_id=s.manifest.run_id(),
        request_id=request_id,
        expires_beacon=10000,
        policy_hash=digest(s.admission_policy),
        hardware_hint=HardwareHint(device_name="advisory", device_count=999, driver="cpu-test"),
    )


def apply(a: Admission, s: Setup, now: int = 1) -> str:
    return a.join(canonicalize(request(s).body()), now=now, ip_prefix="loopback").admission_id


def current_proof(a: Admission, admission_id: str, now: int) -> tuple[WorkProof, WorkScreenV2]:
    row = a.store.db.execute(
        "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
    ).fetchone()
    assert row is not None and row["reference_json"] is not None
    proof = WorkProof.model_validate_json(row["reference_json"])
    c = JoinChallenge.model_validate_json(row["challenge"])
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.gpu_ops.work_screen import screen_work

    samples = trial_samples(a.escrow.manifest, admission_id, c.nonce, a.beacon(c.seed_beacon))
    job, directory = a.stage_reference(c, samples, row["trial_epoch"])
    import shutil

    miner_directory = directory.with_name("miner-" + c.nonce)
    miner_directory.mkdir(exist_ok=True)
    for relative in job.object_paths.values():
        shutil.copyfile(directory / relative, miner_directory / relative)
    directory = miner_directory
    screen, actual = screen_work(job, directory, c, now_beacon=now, backend="cpu")
    assert actual == proof
    return proof, screen


def submit(a: Admission, proof: WorkProof, screen: WorkScreenV2, now: int = 2):
    return a.proof(
        canonicalize(sign_proof(HOT, a.escrow.run_id, proof, 500)),
        canonicalize(seal(HOT, "WorkScreenV2", a.escrow.run_id, screen, 500)),
        now=now,
    )


def test_join_is_idempotent_when_restart(tmp_path: Path) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    # When
    restored = service(s, tmp_path)
    same = restored.join(canonicalize(request(s).body()), now=1, ip_prefix="different")
    # Then
    assert same.admission_id == admission and same.state == "APPLIED"
    assert restored.store.db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 1
    assert restored.status(HOT.ss58, now=1).shadow_only


@pytest.mark.parametrize("fault", ["hot_sig", "cold_sig", "run_id", "late", "conflict", "coldkey"])
def test_join_rejects_when_auth_or_replay_wrong(tmp_path: Path, fault: str) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    apply(a, s)
    req = request(s)
    if fault in ("hot_sig", "cold_sig", "run_id"):
        req = JoinRequest.model_validate(
            {**req.body(), fault: "0" * 128 if fault.endswith("sig") else H}
        )
    if fault == "conflict":
        req = request(s, hot=OTHER)
    if fault == "coldkey":
        req = request(s, "22" * 32, hot=OTHER)
    # When / Then
    with pytest.raises(AdmissionError):
        a.join(canonicalize(req.body()), now=10001 if fault == "late" else 1, ip_prefix="loopback")
    assert a.store.db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 1


def test_quota_rejects_when_exhausted_same_beacon(tmp_path: Path) -> None:
    # Given
    a = service(setup(), tmp_path)
    with a.escrow.tx():
        for _ in range(4):
            a.store.quota("cold", 1, 2, 4)
    # When / Then
    with a.escrow.tx(), pytest.raises(AdmissionError):
        a.store.quota("cold", 1, 2, 4)


def test_trial_capacity_rejects_third_shadow_reference(tmp_path: Path) -> None:
    from hypertrain.protocol.keys import Keypair

    # Given
    s = setup()
    a = service(s, tmp_path)
    ids = []
    for i in range(3):
        k = Keypair(bytes([30 + i]) * 32)
        r = request(s, f"{i + 1:02x}" * 32, k, k)
        ids.append(a.join(canonicalize(r.body()), now=1, ip_prefix="loopback").admission_id)
    a.challenge(ids[0], now=2)
    a.challenge(ids[1], now=2)
    # When / Then
    with pytest.raises(AdmissionError, match="TRIAL_CAPACITY"):
        a.challenge(ids[2], now=2)
    assert a.store.db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0] == 2


def test_challenge_nonce_and_signed_receipt_survive_restart(tmp_path: Path) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    first = a.challenge(admission, now=2)
    # When
    restored = service(s, tmp_path)
    second = restored.challenge(admission, now=3)
    # Then: original signature/receipt, not merely a matching regenerated body.
    assert first == second
    assert restored.store.db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0] == 1


def test_resume_rejects_illegal_applied_transition(tmp_path: Path) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    apply(a, s)
    # When / Then
    with pytest.raises(AdmissionError):
        a.resume(HOT.ss58, now=2)
    assert a.store.record(HOT.ss58).state == "APPLIED"


def test_resume_requires_fresh_challenge_without_erasing_history(tmp_path: Path) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    first = a.challenge(admission, now=2)
    a.suspend(HOT.ss58, reason="infrastructure", now=2)
    # When
    a.resume(HOT.ss58, now=3)
    second = a.challenge(admission, now=4)
    # Then: old nonce cannot masquerade as fresh probation.
    assert first["body"] != second["body"]
    assert a.store.record(HOT.ss58).clean_count == 0


def applied(a: Admission, s: Setup, index: int, now: int) -> str:
    """An APPLIED identity of its own keys (hot == cold), as the capacity tests use."""
    from hypertrain.protocol.keys import Keypair

    key = Keypair(bytes([30 + index]) * 32)
    request_id = f"{index + 1:02x}" * 32
    joined = a.join(canonicalize(request(s, request_id, key, key).body()), now=now, ip_prefix="x")
    return joined.admission_id


def open_trials(a: Admission) -> int:
    return a.store.db.execute(
        "SELECT COUNT(*) FROM admission_trial_results WHERE outcome='OPEN'"
    ).fetchone()[0]


def test_reaper_closes_only_unproven_trials_past_deadline_and_penalises_nobody(
    tmp_path: Path,
) -> None:
    # Given: two trials issued at round 2 (deadline 102) and never proved
    s = setup()
    a = service(s, tmp_path)
    holders = [applied(a, s, i, 1) for i in range(2)]
    for holder in holders:
        a.challenge(holder, now=2)
    # When / Then: the deadline round still accepts a proof, so nothing closes at 102
    assert a.reap_expired_trials(now=102) == [] and open_trials(a) == 2
    assert a.reap_expired_trials(now=103) == [0, 1] and open_trials(a) == 0
    assert [
        row[0]
        for row in a.store.db.execute("SELECT outcome FROM admission_trial_results ORDER BY epoch")
    ] == ["EXPIRED", "EXPIRED"]
    # Expiry is bookkeeping, not a sanction: state, strikes and seed are untouched, and the
    # closed trial is not taken again (one trial per seed).
    for holder in holders:
        record = a.store.by_id(holder)
        assert (record.state, record.strikes, record.clean_count) == ("APPLIED", 0, 0)
    assert a.store.db.execute("SELECT seed_beacon FROM admissions_v2").fetchall()[0][0] == 2
    with pytest.raises(AdmissionError, match="CHALLENGE_EXPIRED"):
        a.challenge(holders[0], now=104)
    assert a.store.db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0] == 2


@pytest.mark.parametrize("state", ["APPLIED", "ACTIVE"])
def test_challenge_issued_when_two_unproven_trials_are_past_their_deadline(
    tmp_path: Path, state: str
) -> None:
    # Given: two registered identities open a trial each and never prove it. The Lens
    # starvation: they are still holding both places thousands of rounds later.
    s = setup()
    a = service(s, tmp_path)
    holders = [applied(a, s, i, 1) for i in range(2)]
    for holder in holders:
        a.challenge(holder, now=2)
        a.store.transition(holder, state, "TEST", 2)  # a lapsed re-screen of an ACTIVE miner
    newcomer = applied(a, s, 2, 5000)
    # When: the newcomer asks inside its own window
    envelope = a.challenge(newcomer, now=5001)
    # Then: it is served, and nobody was suspended for the trial they left open (an unfunded
    # application that lapsed is retired by the join itself, which is another rule)
    assert envelope["type"] == "JoinChallenge"
    assert "CHALLENGE_EXPIRED" not in [
        row[0] for row in a.store.db.execute("SELECT reason FROM admission_transitions")
    ]
    if state == "ACTIVE":
        assert [a.store.by_id(h).state for h in holders] == [state, state]


def sybil_join(a: Admission, s: Setup, index: int, now: int, prefix: str = "203.0.113.0/24"):
    """A registered identity with keys of its own that never funds its application."""
    from hypertrain.protocol.keys import Keypair

    request_id = f"{index + 1:02x}" * 32
    hot, cold = Keypair(bytes([100 + index]) * 32), Keypair(bytes([140 + index]) * 32)
    return a.join(canonicalize(request(s, request_id, hot, cold).body()), now=now, ip_prefix=prefix)


def test_unfunded_applied_rows_release_their_places_once_the_window_is_dead(
    tmp_path: Path,
) -> None:
    # Given: the Lens flood. 32 unfunded joins from one /24 at round 1 (seed 2, window to 102).
    s = setup()
    a = service(s, tmp_path)
    sybils = [sybil_join(a, s, i, 1).admission_id for i in range(32)]
    honest = request(s, "ee" * 32, OTHER, COLD)
    # When / Then: while the window is open the places are really taken. The refused join
    # rolls back, so it reaps nothing at round 102, where challenge() could still serve.
    with pytest.raises(AdmissionError, match="PENDING_CAPACITY"):
        a.join(canonicalize(honest.body()), now=102, ip_prefix="198.51.100.0/24")
    assert a.store.db.execute("SELECT COUNT(*) FROM admission_transitions").fetchone()[0] == 0
    # Once the window is over the rows are dead: an honest join (even with the coldkey of a
    # sybil) takes a place, and the 32 rows are suspended, not deleted.
    joined = a.join(canonicalize(honest.body()), now=103, ip_prefix="198.51.100.0/24")
    assert joined.state == "APPLIED"
    rows = {
        r[0]: (r[1], r[2])
        for r in a.store.db.execute("SELECT admission_id,state,reason FROM admissions_v2")
    }
    assert {rows[i] for i in sybils} == {("SUSPENDED", "APPLIED_EXPIRED")}
    assert a.store.db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 33
    assert (
        a.store.db.execute(
            "SELECT COUNT(*) FROM admission_transitions WHERE reason='APPLIED_EXPIRED'"
        ).fetchone()[0]
        == 32
    )
    # Their hotkeys stay spent: the application is dead, the identity is not recycled.
    from hypertrain.protocol.keys import Keypair

    reused = request(s, "f0" * 32, Keypair(bytes([100]) * 32), Keypair(b"\x5a" * 32))
    with pytest.raises(AdmissionError, match="HOTKEY_HISTORY_EXISTS"):
        a.join(canonicalize(reused.body()), now=104, ip_prefix="elsewhere")


def test_funded_or_outbox_pending_applied_row_is_never_reaped(tmp_path: Path) -> None:
    # Given: two applications past their window that hold stake: one locked, one whose lock
    # is only in the outbox (crash before the ledger commit).
    from hypertrain.protocol.keys import Keypair

    s = setup()
    a = service(s, tmp_path)
    funded = apply(a, s)
    origin, _ = fund(a.escrow)
    op = EscrowLock(
        operation_id="22" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=funded,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    a.lock(canonicalize(seal(COLD, "EscrowLock", a.escrow.run_id, op, 500)), now=1)
    queued_hot = Keypair(b"\x31" * 32)
    queued = a.join(
        canonicalize(request(s, "33" * 32, queued_hot, OTHER).body()), now=1, ip_prefix="queued"
    ).admission_id
    pending = EscrowLock(
        operation_id="44" * 32,
        owner=OTHER.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=queued,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    a.store.enqueue_lock(pending, OTHER.ss58)
    # When: a later join looks for dead applications
    a.join(
        canonicalize(request(s, "55" * 32, Keypair(b"\x32" * 32), Keypair(b"\x33" * 32)).body()),
        now=500,
        ip_prefix="later",
    )
    # Then: neither is touched, and the funded coldkey still holds its one application
    assert [a.store.by_id(i).state for i in (funded, queued)] == ["APPLIED", "APPLIED"]
    assert a.store.db.execute("SELECT COUNT(*) FROM admission_transitions").fetchone()[0] == 0
    with pytest.raises(AdmissionError, match="COLDKEY_PROBATION_EXISTS"):
        a.join(
            canonicalize(request(s, "66" * 32, Keypair(b"\x34" * 32), COLD).body()),
            now=501,
            ip_prefix="x",
        )


def test_reaped_coldkey_can_apply_again_but_other_suspensions_still_block_it(
    tmp_path: Path,
) -> None:
    from hypertrain.protocol.keys import Keypair

    s = setup()
    a = service(s, tmp_path)
    # Given: HOT/COLD applied and never fetched its challenge; OTHER's coldkey was suspended by
    # an operator.
    apply(a, s)
    suspended = a.join(
        canonicalize(request(s, "77" * 32, Keypair(b"\x35" * 32), OTHER).body()),
        now=1,
        ip_prefix="y",
    ).admission_id
    a.suspend(a.store.by_id(suspended).hotkey, reason="OPERATOR", now=2)
    # When: both coldkeys apply again after the window
    again = a.join(
        canonicalize(request(s, "88" * 32, Keypair(b"\x36" * 32), COLD).body()),
        now=103,
        ip_prefix="z",
    )
    # Then: the lapsed application no longer blocks its coldkey; the operator's suspension does
    assert again.state == "APPLIED" and again.coldkey == COLD.ss58
    with pytest.raises(AdmissionError, match="COLDKEY_PROBATION_EXISTS"):
        a.join(
            canonicalize(request(s, "99" * 32, Keypair(b"\x37" * 32), OTHER).body()),
            now=104,
            ip_prefix="w",
        )
    # and a lapsed application whose trial was issued but never proved is dead the same way
    stale = a.join(
        canonicalize(request(s, "aa" * 32, Keypair(b"\x38" * 32), Keypair(b"\x39" * 32)).body()),
        now=110,
        ip_prefix="v",
    ).admission_id
    a.challenge(stale, now=111)
    a.join(
        canonicalize(request(s, "bb" * 32, Keypair(b"\x3a" * 32), Keypair(b"\x3b" * 32)).body()),
        now=300,
        ip_prefix="u",
    )
    assert (a.store.by_id(stale).state, a.store.by_id(stale).reason) == (
        "SUSPENDED",
        "APPLIED_EXPIRED",
    )
    assert open_trials(a) == 0


def capped_setup(cap: int) -> Setup:
    """The test setup with the optional per-prefix cap on unfunded applications."""
    base = setup()
    policy = base.admission_policy.model_copy(update={"max_unfunded_per_prefix": cap})
    body = base.manifest.body()
    body["network"]["admission_policy_hash"] = digest(policy)
    return replace(base, manifest=RunManifestV2.model_validate(body), admission_policy=policy)


def join_from(a: Admission, s: Setup, index: int, now: int, prefix: str, cold=None):
    from hypertrain.protocol.keys import Keypair

    hot = HOT if index == 0 else Keypair(bytes([100 + index]) * 32)
    cold = cold or (COLD if index == 0 else Keypair(bytes([140 + index]) * 32))
    request_id = f"{index + 1:02x}" * 32
    return a.join(canonicalize(request(s, request_id, hot, cold).body()), now=now, ip_prefix=prefix)


def test_prefix_cap_counts_only_unfunded_applied_rows_of_that_prefix(tmp_path: Path) -> None:
    # Given: at most 2 unfunded applications per prefix
    s = capped_setup(2)
    a = service(s, tmp_path)
    first = join_from(a, s, 0, 1, "203.0.113.0/24").admission_id
    join_from(a, s, 1, 1, "203.0.113.0/24")
    # When / Then: a third from the same prefix is refused and leaves nothing behind
    with pytest.raises(AdmissionError, match="PREFIX_PENDING_CAPACITY"):
        join_from(a, s, 2, 1, "203.0.113.0/24")
    assert a.store.db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 2
    assert a.store.db.execute("SELECT COUNT(*) FROM admission_origin").fetchone()[0] == 2
    # another prefix is not affected
    assert join_from(a, s, 2, 1, "198.51.100.0/24").state == "APPLIED"
    # a funded application does not count: it paid for its place
    origin, _ = fund(a.escrow)
    op = EscrowLock(
        operation_id="22" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=first,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    a.lock(canonicalize(seal(COLD, "EscrowLock", a.escrow.run_id, op, 500)), now=1)
    assert join_from(a, s, 3, 1, "203.0.113.0/24").state == "APPLIED"
    # and the same joins pass where the policy sets no cap
    free = setup()
    (tmp_path / "free").mkdir()
    b = service(free, tmp_path / "free")
    for index in range(4):
        join_from(b, free, index, 1, "203.0.113.0/24")


def test_prefix_cap_survives_restart_and_epoch_rollover_then_frees_with_the_window(
    tmp_path: Path,
) -> None:
    from hypertrain.protocol.keys import Keypair

    # Given: two unfunded applications at round 999, the last round of quota epoch 0
    # (work_screen_epoch_rounds is 1000 in this setup); their window runs to round 1100.
    s = capped_setup(2)
    a = service(s, tmp_path)
    for index in range(2):
        join_from(a, s, index, 999, "203.0.113.0/24", cold=Keypair(bytes([150 + index]) * 32))
    tags = [r[0] for r in a.store.db.execute("SELECT prefix_tag FROM admission_origin")]
    assert len(set(tags)) == 1 and "203.0.113" not in tags[0]
    # the tag is keyed by the coordinator's secret, so it names the prefix to nobody else
    other = Admission(
        a.escrow,
        s.admission_policy,
        COORD,
        beacon=a.beacon,
        stage_reference=a.stage_reference,
        objects=a.objects,
        ip_secret=b"another-secret",
        qualified=a.qualified,
        conservative_bound=a.conservative_bound,
        backend="cpu",
    )
    assert other._prefix_tag("203.0.113.0/24") != tags[0]
    assert a._prefix_tag("203.0.113.0/24") == tags[0] != a._prefix_tag("198.51.100.0/24")
    # When: the service restarts and a third arrives in epoch 1, rows still alive
    restored = service(s, tmp_path)
    with pytest.raises(AdmissionError, match="PREFIX_PENDING_CAPACITY"):
        join_from(restored, s, 2, 1001, "203.0.113.0/24")
    # Then: once the window is over the rows are retired, the tag goes with them, the cap frees
    joined = join_from(restored, s, 2, 1101, "203.0.113.0/24")
    assert joined.state == "APPLIED"
    assert restored.store.db.execute("SELECT COUNT(*) FROM admission_origin").fetchone()[0] == 1


def test_database_without_origin_table_gets_it_and_counts_nothing(tmp_path: Path) -> None:
    # Given: an application accepted before the table existed (a database of an older version)
    s = capped_setup(2)
    a = service(s, tmp_path)
    join_from(a, s, 0, 1, "203.0.113.0/24")
    a.store.db.execute("DROP TABLE admission_origin")
    # When: the service opens it again
    restored = service(s, tmp_path)
    # Then: the table is back and the legacy row counts against no prefix
    assert restored.store.db.execute("SELECT COUNT(*) FROM admission_origin").fetchone()[0] == 0
    join_from(restored, s, 1, 2, "203.0.113.0/24")
    join_from(restored, s, 2, 2, "203.0.113.0/24")
    with pytest.raises(AdmissionError, match="PREFIX_PENDING_CAPACITY"):
        join_from(restored, s, 3, 2, "203.0.113.0/24")


def test_poll_after_deadline_keeps_miner_whose_proof_is_accepted(tmp_path: Path) -> None:
    # Given: a proof accepted on the last round of the window; its finalization has not run
    # (the proof route commits the proof and the finalization in two transactions).
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    a.challenge(admission, now=2)
    a.prepare_reference(admission, now=2)
    proof, screen = current_proof(a, admission, 2)
    raw = canonicalize(sign_proof(HOT, a.escrow.run_id, proof, 102))
    screen_raw = canonicalize(seal(HOT, "WorkScreenV2", a.escrow.run_id, screen, 102))
    assert a.proof(raw, screen_raw, now=102).state == "PROBATION"
    # When: any caller polls after the deadline
    a.challenge(admission, now=103)
    # Then: the accepted proof is not a missed window
    assert a.store.record(HOT.ss58).state == "PROBATION"
    # The miner retries the very same signed pair: its envelopes expired at the deadline, yet
    # the exact replay is answered, so the finalization that failed can run.
    assert a.reference_due(raw, now=103) is None
    assert a.proof(raw, screen_raw, now=103).state == "PROBATION"
    # The clock does not expire a proven trial either: it waits for its finalization.
    assert a.reap_expired_trials(now=103) == [] and open_trials(a) == 1
    final = a.trial_finality(admission, now=103)
    assert final is not None
    status = a.finalize_trial(admission, final, now=103)
    assert (status.record.state, status.record.clean_count) == ("PROBATION", 1)
    # A different proof after the deadline is still refused.
    other = WorkProof.model_validate({**proof.body(), "delta_hash": H})
    with pytest.raises(ValueError):
        a.proof(canonicalize(sign_proof(HOT, a.escrow.run_id, other, 102)), screen_raw, now=103)


@pytest.mark.parametrize("boundary", ["before_ledger", "after_ledger", "after_receipt"])
def test_outbox_single_lock_when_crash_boundary(tmp_path: Path, boundary: str) -> None:
    # Given: reserve durable operation before any accounting effect.
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    origin, _ = fund(a.escrow)
    op = EscrowLock(
        operation_id="22" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=admission,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    a.store.enqueue_lock(op, COLD.ss58)
    if boundary in ("after_ledger", "after_receipt"):
        a.escrow.lock(op, signer=COLD.ss58)
    if boundary == "after_receipt":
        a.store.reconcile_lock(op.operation_id)
    # When: reconstruct process state and reconcile original operation.
    restored = service(s, tmp_path)
    receipt = restored.store.reconcile_lock(op.operation_id)
    # Then: no second lock, no granted eligibility from an unacknowledged outbox.
    assert restored.escrow.balances().admission_locked == 1000
    assert restored.store.reconcile_lock(op.operation_id) == receipt
    assert not restored.status(HOT.ss58, now=2).eligible
    assert restored.escrow.balances().conserved()


def test_rotation_carries_locks_strikes_dispute_and_history(tmp_path: Path) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    origin, _ = fund(a.escrow)
    op = EscrowLock(
        operation_id="22" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=admission,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    a.store.enqueue_lock(op, COLD.ss58)
    a.store.reconcile_lock(op.operation_id)
    a.suspend(HOT.ss58, reason="pending", now=2, pending_dispute=True)
    a.store.db.execute("UPDATE admissions_v2 SET strikes=2 WHERE admission_id=?", (admission,))
    rotate = RotateRequest(hotkey=HOT.ss58, new_hotkey=OTHER.ss58, operation_id="33" * 32)
    # When
    r = a.rotate(canonicalize(sign_rotation(COLD, a.escrow.run_id, rotate, 200)), now=2)
    # Then
    assert r.admission_id == admission and r.pending_dispute and r.strikes == 2
    assert a.status(OTHER.ss58, now=2).funding.locked_units == 1000
    with pytest.raises(AdmissionError):
        a.status(HOT.ss58, now=2)
    with pytest.raises(AdmissionError):
        a.resume(OTHER.ss58, now=2)


def test_rotation_rejects_hotkey_instead_of_coldkey(tmp_path: Path) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    apply(a, s)
    op = RotateRequest(hotkey=HOT.ss58, new_hotkey=OTHER.ss58, operation_id="33" * 32)
    # When / Then
    with pytest.raises(ValueError):
        a.rotate(canonicalize(sign_rotation(HOT, a.escrow.run_id, op, 200)), now=2)
    assert a.store.record(HOT.ss58).state == "APPLIED"


def accepted_settlement(
    a: Admission, admission_id: str, coldkey: str, reference: str, evidence_hash: str
) -> None:
    """Persist actual service acceptance; callback checks the mutation transaction."""
    a.store.db.execute(
        "CREATE TABLE IF NOT EXISTS accepted_settlements(reference TEXT PRIMARY KEY, body TEXT)"
    )
    status = Settlement(
        finality_hash=H,
        closed_dispute_root=H,
        release_beacon=2,
        unresolved=False,
        outcome="MATCH",
        run_id=a.escrow.run_id,
        admission_id=admission_id,
        coldkey=coldkey,
        dispute_id=reference,
        evidence_hash=evidence_hash,
    )
    a.store.db.execute(
        "INSERT INTO accepted_settlements VALUES(?,?)", (reference, status.model_dump_json())
    )

    def read(reference: str) -> Settlement:
        assert a.store.db.in_transaction
        row = a.store.db.execute(
            "SELECT body FROM accepted_settlements WHERE reference=?", (reference,)
        ).fetchone()
        assert row is not None
        return Settlement.model_validate_json(row[0])

    a.escrow.settlement = read


def recovery_snapshot(a: Admission, hotkey: str) -> tuple:
    return (
        a.store.record(hotkey),
        tuple(
            tuple(row)
            for row in a.store.db.execute("SELECT * FROM admission_pending ORDER BY dispute_id")
        ),
        tuple(
            tuple(row)
            for row in a.store.db.execute("SELECT * FROM admission_transitions ORDER BY seq")
        ),
    )


def test_pending_recovery_rejects_cross_admission_accepted_match(tmp_path: Path) -> None:
    # Given: two actual dual-signed joins, second alone has accepted MATCH.
    s = setup()
    a = service(s, tmp_path)
    first = apply(a, s)
    second = a.join(
        canonicalize(request(s, "22" * 32, OTHER, OTHER).body()), now=1, ip_prefix="loopback"
    ).admission_id
    a.suspend(
        HOT.ss58,
        reason="pending",
        now=2,
        pending_dispute=True,
        dispute_id="33" * 32,
        evidence_hash="44" * 32,
    )
    a.suspend(
        OTHER.ss58,
        reason="pending",
        now=2,
        pending_dispute=True,
        dispute_id="55" * 32,
        evidence_hash="66" * 32,
    )
    accepted_settlement(a, second, OTHER.ss58, "55" * 32, "66" * 32)
    before = recovery_snapshot(a, HOT.ss58)
    # When / Then: trusted accepted record cannot clear first identity.
    with pytest.raises(AdmissionError):
        a.resolve_pending(HOT.ss58, "55" * 32, now=2)
    assert recovery_snapshot(a, HOT.ss58) == before
    assert a.store.record(HOT.ss58).admission_id == first


def test_pending_recovery_rejects_old_dispute_for_current_incident(tmp_path: Path) -> None:
    # Given: one real identity recovered once, then new pending evidence.
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    a.suspend(
        HOT.ss58,
        reason="old",
        now=2,
        pending_dispute=True,
        dispute_id="33" * 32,
        evidence_hash="44" * 32,
    )
    accepted_settlement(a, admission, COLD.ss58, "33" * 32, "44" * 32)
    a.resolve_pending(HOT.ss58, "33" * 32, now=2)
    a.suspend(
        HOT.ss58,
        reason="current",
        now=3,
        pending_dispute=True,
        dispute_id="55" * 32,
        evidence_hash="66" * 32,
    )
    before = recovery_snapshot(a, HOT.ss58)
    # When / Then
    with pytest.raises(AdmissionError):
        a.resolve_pending(HOT.ss58, "33" * 32, now=3)
    assert recovery_snapshot(a, HOT.ss58) == before


@pytest.mark.parametrize(
    "field,value",
    [
        ("admission_id", H),
        ("coldkey", OTHER.ss58),
        ("run_id", H),
        ("evidence_hash", H),
        ("dispute_id", H),
        ("evidence_hash", None),
    ],
)
def test_pending_recovery_rejects_wrong_accepted_linkage(tmp_path: Path, field: str, value) -> None:
    # Given: correct lookup reference, but the accepted record linkage differs.
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    a.suspend(
        HOT.ss58,
        reason="current",
        now=2,
        pending_dispute=True,
        dispute_id="33" * 32,
        evidence_hash="44" * 32,
    )
    accepted_settlement(a, admission, COLD.ss58, "33" * 32, "44" * 32)
    row = a.store.db.execute("SELECT body FROM accepted_settlements").fetchone()
    status = Settlement.model_validate_json(row[0])
    wrong = Settlement.model_validate({**status.body(), field: value})
    a.store.db.execute("UPDATE accepted_settlements SET body=?", (wrong.model_dump_json(),))
    before = recovery_snapshot(a, HOT.ss58)
    # When / Then
    with pytest.raises(AdmissionError):
        a.resolve_pending(HOT.ss58, "33" * 32, now=2)
    assert recovery_snapshot(a, HOT.ss58) == before


def test_rotation_preserves_exact_pending_reference_across_restart(tmp_path: Path) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    a.suspend(
        HOT.ss58,
        reason="pending",
        now=2,
        pending_dispute=True,
        dispute_id="33" * 32,
        evidence_hash="44" * 32,
    )
    rotate = RotateRequest(hotkey=HOT.ss58, new_hotkey=OTHER.ss58, operation_id="77" * 32)
    a.rotate(canonicalize(sign_rotation(COLD, a.escrow.run_id, rotate, 200)), now=2)
    restored = service(s, tmp_path)
    accepted_settlement(restored, admission, COLD.ss58, "33" * 32, "44" * 32)
    before = recovery_snapshot(restored, OTHER.ss58)
    # When / Then: retired key and wrong reference leave the binding intact.
    with pytest.raises(AdmissionError):
        restored.resolve_pending(HOT.ss58, "33" * 32, now=2)
    with pytest.raises(AdmissionError):
        restored.resolve_pending(OTHER.ss58, "88" * 32, now=2)
    assert recovery_snapshot(restored, OTHER.ss58) == before
    restored.resolve_pending(OTHER.ss58, "33" * 32, now=2)
    assert restored.store.record(OTHER.ss58).state == "PROBATION"
    assert not restored.store.record(OTHER.ss58).pending_dispute
    assert restored.store.db.execute("SELECT resolved FROM admission_pending").fetchone()[0] == 1


def test_legacy_unlinked_pending_recovery_fails_closed(tmp_path: Path) -> None:
    # Given: pre-fix pending boolean, no fabricated dispute identity.
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    a.suspend(HOT.ss58, reason="legacy", now=2, pending_dispute=True)
    accepted_settlement(a, admission, COLD.ss58, "33" * 32, "44" * 32)
    before = recovery_snapshot(a, HOT.ss58)
    # When / Then
    with pytest.raises(AdmissionError):
        a.resolve_pending(HOT.ss58, "33" * 32, now=2)
    assert recovery_snapshot(a, HOT.ss58) == before


@pytest.fixture(scope="module")
def trial_launches():
    """Count real reference/miner launches at the existing shared launcher seam."""
    from hypertrain.gpu_ops import work_screen
    from hypertrain.miner import island_launch

    original = island_launch.launch_island
    calls = []

    def counted(job, directory, **kwargs):
        calls.append((job.model_dump(mode="json", exclude={"deadline"}), directory))
        return original(job, directory, **kwargs)

    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(island_launch, "launch_island", counted)
        patch.setattr(work_screen, "launch_island", counted)
        yield calls


@pytest.fixture(scope="module")
def actual_proof(tmp_path_factory: pytest.TempPathFactory, trial_launches):
    # Actual full-round launch and independently committed reference, no worker PASS mock.
    path = tmp_path_factory.mktemp("real-admission")
    s = setup()
    a = service(s, path)
    admission = apply(a, s)
    a.challenge(admission, now=2)
    before = len(trial_launches)
    a.prepare_reference(admission, now=2)
    proof, screen = current_proof(a, admission, 2)
    nonce = screen.nonce
    pair = trial_launches[before:]
    assert len(pair) == 2 and pair[0][0] == pair[1][0]
    assert [p.name for _, p in pair] == ["reference-" + nonce, "miner-" + nonce]
    return s, a, proof, screen


@pytest.mark.parametrize(
    "fault", ["signature", "late", "challenge", "nonce", "layout", "artifact", "unsupported"]
)
def test_proof_rejects_when_not_bound_to_reference(actual_proof, fault: str) -> None:
    # Given
    s, a, p, screen = actual_proof
    p = WorkProof.model_validate(p.body())
    screen = WorkScreenV2.model_validate(screen.body())
    if fault == "challenge":
        p = WorkProof.model_validate({**p.body(), "challenge_hash": H})
    if fault == "nonce":
        screen = WorkScreenV2.model_validate({**screen.body(), "nonce": H})
    if fault == "artifact":
        screen = WorkScreenV2.model_validate({**screen.body(), "artifact_hashes": [H]})
    if fault == "unsupported":
        from pydantic import ValidationError

        with pytest.raises(ValidationError):
            WorkScreenV2.model_validate({**screen.body(), "evidence_kind": "nvidia-cc"})
        return
    if fault == "layout":
        screen = WorkScreenV2.model_validate(
            {**screen.body(), "layout": {**screen.layout.model_dump(mode="json"), "zero1": True}}
        )
    signer = OTHER if fault == "signature" else HOT
    # When / Then
    with pytest.raises(ValueError):
        a.proof(
            canonicalize(sign_proof(signer, a.escrow.run_id, p, 500)),
            canonicalize(seal(HOT, "WorkScreenV2", a.escrow.run_id, screen, 500)),
            now=103 if fault == "late" else 2,
        )
    assert a.escrow.balances().issued == 0


def test_full_shadow_probation_and_graduation_when_funded(tmp_path: Path, trial_launches) -> None:
    # Given: each finalized participation executes its own nonce-bound full reference.
    s = setup()
    a = service(s, tmp_path)
    admission = apply(a, s)
    origin, _ = fund(a.escrow)
    op = EscrowLock(
        operation_id="22" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=admission,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    a.lock(canonicalize(seal(COLD, "EscrowLock", a.escrow.run_id, op, 500)), now=1)
    before = len(trial_launches)
    for i in range(12):
        now = i + 2
        a.challenge(admission, now=now)
        a.prepare_reference(admission, now=now)
        proof, screen = current_proof(a, admission, now)
        assert len(trial_launches) - before == 2 * (i + 1)
        reference, miner = trial_launches[-2:]
        assert reference[0] == miner[0]
        assert [reference[1].name, miner[1].name] == [
            "reference-" + screen.nonce,
            "miner-" + screen.nonce,
        ]
        submit(a, proof, screen, now)
        row = a.store.db.execute(
            "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission,)
        ).fetchone()
        from hypertrain.auditor.replay import unpack_state
        from hypertrain.trainer.compress import state_hash

        theta, _ = unpack_state(get_large(proof.artifact_refs[0].sha256, a.objects.get))
        final = Finalize(
            w=row["trial_epoch"],
            final_theta_hash_w1=state_hash(theta),
            included=[HOT.ss58],
            entitlements_root=digest(proof),
        )
        # When: authenticated finalize consumes the real committed reference once.
        status = a.finalize_trial(
            admission, canonicalize(seal(COORD, "Finalize", a.escrow.run_id, final, 500)), now=now
        )
        # Then: exact schedule; calendar time never adds clean participation.
        assert status.record.clean_count == i + 1 and status.actual_q_ppm == 1_000_000
        assert status.nominal_q_ppm == (
            1_000_000 if i + 1 < 4 else 500_000 if i + 1 < 8 else 250_000
        )
        if i < 3:
            assert status.shadow_only
        if i < 11:
            assert status.record.state == "PROBATION"
    assert status.record.state == "ACTIVE" and status.eligible
    assert status.record.canary_blocks == (1, 2, 3)
    assert a.escrow.balances().issued == 30000  # no reward minted by trial PASS
    assert a.escrow.balances().conserved()
    # Honest graduation does not waive ongoing full replay: a new defective assignment suspends.
    a.challenge(admission, now=14)
    a.prepare_reference(admission, now=14)
    proof, screen = current_proof(a, admission, 14)
    assert len(trial_launches) - before == 26
    bad = WorkProof.model_validate({**proof.body(), "delta_hash": H})
    r = submit(a, bad, screen, 14)
    assert r.state == "SUSPENDED" and r.pending_dispute
    assert not a.status(HOT.ss58, now=14).eligible


def test_production_screen_holds_over_4_real_and_36_modelled_trials_then_expires(
    tmp_path: Path, trial_launches
) -> None:
    # Given: the admission policy `manifest_v2 example --ledger-mode production` emits, and the
    # oldest screen at round 0's d_final after a 40-trial campaign (3 ACTIVE x 12 + 1 PROBATION
    # x 4): the PROBATION miner, screened first. Only its 4 trials run for real. Each spends its
    # whole window: proof and finalize at seed + challenge_rounds, so the next seed is one round
    # later.
    from hypertrain.ops.manifest_v2 import main as manifest_cli

    example = tmp_path / "example"
    assert manifest_cli(["example", "--ledger-mode", "production", "--out", str(example)]) == 0
    policy = AdmissionPolicyV2.model_validate(
        json.loads((example / "policies.json").read_bytes())["admission"]
    )
    base = setup()
    body = base.manifest.body()
    body["network"]["admission_policy_hash"] = digest(policy)
    s = replace(base, manifest=RunManifestV2.model_validate(body), admission_policy=policy)
    a = service(s, tmp_path)
    admission = apply(a, s)
    origin, _ = fund(a.escrow)
    op = EscrowLock(
        operation_id="22" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=admission,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    a.lock(canonicalize(seal(COLD, "EscrowLock", a.escrow.run_id, op, 500)), now=1)
    for _ in range(4):
        seed = a.store.db.execute(
            "SELECT seed_beacon FROM admissions_v2 WHERE admission_id=?", (admission,)
        ).fetchone()[0]
        a.challenge(admission, now=seed)
        deadline = seed + policy.challenge_rounds
        a.prepare_reference(admission, now=deadline)
        proof, screen = current_proof(a, admission, deadline)
        a.proof(
            canonicalize(sign_proof(HOT, a.escrow.run_id, proof, 100000)),
            canonicalize(seal(HOT, "WorkScreenV2", a.escrow.run_id, screen, 100000)),
            now=deadline,
        )
        final = a.trial_finality(admission, now=deadline)
        assert final is not None
        status = a.finalize_trial(admission, final, now=deadline)
    assert status.record.state == "PROBATION" and status.record.clean_count == 4
    # When: the other 36 trials and round 0 are arithmetic, not run: challenge_rounds + 1 rounds
    # per back-to-back worst-case trial, then 470 rounds from round 0's open request to its
    # d_final, as reported for TEST run 2 (b14a975e) and not replayed here.
    d_final = deadline + 36 * (policy.challenge_rounds + 1) + 470
    # Then: the first screen still holds at round 0's d_final, and still lapses after
    # work_screen_epoch_rounds without a new proof. Aggregation and finalization re-check
    # eligibility at or after d_final; the rest of the window is their budget.
    assert a.status(HOT.ss58, now=d_final).eligible
    assert not a.status(HOT.ss58, now=deadline + policy.work_screen_epoch_rounds + 1).eligible


def test_zero_origins_cannot_graduate_even_after_calendar_age(tmp_path: Path) -> None:
    # Given
    s = setup()
    a = service(s, tmp_path)
    apply(a, s)
    # When
    status = a.graduate(HOT.ss58, now=1000)
    # Then
    assert (
        not status.eligible and status.record.clean_count == 0 and status.record.state == "APPLIED"
    )
