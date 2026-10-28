"""Admission defences over the signed HTTP routes of the real in-process service.

RTA-01..RTA-05 and RTA-10 come from the red-team report (Lens, 2026-10-27), which proved them
against the in-process `Admission` API only. Each hole below is an xfail(strict, raises=
AssertionError) test whose body asserts the DESIRED defence; its control passes today (RTA-04's
control is the existing tests/challenge/test_trial_export.py::
test_job_route_refuses_another_miners_admission). A broken scenario step raises
PreconditionError, which is not an AssertionError, so it cannot hide behind an expected failure.
The reference trial is run once per module and cloned per test.

The fixes for RTA-01..05 and RTA-10 are not on this base (`65be1962`): the lot that carries them
is the branch `v3-admission-hardening` (Forge, 2026-10-27). Until it is merged these strict xfails
XFAIL; on the day it is, each XPASS(strict) fails the run and the marker is removed, which turns
the test into a normal regression test (RTA-04: `owner_challenge` must then sign as the owner).
"""

from __future__ import annotations

import importlib.util
import shutil
import sqlite3
import sys
from contextlib import closing
from pathlib import Path
from types import SimpleNamespace

import pytest
from ac_precondition import precondition
from fastapi.testclient import TestClient

from env_pin_fixtures import honest_env
from hypertrain.miner.admission import sign_join
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import HardwareHint, WorkProof, WorkScreenV2

_spec = importlib.util.spec_from_file_location(
    "admission_defences_service_fixture",
    Path(__file__).parents[1] / "challenge/test_service_network_v2.py",
)
assert _spec is not None and _spec.loader is not None
svc = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = svc
_spec.loader.exec_module(svc)

# The fixture registers four hotkeys on the fake metagraph; scenarios need more of them.
EXTRA_HOT = [Keypair(bytes([130 + i]) * 32) for i in range(40)]
EXTRA_COLD = [Keypair(bytes([170 + i]) * 32) for i in range(40)]


