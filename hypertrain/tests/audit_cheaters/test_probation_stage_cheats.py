"""Cheats at the probation stage (join-v2 -> trial -> proof), the one place where v2 sanctions.

The master replays every trial itself and compares the miner's whole WorkProof with its own
(admission.py proof()). A differing proof suspends the identity (PENDING_REPLAY_MISMATCH), adds a
strike and closes the trial as MISMATCH; a proof that is not bound to the open trial is refused
with WORK_SCREEN_BINDING and changes nothing. test_admission_defences_http.py proves both
reactions for proofs edited field by field and pins, once, what follows a mismatch (the trial
route closes, the coldkey stays bound); here the work comes from the real miner path:
join -> NetworkMiner.probe_trial (the production screen_work builds the proof from the artifacts
of the launch) -> POST /join/proof, with a cheating-miner double of ac_cheats.py in the launch.
The double is installed where screen_work looks up launch_island, so the artifacts reach the
master unvalidated, as from a miner that does not run the honest client; only the master's
comparison is under test. Each scenario joins a fresh identity of an unjoined shape (one
graduated identity, nine unjoined) in its own clone of the shared snapshot.

The honest-hardware false positive (a last-bit CPU difference) is indistinguishable from the
one-ulp double by outcome; PRB-01 reproduces that its consequence has no way back.

Every launch runs against a wall-clock budget: the job deadline is (deadline_beacon - 1) * 3 s
after the genesis, the fixture clock starts at genesis + 1 when the World is installed and
pushing beacons does not move it. Measured on the shared snapshot, a trial opened right after
install has about 640 s of real time; a launch that outlives it fails with an IslandFailure
("job deadline ...", or "rank failure 1: ..." when a rank hits the deadline first).
"""

from __future__ import annotations

import json
import shutil
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest
from ac_cheats import BY_NAME, Cheat, artifacts_of, lie
from ac_precondition import precondition
from ac_world import Shape, record

from hypertrain.gpu_ops import work_screen
from hypertrain.gpu_ops.work_screen import WorkScreenError
from hypertrain.miner.core import OWNER_REQUEST_ROUNDS, ChallengeHTTPError, NetworkMiner
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import IslandJobV1

ESTABLISHED = 0
NEWCOMER = 1
SECOND_NEWCOMER = 2
OUTSIDER = 9
COLD_SEED = 90
PROOF_ROUTE = "POST /join/proof"

# Doubles of ac_cheats.py that change the work the master replays (cheats 1, 3, 5 and 6).
MISMATCHING = [
    "altered-weights-one-ulp",
    "altered-weights-state-file-only",
    "forged-delta-valid-hash",
    "skipped-optimizer-step",
    "truncated-step-count",
    "wrong-data-shard",
    "wrong-seed-forged-rng-ctr",
    "wrong-initial-theta",
]


@pytest.fixture(scope="module")
def ac_shape() -> Shape:
    return Shape(identities=10, graduated=1)


def cold_keyfile(world: Any, index: int) -> Path:
    keys = world.scratch / "keys"
    keys.mkdir(parents=True, exist_ok=True)
    path = keys / f"cold-{index}.key"
    path.write_bytes(bytes([COLD_SEED + index]) * 32)
    path.chmod(0o600)
    return path


def join(world: Any, miner: NetworkMiner, index: int) -> dict[str, Any]:
    request_id = sha256_hex(f"probation-join|{miner.kp.ss58}|{index}".encode())
    return miner.join(cold_keyfile(world, index), request_id, 10000)


def apply(world: Any, index: int) -> NetworkMiner:
    """The real join-v2 of identity `index`, then one beacon so its trial seed is ready."""
    miner = world.miner(index)
    join(world, miner, index)
    world.network.push(world.network.now + 1)
    return miner


def refusal(call: Callable[..., Any], *args: Any) -> ChallengeHTTPError:
    with pytest.raises(ChallengeHTTPError) as caught:
        call(*args)
    return caught.value


def attempt(call: Callable[..., Any], *args: Any) -> tuple[Any, ChallengeHTTPError | None]:
    try:
        return call(*args), None
    except ChallengeHTTPError as error:
        return None, error


