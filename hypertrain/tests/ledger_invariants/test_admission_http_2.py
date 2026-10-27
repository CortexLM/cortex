"""In-process admission rules: the credit chain of a trial (AD-02, AD-03) and the identity rules
of the application queue under any sequence of joins (AD-05, SG-06).

One genuine shadow trial is computed per module (`TrialRig`) and cloned for each test, so every
test starts from a real challenge, a real committed reference and the miner's real work, and
changes only the message it is about.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from hypertrain.beacon.core import FixtureBeacon
from hypertrain.challenge.admission import Admission
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.data.store import LocalFSStore
from hypertrain.miner.admission import sign_join
from hypertrain.protocol.envelope_v2 import UnauthorizedSigner, parse_envelope, seal
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import HardwareHint, WorkProof

from .support import load_fixture_module, memory_db
from .support_admission_http import ADMISSION_FIXTURE, COLDS, HOTS, TrialRig, build_rig, table_rows

H = "ab" * 32
MINER = load_fixture_module(*ADMISSION_FIXTURE).HOT


@pytest.fixture(scope="module")
def rig(tmp_path_factory: pytest.TempPathFactory) -> TrialRig:
    return build_rig(tmp_path_factory.mktemp("trial-rig"))


@pytest.fixture(scope="module")
def genuine_final(rig: TrialRig, tmp_path_factory: pytest.TempPathFactory) -> bytes:
    """The coordinator-signed Finalize of the genuine trial, as the service itself derives it."""
    admission = rig.open("referenced", tmp_path_factory.mktemp("final") / "a")
    rig.submit(admission)
    raw = admission.trial_finality(rig.admission_id, now=2)
    assert raw is not None
    return raw


def resigned(rig: TrialRig, raw: bytes, signer: Any = None, **changes: Any) -> bytes:
    """`raw` with fields changed or only signed again: a different signature, the same content."""
    envelope = parse_envelope(raw)
    final = Finalize.model_validate({**envelope.body, **changes})
    key = signer or rig.module.COORD
    return canonicalize(seal(key, "Finalize", envelope.run_id, final, envelope.exp_drand))


def outcome_of(admission: Admission) -> str:
    return str(
        admission.store.db.execute("SELECT outcome FROM admission_trial_results").fetchone()[0]
    )


# AD-03: credit only when the signed proof equals the reference committed before the comparison.


def test_ad03_a_proof_arriving_before_the_service_reference_is_committed_earns_nothing(
    rig: TrialRig, tmp_path: Path
) -> None:
    # Given: a challenge was issued and the service has not computed its reference yet
    admission = rig.open("challenged", tmp_path / "a")
    before = table_rows(admission.store.db)
    # When / Then: the miner's genuine proof is refused and nothing is written
    with pytest.raises(AdmissionError, match="REFERENCE_NOT_COMMITTED"):
        rig.submit(admission)
    assert table_rows(admission.store.db) == before
    record = admission.store.by_id(rig.admission_id)
    assert (record.state, record.clean_count) == ("APPLIED", 0)


def test_ad03_a_proof_dated_before_the_reference_commitment_is_refused(
    rig: TrialRig, tmp_path: Path
) -> None:
    # Given: the reference was committed at round 2
    admission = rig.open("referenced", tmp_path / "a")
    before = table_rows(admission.store.db)
    # When / Then: a proof stamped round 1 cannot have been compared to it
    with pytest.raises(AdmissionError, match="PROOF_BEFORE_REFERENCE"):
        rig.submit(admission, now=1)
    assert table_rows(admission.store.db) == before


def test_ad03_an_equal_proof_is_accepted_and_credited_only_by_the_finalization(
    rig: TrialRig, tmp_path: Path, genuine_final: bytes
) -> None:
    # Given
    admission = rig.open("referenced", tmp_path / "a")
    # When: the genuine proof is accepted
    record = rig.submit(admission)
    # Then: the admission leaves APPLIED but earns no clean participation yet
    assert (record.state, record.reason, record.clean_count) == (
        "PROBATION",
        "FULL_REFERENCE_MATCH",
        0,
    )
    assert outcome_of(admission) == "OPEN"
    # And: the coordinator's finalization is what credits it
    status = admission.finalize_trial(rig.admission_id, genuine_final, now=2)
    assert status.record.clean_count == 1
    assert outcome_of(admission) == "MATCH"


def test_ad03_a_proof_that_differs_from_the_committed_reference_earns_nothing(
    rig: TrialRig, tmp_path: Path, genuine_final: bytes
) -> None:
    # Given: a proof whose delta hash is not the one the service computed
    admission = rig.open("referenced", tmp_path / "a")
    different = WorkProof.model_validate({**rig.proof.body(), "delta_hash": H})
    # When
    record = rig.submit(admission, different)
    # Then: suspended pending a dispute with a strike, no credit, and nothing to finalize
    assert (record.state, record.reason, record.strikes, record.pending_dispute) == (
        "SUSPENDED",
        "PENDING_REPLAY_MISMATCH",
        1,
        True,
    )
    assert record.clean_count == 0
    assert outcome_of(admission) == "MISMATCH"
    assert admission.trial_finality(rig.admission_id, now=2) is None
    before = table_rows(admission.store.db)
    with pytest.raises(AdmissionError, match="UNVERIFIED_TRIAL"):
        admission.finalize_trial(rig.admission_id, genuine_final, now=2)
    assert table_rows(admission.store.db) == before


# AD-02: a trial is finalized at most once, only with a Finalize bound to this participation.


@pytest.fixture
def proved(rig: TrialRig, tmp_path: Path) -> Admission:
    admission = rig.open("referenced", tmp_path / "a")
    rig.submit(admission)
    return admission


@pytest.mark.parametrize(
    ("fault", "changes"),
    [
        ("another_epoch", {"w": 7}),
        ("another_participant", {"included": [HOTS[0].ss58]}),
        ("another_reference", {"entitlements_root": H}),
        ("another_final_state", {"final_theta_hash_w1": H}),
        ("an_additional_participant", {"included": [MINER.ss58, HOTS[0].ss58]}),
    ],
)
def test_ad02_a_finalize_not_bound_to_this_participation_credits_nothing(
    rig: TrialRig, proved: Admission, genuine_final: bytes, fault: str, changes: dict[str, Any]
) -> None:
    # Given: a proof accepted, and a Finalize signed by the coordinator but for another subject
    before = table_rows(proved.store.db)
    other = resigned(rig, genuine_final, **changes)
    # When / Then
    with pytest.raises(AdmissionError, match="TRIAL_FINALITY_BINDING"):
        proved.finalize_trial(rig.admission_id, other, now=2)
    assert table_rows(proved.store.db) == before
    # And: the genuine Finalize still credits, so the refusal was about the subject only
    assert proved.finalize_trial(rig.admission_id, genuine_final, now=2).record.clean_count == 1


def test_ad02_a_finalize_not_signed_by_the_coordinator_credits_nothing(
    rig: TrialRig, proved: Admission, genuine_final: bytes
) -> None:
    # Given: the genuine content signed by the miner itself
    before = table_rows(proved.store.db)
    forged = resigned(rig, genuine_final, signer=rig.module.HOT)
    # When / Then
    with pytest.raises(UnauthorizedSigner):
        proved.finalize_trial(rig.admission_id, forged, now=2)
    assert table_rows(proved.store.db) == before


def test_ad02_a_finalized_trial_is_credited_once_at_any_later_beacon(
    rig: TrialRig, proved: Admission, genuine_final: bytes
) -> None:
    # Given: the participation was credited once
    first = proved.finalize_trial(rig.admission_id, genuine_final, now=2)
    assert first.record.clean_count == 1
    credited = table_rows(proved.store.db)
    # When: the same content arrives again later, each time with a new signature
    for now in (3, 5):
        again = proved.finalize_trial(rig.admission_id, resigned(rig, genuine_final), now=now)
        # Then: it is answered with the standing and writes nothing
        assert again.record.clean_count == 1
        assert table_rows(proved.store.db) == credited


# AD-05 and SG-06 for any sequence of joins.


def no_trial(*_: Any) -> Any:
    raise AssertionError("a join never runs a trial")


@pytest.fixture(scope="module")
def objects(tmp_path_factory: pytest.TempPathFactory) -> LocalFSStore:
    return LocalFSStore(tmp_path_factory.mktemp("objects"))


JOIN_OPERATIONS = st.lists(
    st.tuples(
        st.integers(0, 4),  # hotkey
        st.integers(0, 2),  # coldkey
        st.integers(0, 1),  # request id
        st.booleans(),  # the admission is ACTIVE afterwards
    ),
    min_size=1,
    max_size=12,
)


@given(operations=JOIN_OPERATIONS)
def test_ad05_sg06_identity_rules_hold_for_any_sequence_of_joins(
    objects: LocalFSStore, operations: list[tuple[int, int, int, bool]]
) -> None:
    module = load_fixture_module(*ADMISSION_FIXTURE)
    setup = module.setup()
    db = memory_db()
    admission = Admission(
        module.ledger(setup, db),
        setup.admission_policy,
        module.COORD,
        beacon=FixtureBeacon(current=100000).get,
        stage_reference=no_trial,
        objects=objects,
        ip_secret=b"private-prefix-key",
        qualified=lambda _: True,
        conservative_bound=lambda: True,
        backend="cpu",
    )
    run_id = setup.manifest.run_id()
    policy_hash = setup.manifest.network.admission_policy_hash
    spent_hotkeys: set[str] = set()
    state_of: dict[str, tuple[str, str]] = {}  # admission id -> (coldkey, state)
    answered: dict[tuple[str, str], tuple[str, str]] = {}  # (coldkey, request id) -> (hotkey, id)
    for step, (h, c, r, active) in enumerate(operations):
        now = 1 + 2 * step  # two rounds apart: the per-identity quota is never the reason
        hot, cold = HOTS[h], COLDS[c]
        request = sign_join(
            hot,
            cold,
            run_id=run_id,
            request_id=f"{r + 1:02x}" * 32,
            expires_beacon=10000,
            policy_hash=policy_hash,
            hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
        )
        raw = canonicalize(request.body())
        before = table_rows(admission.store.db)
        known = answered.get((cold.ss58, request.request_id))
        code: str | None = None
        record = None
        try:
            record = admission.join(raw, now=now, ip_prefix="p")
        except AdmissionError as error:
            code = error.code
        if known is not None:
            # A repeat is answered from the reservation; another body under it is a conflict.
            if known[0] == hot.ss58:
                assert record is not None and record.admission_id == known[1]
            else:
                assert code == "REPLAY_CONFLICT"
            assert table_rows(admission.store.db) == before
            continue
        busy = any(owner == cold.ss58 and state != "ACTIVE" for owner, state in state_of.values())
        if hot.ss58 in spent_hotkeys:
            assert code == "HOTKEY_HISTORY_EXISTS"
        elif busy:
            assert code == "COLDKEY_PROBATION_EXISTS"
        else:
            assert code is None and record is not None
            assert (record.hotkey, record.coldkey, record.state) == (hot.ss58, cold.ss58, "APPLIED")
            spent_hotkeys.add(hot.ss58)
            answered[(cold.ss58, request.request_id)] = (hot.ss58, record.admission_id)
            state_of[record.admission_id] = (cold.ss58, "APPLIED")
            if active:
                admission.store.transition(record.admission_id, "ACTIVE", "TEST", now)
                state_of[record.admission_id] = (cold.ss58, "ACTIVE")
            continue
        assert table_rows(admission.store.db) == before
    stored = db.execute("SELECT admission_id, hotkey, coldkey, state FROM admissions_v2").fetchall()
    assert len({row["hotkey"] for row in stored}) == len(stored) == len(state_of)
    assert {row["admission_id"]: (row["coldkey"], row["state"]) for row in stored} == state_of
    history = {row[0] for row in db.execute("SELECT hotkey FROM admission_history")}
    assert history == spent_hotkeys