@pytest.fixture
def net(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr(svc, "HOT", [*svc.HOT, *EXTRA_HOT])
    yield from svc.network.__wrapped__(tmp_path, SimpleNamespace(param=None))


def apply_to_join(n, hot: Keypair, cold: Keypair, label: str):
    request = sign_join(
        hot,
        cold,
        run_id=n.manifest.run_id(),
        request_id=sha256_hex(label.encode()),
        expires_beacon=10000,
        policy_hash=n.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
        env=honest_env(),
    )
    return n.client.post(n.url + "/join", content=canonicalize(request.body()))


def rotate_to(
    n, cold: Keypair, hot: Keypair, new_hot: Keypair, label: str, consent: Keypair | None = None
):
    """Rotate `hot` to `new_hot`: the coldkey seals the request, `consent` (default the new hotkey)
    signs the new hotkey's agreement (RTA-01)."""
    from hypertrain.miner.admission import sign_rotation_consent

    request = sign_rotation_consent(
        consent or new_hot,
        run_id=n.manifest.run_id(),
        coldkey=cold.ss58,
        hotkey=hot.ss58,
        operation_id=sha256_hex(label.encode()),
        expires_beacon=10000,
    ).model_copy(update={"new_hotkey": new_hot.ss58})
    return n.client.post(
        n.url + "/admission/" + hot.ss58 + "/rotate", json=n.signed(cold, "RotateRequest", request)
    )


def stranger_challenge(n, admission_id: str):
    """Ask for the challenge of an admission with the signature of another hotkey: the RTA-04
    caller. The route takes a request signed in its body by the admission's own hotkey, so this
    one must be refused."""
    return n.challenge(admission_id, svc.HOT[3])


def owner_challenge(n, admission_id: str, owner: Keypair):
    """GET the challenge as the admission's owner: every honest poll of this module goes here.

    RTA-04: the route only checks that the admission's hotkey is registered, so `owner` is not
    used yet. Once the route needs the owner's signature this is the one place that signs: follow
    the miner-facing /job route, which takes the headers x-hotkey and x-trial-signature (the
    hotkey's signature over a per-route domain, the run id and the admission id; see `job()` in
    tests/challenge/test_trial_export.py), with whatever scheme the fix defines. A key other than
    the owner's, registered on the subnet, must be refused too.
    """
    return n.challenge(admission_id, owner)


def record_of(n, hotkey: Keypair) -> dict:
    response = n.client.get(n.url + "/admission/" + hotkey.ss58)
    assert response.status_code == 200, response.text
    return response.json()["record"]


def count(n, table: str, where: str = "1") -> int:
    return n.store._db.execute(f"SELECT COUNT(*) FROM {table} WHERE {where}").fetchone()[0]


def test_owner_coldkey_rotates_its_own_admission_to_another_registered_hotkey(net) -> None:
    # Given: an applied admission and a second hotkey of the same owner
    admission_id = net.join(0).json()["admission_id"]
    # When
    response = rotate_to(net, svc.COLD[0], svc.HOT[0], EXTRA_HOT[0], "own-rotation")
    # Then: the same admission continues under the new hotkey
    assert response.status_code == 200, response.text
    assert record_of(net, EXTRA_HOT[0])["admission_id"] == admission_id


def test_rotation_signed_by_a_foreign_coldkey_is_refused(net) -> None:
    # Given
    net.join(0)
    # When: another coldkey tries to move the admission of HOT[0]
    response = rotate_to(net, svc.COLD[1], svc.HOT[0], EXTRA_HOT[0], "foreign-rotation")
    # Then
    assert response.status_code == 409
    assert record_of(net, svc.HOT[0])["hotkey"] == svc.HOT[0].ss58
    assert count(net, "admission_history", f"hotkey='{EXTRA_HOT[0].ss58}'") == 0


def test_rotation_to_a_hotkey_that_did_not_consent_is_refused(net) -> None:
    # Given: a victim hotkey registered on the subnet that never applied
    net.join(0)
    victim_hot, victim_cold = EXTRA_HOT[1], EXTRA_COLD[1]
    # When: someone else's coldkey rotates its admission onto the victim hotkey
    response = rotate_to(net, svc.COLD[0], svc.HOT[0], victim_hot, "squat", consent=svc.HOT[0])
    # Then: refused, and the victim can still apply with its own keys
    assert 400 <= response.status_code < 500, response.text
    assert apply_to_join(net, victim_hot, victim_cold, "victim-join").status_code == 200


def test_rotation_chain_is_bounded_per_beacon(net) -> None:
    # Given: the join burst is the repository's bound for identity-creating requests
    burst = net.setup.admission_policy.join_burst
    net.join(0)
    # When: the owner keeps rotating onto fresh registered hotkeys within one beacon
    current, accepted = svc.HOT[0], 0
    for i in range(burst + 1):
        response = rotate_to(net, svc.COLD[0], current, EXTRA_HOT[10 + i], f"chain-{i}")
        if response.status_code == 200:
            accepted += 1
            current = EXTRA_HOT[10 + i]
    # Then
    assert accepted <= burst


def apply_many(n, places: int) -> None:
    for i in range(places):
        response = apply_to_join(n, EXTRA_HOT[i], EXTRA_COLD[i], f"unfunded-{i}")
        precondition(response.status_code == 200, f"application {i} refused: {response.text}")


def test_pending_places_are_bounded_and_a_refusal_changes_nothing(net) -> None:
    # Given: every pending place taken by applications from one source prefix
    places = net.setup.admission_policy.max_pending
    apply_many(net, places)
    # When: one more registered hotkey applies in the same beacon
    refused = net.join(0)
    # Then
    assert refused.status_code == 409 and "PENDING_CAPACITY" in refused.text
    assert count(net, "admissions_v2") == places


def test_idle_unfunded_applications_do_not_hold_the_pending_places_forever(net) -> None:
    # Given: every pending place taken, none of them funded or ever proved
    policy = net.setup.admission_policy
    apply_many(net, policy.max_pending)
    # When: far past the window in which an application can still open a trial
    net.push(net.now + 2 * policy.challenge_rounds)
    # Then: an honest funded identity can apply
    response = net.join(0)
    assert response.status_code == 200, response.text


def test_trial_places_are_bounded_and_the_owners_own_poll_frees_one(net) -> None:
    # Given: two applicants hold the whole trial capacity with unproved trials
    holders = [(net.join(i).json()["admission_id"], svc.HOT[i]) for i in (1, 2)]
    net.push(2)
    assert [owner_challenge(net, *holder).status_code for holder in holders] == [200, 200]
    third = net.join(3).json()["admission_id"]
    net.push(3)
    # When / Then: a third trial is refused and leaves no row behind
    refused = owner_challenge(net, third, svc.HOT[3])
    assert refused.status_code == 409 and "TRIAL_CAPACITY" in refused.text
    assert count(net, "admission_trials") == 2
    # When: past its deadline the first holder polls its own trial (the answer to that poll is
    # not part of the contract here: only the place it frees is)
    deadline = net.setup.admission_policy.challenge_rounds + 2
    net.push(deadline + 1)
    owner_challenge(net, *holders[0])
    # Then: the freed place goes to the third applicant
    assert owner_challenge(net, third, svc.HOT[3]).status_code == 200


def test_unproved_trials_release_their_places_at_the_deadline(net) -> None:
    # Given: two applicants open a trial and never prove it
    holders = [(net.join(i).json()["admission_id"], svc.HOT[i]) for i in (1, 2)]
    net.push(2)
    opened = [owner_challenge(net, *holder).status_code for holder in holders]
    precondition(opened == [200, 200], f"the two trials did not open: {opened}")
    # When: the deadline passes with nobody polling those admissions, and an honest one applies
    net.push(net.setup.admission_policy.challenge_rounds + 3)
    honest = net.join(0).json()["admission_id"]
    net.push(net.now + 1)
    # Then: the honest hotkey can open a trial
    response = owner_challenge(net, honest, svc.HOT[0])
    assert response.status_code == 200, response.text


def test_a_non_owner_cannot_open_a_rivals_trial(net) -> None:
    # Given: an applicant that has not asked for its challenge
    rival = net.join(1).json()["admission_id"]
    net.push(2)
    # When: a caller holding no key of the applicant requests the challenge
    response = stranger_challenge(net, rival)
    # Then: refused, as the signed /job route refuses it, and no trial slot is consumed
    assert response.status_code in (401, 403), response.text
    assert count(net, "admission_trials") == 0


def test_a_non_owner_cannot_expire_a_rivals_trial(net) -> None:
    # Given: an applicant opened its trial and has not proved it yet
    victim = svc.HOT[1]
    rival = net.join(1).json()["admission_id"]
    net.push(2)
    opened = owner_challenge(net, rival, victim).status_code
    precondition(opened == 200, f"the rival's own trial did not open: {opened}")
    net.push(net.setup.admission_policy.challenge_rounds + 3)
    # When: a caller holding no key of the applicant requests the challenge again
    stranger_challenge(net, rival)
    # Then: the applicant's standing is untouched
    assert record_of(net, victim)["state"] == "APPLIED"


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="RTA-10: ip_prefix_burst (64) exceeds max_pending (32), so the per-prefix quota "
    "cannot stop one source prefix from taking every pending place (Lens recommends "
    "ip_prefix_burst below max_pending; both are policy literals, so a fix is a policy change)",
)
def test_one_source_prefix_cannot_take_every_pending_place(net) -> None:
    # Given: applications of distinct registered identities, all from one client address
    places = net.setup.admission_policy.max_pending
    accepted = 0
    for i in range(places):
        response = apply_to_join(net, EXTRA_HOT[i], EXTRA_COLD[i], f"one-prefix-{i}")
        precondition(response.status_code < 500, f"application {i} failed: {response.text}")
        accepted += response.status_code == 200
    # Then: some place is left for applicants from other prefixes
    assert accepted < places