def submit(miner: NetworkMiner, result: dict[str, Any], folder: Path) -> dict[str, Any]:
    """proof-v2: the signed proof and screen of probe-v2, posted as the miner CLI posts them."""
    folder.mkdir(parents=True, exist_ok=True)
    document = folder / f"proof-{len(list(folder.iterdir()))}.json"
    document.write_text(json.dumps(result))
    return miner.proof(document)


@contextmanager
def launching(make: Callable[[Any], Any]) -> Iterator[None]:
    """`make(real)` replaces launch_island where screen_work looks it up, for one probe."""
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(work_screen, "launch_island", make(work_screen.launch_island))
        yield


def probe_with_double(
    world: Any, miner: NetworkMiner, cheat: Cheat, peer: Any = None
) -> dict[str, Any]:
    context = SimpleNamespace(
        w=0,
        hotkey=miner.kp.ss58,
        rows=world.network.setup.rows,
        tree=world.network.setup.tree,
        peer_artifacts=lambda: peer,
        own_previous_artifacts=lambda: None,
    )
    with launching(lambda real: cheat.launch(real, context)):
        return miner.probe_trial()


def keeping_probe(miner: NetworkMiner, kept: Path) -> dict[str, Any]:
    """An honest probe that also keeps a copy of the published artifacts of its launch."""

    def keeping(real: Callable[..., Any]) -> Callable[..., Any]:
        def launch(*args: Any, **kwargs: Any) -> Any:
            artifacts = real(*args, **kwargs)
            shutil.copytree(artifacts.directory, kept)
            return artifacts

        return launch

    with launching(keeping):
        return miner.probe_trial()


def trial_outcome(world: Any, miner: NetworkMiner) -> str | None:
    row = world.network.store._db.execute(
        "SELECT r.outcome FROM admissions_v2 a JOIN admission_trial_results r "
        "ON r.epoch=a.trial_epoch WHERE a.hotkey=?",
        (miner.kp.ss58,),
    ).fetchone()
    return None if row is None else str(row[0])


def stored_state(world: Any, miner: NetworkMiner) -> dict[str, Any]:
    """What accepting or sanctioning a proof writes. The reference columns are left out: the
    master replays the trial on the first signed proof of its owner, accepted or not."""
    db, hotkey = world.network.store._db, miner.kp.ss58
    return {
        "admission": list(
            db.execute(
                "SELECT state,reason,strikes,pending_dispute,clean_count,proof_digest,screen_json "
                "FROM admissions_v2 WHERE hotkey=?",
                (hotkey,),
            )
        ),
        "trial": list(
            db.execute(
                "SELECT r.outcome,r.proof,r.screen FROM admissions_v2 a JOIN "
                "admission_trial_results r ON r.epoch=a.trial_epoch WHERE a.hotkey=?",
                (hotkey,),
            )
        ),
        "transitions": list(db.execute("SELECT * FROM admission_transitions")),
        "pending": list(db.execute("SELECT * FROM admission_pending")),
    }


def standing(miner: NetworkMiner) -> dict[str, Any]:
    keys = ("state", "reason", "strikes", "pending_dispute", "clean_count")
    return {key: miner.status()["record"][key] for key in keys}


def next_trial(miner: NetworkMiner) -> dict[str, Any]:
    admission_id = miner.status()["record"]["admission_id"]
    asked = envelope_v2.seal(
        miner.kp,
        "ChallengeRequest",
        miner.run_id,
        {"admission_id": admission_id},
        miner.api.call("GET", miner.url)["now_round"] + OWNER_REQUEST_ROUNDS,
    )
    return miner.api.call("POST", miner.url + f"/join/{admission_id}/challenge", json=asked)


def pending_reference(world: Any, miner: NetworkMiner) -> str:
    row = world.network.store._db.execute(
        "SELECT p.dispute_id FROM admission_pending p JOIN admissions_v2 a "
        "ON a.admission_id=p.admission_id WHERE a.hotkey=? AND p.resolved=0",
        (miner.kp.ss58,),
    ).fetchone()
    return str(row[0])