H = "ab" * 32
STATE_TABLES = (
    "admissions_v2",
    "admission_trials",
    "admission_trial_results",
    "admission_transitions",
)


@pytest.fixture(scope="module")
def reference(tmp_path_factory):
    """One genuine service reference and the miner's matching proof, snapshotted once."""
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.gpu_ops.work_screen import screen_work
    from hypertrain.protocol.messages_v2 import JoinChallenge

    generator = svc.network.__wrapped__(
        tmp_path_factory.mktemp("reference"), SimpleNamespace(param=None)
    )
    n = next(generator)
    try:
        identity = n.join(0).json()["admission_id"]
        n.push(2)
        challenge = JoinChallenge.model_validate(
            owner_challenge(n, identity, svc.HOT[0]).json()["body"]
        )
        response = n.client.post(
            n.url + "/admin/join/" + identity + "/reference", headers=svc.admin()
        )
        assert response.status_code == 200, response.text
        row = n.store._db.execute(
            "SELECT * FROM admissions_v2 WHERE admission_id=?", (identity,)
        ).fetchone()
        samples = trial_samples(
            n.manifest, identity, challenge.nonce, n.store._beacon_v2(challenge.seed_beacon)
        )
        job, staged = n.store.stage_trial_v2(
            n.manifest.run_id(), challenge, samples, row["trial_epoch"]
        )
        root = n.store.state_dir.parent
        work = root / "miner"
        work.mkdir()
        for relative in job.object_paths.values():
            shutil.copyfile(staged / relative, work / relative)
        screen, proof = screen_work(job, work, challenge, now_beacon=2, backend="cpu")
        assert proof == WorkProof.model_validate_json(row["reference_json"])
        for name in ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json"):
            n.store.objects.put((work / "published/rank-0" / name).read_bytes())
        snapshot = root / "snapshot"
        (snapshot / "state").mkdir(parents=True)
        with closing(sqlite3.connect(snapshot / "state/challenge.db")) as copied:
            n.store._db.backup(copied)
        for name in ("objects", "trials-v2", "ledger"):
            shutil.copytree(n.store.state_dir / name, snapshot / "state" / name)
        for name in ("admin", "coord.key"):
            shutil.copyfile(root / name, snapshot / name)
        return SimpleNamespace(
            snapshot=snapshot,
            manifest=n.manifest,
            setup=n.setup,
            origin_ids=n.origin_ids,
            params=n.store.params,
            verify_beacon=n.store.verify_beacon,
            identity=identity,
            challenge=challenge,
            proof=proof,
            screen=screen,
        )
    finally:
        generator.close()


@pytest.fixture
def trial(reference, tmp_path: Path):
    """A clone of the snapshot: challenge open, reference committed, no proof yet, beacon 2."""
    from hypertrain.challenge.app import Config, create_app
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.data.store import LocalFSStore

    root = tmp_path / "clone"
    shutil.copytree(reference.snapshot, root)
    state = root / "state"
    store = ChallengeStore(
        state,
        reference.params,
        svc.COORD,
        svc.OWNER.ss58,
        reference.verify_beacon,
        LocalFSStore(state / "objects"),
    )
    store.clock = lambda: reference.manifest.training.beacon.genesis_time + 3
    cfg = Config(
        "hypertrain",
        state,
        "https://master.test",
        None,
        root / "admin",
        None,
        root / "coord.key",
        svc.OWNER.ss58,
        reference.params,
    )
    app = create_app(
        cfg,
        verify_beacon=store.verify_beacon,
        _store=store,
        clock=store.clock,
        transport=svc.subnet(),
    )
    with TestClient(app) as client:
        n = svc.Network(client, reference.manifest, reference.setup, reference.origin_ids, 2)
        yield SimpleNamespace(
            n=n,
            identity=reference.identity,
            deadline=reference.challenge.deadline_beacon,
            proof=reference.proof,
            screen=reference.screen,
        )