def recover(world: Any, miner: NetworkMiner, reference: str) -> dict[str, Any]:
    receipt = envelope_v2.seal(
        miner.kp,
        "Receipt",
        miner.run_id,
        {"w": 0, "commit_hash": reference, "received_round": world.network.now},
        10000,
    )
    return miner.api.call("POST", miner.url + "/admission/recover", json=receipt)


def established_record(world: Any) -> dict[str, Any]:
    response = world.network.client.get(
        world.network.url + "/admission/" + world.hotkey(ESTABLISHED)
    )
    assert response.status_code == 200, response.text
    return dict(response.json()["record"])


def evidence(cheat: tuple[int, str, str], intake: tuple[int, str | None], **fields: Any) -> None:
    """One table row: (cheat id, double, seam) and what the master answered at intake."""
    record(
        {
            "stage": "probation",
            "cheat": cheat[0],
            "name": cheat[1],
            "seam": cheat[2],
            "intake": {"status": intake[0], "endpoint": PROOF_ROUTE, "detail": intake[1]},
            "audit_verdict": None,
            "round_settled": None,
            **fields,
        }
    )


def row_of(cheat: Cheat) -> tuple[int, str, str]:
    return cheat.cheat, cheat.name, cheat.seam


def test_an_honest_miner_proves_two_trials_and_is_never_struck(ac_clone, tmp_path: Path) -> None:
    """Control: the master's replay of an honest proof matches, trial after trial."""
    # Given
    world, miner = ac_clone, apply(ac_clone, NEWCOMER)
    # When
    first = submit(miner, miner.probe_trial(), tmp_path / "first")
    # Then
    assert (first["state"], first["reason"]) == ("PROBATION", "FULL_REFERENCE_MATCH")
    assert (first["strikes"], first["pending_dispute"], first["clean_count"]) == (0, False, 1)
    assert trial_outcome(world, miner) == "MATCH"
    # When: the next trial opens one beacon after the finalization of the first
    world.network.push(world.network.now + 1)
    second = submit(miner, miner.probe_trial(), tmp_path / "second")
    # Then
    assert (second["state"], second["strikes"], second["clean_count"]) == ("PROBATION", 0, 2)
    assert trial_outcome(world, miner) == "MATCH"
    evidence(
        (0, "honest", "none"),
        (200, None),
        admission=standing(miner),
        trial_outcome=trial_outcome(world, miner),
        honest_effect="not applicable",
        hole=None,
    )


@pytest.mark.parametrize("name", MISMATCHING)
def test_work_that_differs_from_the_master_replay_is_suspended_and_struck(
    ac_clone, tmp_path: Path, name: str
) -> None:
    """The master's own replay is the reference: any difference in the WorkProof built from the
    miner's artifacts suspends the identity with a strike and a MISMATCH trial, and leaves an
    established miner untouched. (The closed trial route and the bound coldkey that follow are
    pinned once, in test_admission_defences_http.py.)"""
    # Given
    world, cheat = ac_clone, BY_NAME[name]
    before = established_record(world)
    miner = apply(world, NEWCOMER)
    result = probe_with_double(world, miner, cheat)
    # When
    answer = submit(miner, result, tmp_path)
    # Then: suspended pending a dispute, one strike, no clean participation
    assert (answer["state"], answer["reason"]) == ("SUSPENDED", "PENDING_REPLAY_MISMATCH")
    assert (answer["strikes"], answer["pending_dispute"], answer["clean_count"]) == (1, True, 0)
    assert trial_outcome(world, miner) == "MISMATCH"
    # And: the established miner is unaffected
    assert established_record(world) == before
    evidence(
        row_of(cheat),
        (200, None),
        admission=standing(miner),
        trial_outcome=trial_outcome(world, miner),
        honest_effect="established identity record unchanged",
        hole=None,
    )