def post_proof(n, proof: WorkProof, screen: WorkScreenV2, signer: Keypair | None = None):
    key = signer or svc.HOT[0]
    return n.client.post(
        n.url + "/join/proof",
        json={
            "proof": n.signed(key, "WorkProof", proof),
            "screen": n.signed(key, "WorkScreenV2", screen),
        },
    )


def stored_state(n) -> dict:
    return {
        table: [tuple(row) for row in n.store._db.execute(f"SELECT * FROM {table}")]
        for table in STATE_TABLES
    }


@pytest.mark.parametrize("fault", ["delta_hash", "artifact_ref"])
def test_a_proof_differing_from_the_reference_suspends_with_a_strike_and_binds_the_coldkey(
    trial, fault: str
) -> None:
    # Given: the miner's proof with one byte-level reference altered, signed consistently
    n, proof, screen = trial.n, trial.proof, trial.screen
    if fault == "delta_hash":
        proof = WorkProof.model_validate({**proof.body(), "delta_hash": H})
    else:
        refs = [ref.body() for ref in proof.artifact_refs]
        refs[-1] = {**refs[-1], "sha256": H}
        proof = WorkProof.model_validate({**proof.body(), "artifact_refs": refs})
        screen = WorkScreenV2.model_validate(
            {**screen.body(), "artifact_hashes": [ref.sha256 for ref in proof.artifact_refs]}
        )
    # When
    response = post_proof(n, proof, screen)
    # Then: suspended pending dispute with a strike, trial recorded as a mismatch
    assert response.status_code == 200, response.text
    record = response.json()
    assert (record["state"], record["reason"]) == ("SUSPENDED", "PENDING_REPLAY_MISMATCH")
    assert record["strikes"] == 1 and record["pending_dispute"]
    outcome = n.store._db.execute("SELECT outcome FROM admission_trial_results").fetchone()[0]
    assert outcome == "MISMATCH"
    # And: it cannot open another trial
    closed = owner_challenge(n, trial.identity, svc.HOT[0])
    assert closed.status_code == 409 and "CHALLENGE_STATE" in closed.text
    # And: the coldkey cannot start over with a fresh hotkey
    again = apply_to_join(n, svc.HOT[2], svc.COLD[0], "fresh-hotkey")
    assert again.status_code == 409 and "COLDKEY_PROBATION_EXISTS" in again.text


@pytest.mark.parametrize(
    "fault,code",
    [
        ("challenge", "WORK_SCREEN_BINDING"),
        ("screen-challenge", "WORK_SCREEN_BINDING"),
        ("nonce", "WORK_SCREEN_BINDING"),
        ("layout", "WORK_SCREEN_BINDING"),
        ("artifact", "WORK_SCREEN_BINDING"),
        ("late", "PROOF_STATE_OR_DEADLINE"),
    ],
)
def test_a_proof_not_bound_to_the_open_trial_is_refused_without_state_change(
    trial, fault: str, code: str
) -> None:
    # Given
    n, proof, screen = trial.n, trial.proof, trial.screen
    if fault == "challenge":
        proof = WorkProof.model_validate({**proof.body(), "challenge_hash": H})
    if fault == "screen-challenge":
        screen = WorkScreenV2.model_validate({**screen.body(), "challenge_hash": H})
    if fault == "nonce":
        screen = WorkScreenV2.model_validate({**screen.body(), "nonce": H})
    if fault == "layout":
        layout = {**screen.layout.model_dump(mode="json"), "zero1": True}
        screen = WorkScreenV2.model_validate({**screen.body(), "layout": layout})
    if fault == "artifact":
        screen = WorkScreenV2.model_validate({**screen.body(), "artifact_hashes": [H]})
    if fault == "late":
        n.push(trial.deadline + 1)
    before = stored_state(n)
    # When
    response = post_proof(n, proof, screen)
    # Then
    assert response.status_code == 409 and code in response.text
    assert stored_state(n) == before


def test_a_proof_signed_by_another_registered_key_is_refused(trial) -> None:
    # Given: a peer's key signs the proof and screen of someone else's admission
    before = stored_state(trial.n)
    # When
    response = post_proof(trial.n, trial.proof, trial.screen, signer=svc.HOT[1])
    # Then
    assert response.status_code == 409 and "not authorized" in response.text
    assert stored_state(trial.n) == before


def test_a_replayed_finalization_counts_one_clean_participation(trial) -> None:
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.store import get_large
    from hypertrain.protocol.messages import Finalize
    from hypertrain.trainer.compress import state_hash

    # Given: an honest proof, finalized by the service itself
    n = trial.n
    response = post_proof(n, trial.proof, trial.screen)
    assert response.status_code == 200, response.text
    assert response.json()["clean_count"] == 1
    theta, _ = unpack_state(get_large(trial.proof.artifact_refs[0].sha256, n.store.objects.get))
    final = Finalize(
        w=n.store._db.execute("SELECT trial_epoch FROM admissions_v2").fetchone()[0],
        final_theta_hash_w1=state_hash(theta),
        included=[svc.HOT[0].ss58],
        entitlements_root=trial.proof.digest(),
    )
    wire = n.signed(svc.COORD, "Finalize", final)
    # When: the operator route receives the same signed finalization twice
    replies = [
        n.client.post(
            n.url + "/admin/join/" + trial.identity + "/finalize", json=wire, headers=svc.admin()
        )
        for _ in range(2)
    ]
    # Then: both answer, and the participation is still counted once
    assert [r.status_code for r in replies] == [200, 200]
    assert replies[1].json()["record"]["clean_count"] == 1
    assert record_of(n, svc.HOT[0])["clean_count"] == 1


def test_a_finalized_proof_is_not_suspended_by_a_challenge_poll_after_its_deadline(trial) -> None:
    # Given: the honest proof lands on the last round of the window and is finalized with it
    n = trial.n
    n.push(trial.deadline)
    response = post_proof(n, trial.proof, trial.screen)
    assert response.status_code == 200, response.text
    assert response.json()["clean_count"] == 1
    # When: the deadline has passed and the admission is polled again
    n.push(trial.deadline + 1)
    poll = owner_challenge(n, trial.identity, svc.HOT[0])
    # Then: the miner keeps its standing and gets its next trial
    assert poll.status_code == 200, poll.text
    record = record_of(n, svc.HOT[0])
    assert (record["state"], record["clean_count"]) == ("PROBATION", 1)


def test_an_accepted_proof_is_not_suspended_before_its_trial_is_finalized(
    trial, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: the proof lands on the last round, then the beacon feed pauses (the clock moves on
    # without a new beacon) so the service cannot finalize it in the same request
    n = trial.n
    n.push(trial.deadline)
    accept = n.store.trial_proof_v2

    def accept_then_stall_beacons(*args):
        accepted = accept(*args)
        n.store.clock = lambda: n.manifest.training.beacon.genesis_time + (trial.deadline + 20) * 3
        return accepted

    monkeypatch.setattr(n.store, "trial_proof_v2", accept_then_stall_beacons)
    response = post_proof(n, trial.proof, trial.screen)
    precondition(response.status_code == 503, f"finalization was not stalled: {response.text}")
    row = n.store._db.execute("SELECT proof_digest, state FROM admissions_v2").fetchone()
    precondition(
        row["proof_digest"] is not None and row["state"] == "PROBATION",
        f"the proof was not accepted before the stall: {tuple(row)}",
    )
    # When: the feed recovers after the deadline and its owner polls the admission (RTA-04 or not,
    # the owner's own poll must not suspend an accepted proof)
    n.push(trial.deadline + 1)
    owner_challenge(n, trial.identity, svc.HOT[0])
    # Then: the accepted proof still stands
    assert record_of(n, svc.HOT[0])["state"] == "PROBATION"