def test_work_copied_from_a_peer_is_suspended_while_the_peer_keeps_its_standing(
    ac_clone, tmp_path: Path
) -> None:
    """Trial samples are drawn per admission and nonce, so a peer's artifacts presented as one's
    own are another job's output: the replay of the copier's job differs and it is struck."""
    # Given: an honest peer, accepted, whose published trial output was kept
    world, cheat = ac_clone, BY_NAME["copied-peer-upload-resigned"]
    peer, copier = apply(world, NEWCOMER), apply(world, SECOND_NEWCOMER)
    kept = tmp_path / "peer-published"
    peer_result = keeping_probe(peer, kept)
    peer_answer = submit(peer, peer_result, tmp_path / "peer")
    assert peer_answer["state"] == "PROBATION"
    # When: the copier presents the peer's artifacts as its own
    result = probe_with_double(world, copier, cheat, peer=artifacts_of(kept))
    answer = submit(copier, result, tmp_path / "copier")
    # Then: what it signed is the peer's work, and the master suspends and strikes it
    assert result["proof"]["body"]["artifact_refs"] == peer_result["proof"]["body"]["artifact_refs"]
    assert (answer["state"], answer["reason"]) == ("SUSPENDED", "PENDING_REPLAY_MISMATCH")
    assert (answer["strikes"], answer["pending_dispute"]) == (1, True)
    assert trial_outcome(world, copier) == "MISMATCH"
    # And: the peer is unaffected
    assert standing(peer) == {
        "state": "PROBATION",
        "reason": "FULL_REFERENCE_MATCH",
        "strikes": 0,
        "pending_dispute": False,
        "clean_count": 1,
    }
    evidence(
        row_of(cheat),
        (200, None),
        admission=standing(copier),
        trial_outcome=trial_outcome(world, copier),
        honest_effect="peer stays PROBATION, clean_count 1, no strike",
        hole=None,
    )


def test_the_honest_client_refuses_a_job_whose_samples_the_challenge_does_not_commit_to(
    ac_clone, tmp_path: Path
) -> None:
    """The signed challenge commits to the trial's sample ids: a job edited to train on other
    samples is refused by screen_work before any launch."""
    # Given: a fetched trial, still open, whose job file now names the next sample ids
    world, miner = ac_clone, apply(ac_clone, NEWCOMER)
    job_path, challenge_path = miner._fetch_trial(tmp_path)
    assert trial_outcome(world, miner) == "OPEN"
    job = IslandJobV1.model_validate_json(job_path.read_bytes())
    size = job.manifest.training.dataset.n_samples
    edited = job.model_copy(update={"sample_ids": [(i + 1) % size for i in job.sample_ids]})
    job_path.write_bytes(edited.model_dump_json().encode())

    def no_launch(real: Callable[..., Any]) -> Callable[..., Any]:
        def launch(*args: Any, **kwargs: Any) -> Any:
            pytest.fail("screen_work launched a job the challenge does not commit to")

        return launch

    # When / Then
    with launching(no_launch), pytest.raises(WorkScreenError) as caught:
        miner.probe(job_path, challenge_path)
    assert caught.value.code == "CHALLENGE_BINDING"


def sealed(miner: NetworkMiner, kind: str, body: dict[str, Any], like: dict[str, Any]) -> Any:
    return envelope_v2.seal(miner.kp, kind, miner.run_id, body, like["exp_drand"])


def test_a_proof_of_an_earlier_trial_is_refused_and_the_open_trial_stays_provable(
    ac_clone, tmp_path: Path
) -> None:
    """A proof is accepted only for the exact challenge of the open trial. The signed proof of
    the previous trial, replayed while the next one is open, is refused with WORK_SCREEN_BINDING
    and changes no stored state; the honest proof of the open trial is then still accepted.
    (Edited challenge hashes, nonces, layouts and artifact lists are pinned field by field in
    test_admission_defences_http.py.)"""
    # Given: trial 1 proved and finalized, trial 2 open
    world, miner = ac_clone, apply(ac_clone, NEWCOMER)
    earlier = miner.probe_trial()
    assert submit(miner, earlier, tmp_path / "first")["clean_count"] == 1
    world.network.push(world.network.now + 1)
    next_trial(miner)
    genuine = miner.probe_trial()
    before = stored_state(world, miner)
    # When
    error = refusal(submit, miner, earlier, tmp_path / "replayed")
    # Then: 409 WORK_SCREEN_BINDING and no state change
    assert error.status == 409 and "WORK_SCREEN_BINDING" in error.detail
    assert stored_state(world, miner) == before
    after_refusal = standing(miner)
    assert trial_outcome(world, miner) == "OPEN"
    # And: the genuine proof of the open trial is still accepted
    accepted = submit(miner, genuine, tmp_path / "genuine")
    assert (accepted["state"], accepted["strikes"], accepted["clean_count"]) == ("PROBATION", 0, 2)
    evidence(
        (2, "replayed-earlier-trial-proof", "wire"),
        (error.status, error.detail),
        admission=after_refusal,
        trial_outcome="OPEN",
        honest_effect="not applicable",
        hole=None,
    )


def test_a_proof_claiming_a_leaves_root_the_artifacts_do_not_have_is_suspended_and_struck(
    ac_clone, tmp_path: Path
) -> None:
    """The master compares the whole WorkProof with its replay, not only the artifact refs: a
    miner that signs a leaves root other than its leaves object's is struck like any mismatch."""
    # Given: an honest result whose proof claims another leaves root, re-signed by the miner
    world, miner = ac_clone, apply(ac_clone, NEWCOMER)
    honest = miner.probe_trial()
    body = honest["proof"]["body"]
    forged = {
        **honest,
        "proof": sealed(
            miner, "WorkProof", {**body, "leaves_root": lie(body["leaves_root"])}, honest["proof"]
        ),
    }
    # When
    answer = submit(miner, forged, tmp_path)
    # Then
    assert (answer["state"], answer["reason"]) == ("SUSPENDED", "PENDING_REPLAY_MISMATCH")
    assert (answer["strikes"], answer["pending_dispute"]) == (1, True)
    assert trial_outcome(world, miner) == "MISMATCH"
    evidence(
        (7, "proof-lie-leaves-root", "proof"),
        (200, None),
        admission=standing(miner),
        trial_outcome=trial_outcome(world, miner),
        honest_effect="not applicable",
        hole=None,
    )


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="PRB-01: PENDING_REPLAY_MISMATCH binds a synthetic dispute id that no settlement can "
    "exist for (Store._settlement_v2 finds neither a dispute nor a settlement record, "
    "store.py:2318-2332), so /admission/recover stays UNACCEPTED_SETTLEMENT; resume() is only "
    "reachable through it, strikes are written (admission.py:733) and never read, and the "
    "coldkey stays blocked (COLDKEY_PROBATION_EXISTS). An honest last-bit hardware difference "
    "and a one-ulp cheater fail the same bitwise comparison, so the suspension is absorbing "
    "for both",
)
def test_a_miner_suspended_by_a_trial_mismatch_has_a_defined_way_back(
    ac_clone, tmp_path: Path
) -> None:
    """A trial mismatch is not a proven fraud: the suspended miner needs a defined way back to
    PROBATION however long it waits, one that keeps its strikes (forgiveness that erases them
    could be farmed). The one-ulp double stands for an honest last-bit hardware difference,
    which the master cannot tell from it. A new identity for the same coldkey is not a way
    back: it would leave the old stake frozen and give free retries."""
    # Given: a miner suspended by a one-ulp difference
    world, miner, cheat = ac_clone, apply(ac_clone, NEWCOMER), BY_NAME["altered-weights-one-ulp"]
    result = probe_with_double(world, miner, cheat)
    suspended = submit(miner, result, tmp_path)
    precondition(suspended["state"] == "SUSPENDED", f"the double was not caught: {suspended}")
    reference = pending_reference(world, miner)
    # When: a long time later it uses the documented recovery route
    world.network.push(
        world.network.now + 10 * world.network.setup.admission_policy.challenge_rounds
    )
    _, recovery = attempt(recover, world, miner, reference)
    after = standing(miner)
    _, restart = attempt(join, world, world.miner(OUTSIDER), NEWCOMER)
    evidence(
        row_of(cheat),
        (200, None),
        admission=after,
        trial_outcome=trial_outcome(world, miner),
        recover=None if recovery is None else [recovery.status, recovery.detail],
        restart=None if restart is None else [restart.status, restart.detail],
        hole="PRB-01",
    )
    # Then: it is back in the admission pipeline, with its strike
    assert after["state"] != "SUSPENDED", f"recover: {recovery}; restart: {restart}"
    assert after["strikes"] >= 1, after
