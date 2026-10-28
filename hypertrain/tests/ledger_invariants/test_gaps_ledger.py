"""Gaps of the ledger group (D39, gap wave): what the first wave left open in the credit paths.

Each test states a defence the protocol must hold ("refuses X and the state is unchanged",
"credits once", "a stored answer is returned whatever the beacon is now"). A hole is a strict
xfail whose body first runs a control that proves the scenario reaches the point under test, then
asserts the defence; a decision the maintainers have not taken is named in the reason.
"""

from __future__ import annotations

import os
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

import hypertrain
from hypertrain.challenge.app import Config
from hypertrain.challenge.store import ChallengeError, rerun_message
from hypertrain.ledger import FULL_SHARE_MASS
from hypertrain.ledger.escrow_v2 import digest
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize

from . import support_audit as sa
from .support import SERVICE_FIXTURE, SLUG, load_fixture_module, service_world
from .support_audit import settled_state
from .support_credit import (
    BUDGET,
    Plan,
    build_audited_round,
    build_plan,
    credited,
    database_digest,
    event_count,
    ledger_for,
    ledger_rows,
    miner,
    refusal,
    rotation_envelope,
)
from .support_gaps_ledger import (
    ENV_SCRUB,
    POISONED_BEARER,
    POISONED_SLUG,
    Frozen,
    environment_names,
    execute_round,
    finality,
    live_origins,
    package_sources,
    polluted_environment,
    post_finalize,
    retire_like_a_rollback,
    round_body,
    scrub_environment,
    v1_mismatch,
    weights_answer,
)
from .support_locks import BOND, STAKE, Rig, units_of
from .support_weights import parse, pay_events
from .test_audit import audit_world  # noqa: F401  (the session fixture of test_audit.py)

SRC = Path(hypertrain.__file__).resolve().parent
service = load_fixture_module(*SERVICE_FIXTURE)
# The fixture of the service module, bound here so that pytest discovers it for this module.
network = service.network
E2E_FIXTURE = ("li_service_e2e", "e2e/test_service_network_v2_e2e.py")


@pytest.fixture(autouse=True)
def scrubbed_environment(monkeypatch: pytest.MonkeyPatch) -> None:
    """M-17: no ambient CHALLENGE_* or HYPERTRAIN_* value reaches a test of this module."""
    scrub_environment(monkeypatch)


# --- M-17: the environment the lot runs in is scrubbed, and the scrub list cannot go stale ------

KNOBS = """
import os

class Config:
    @classmethod
    def from_env(cls):
        def path(name):
            value = os.environ.get(name)
            return value

        env = os.environ.get
        return cls(
            a=env("ALPHA", "x"),
            b=path("BETA"),
            c=os.getenv("GAMMA"),
            d=os.environ["DELTA"],
            e=env(computed_name),
            f=os.environ.copy(),
        )
"""


def test_the_environment_scan_finds_literal_names_through_aliases_and_helpers() -> None:
    """M-17 (the scan itself): a literal name read through `os.environ.get`, `os.getenv`,
    `os.environ[...]`, an alias of `get` or a helper that forwards its parameter is found; a
    computed name and a whole-environment copy are not names. One more knob is reported."""
    assert environment_names(KNOBS) == {"ALPHA", "BETA", "GAMMA", "DELTA"}
    assert environment_names(KNOBS + '\nextra = os.environ.get("EPSILON")\n') == {
        "ALPHA",
        "BETA",
        "GAMMA",
        "DELTA",
        "EPSILON",
    }


def test_the_scrub_list_names_every_environment_variable_the_challenge_and_ledger_read() -> None:
    """M-17: the names `Config.from_env` and any other module of hypertrain.challenge and
    hypertrain.ledger read by literal name are exactly the names the lot scrubs, so a knob added
    to the service is either scrubbed or this test fails."""
    read: dict[str, list[str]] = {}
    for module, source in package_sources(SRC, "challenge", "ledger"):
        for name in environment_names(source):
            read.setdefault(name, []).append(module)
    assert {name: where for name, where in read.items() if name not in ENV_SCRUB} == {}
    assert set(ENV_SCRUB) - set(read) == set()


def test_the_scrub_removes_every_name_the_service_reads_and_leaves_its_defaults(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """M-17: with every variable `Config.from_env` reads set, the scrub leaves the service on its
    own defaults. The names come from the source, not from the scrub list, so a name missing
    from the list stays set and the defaults differ."""
    names = environment_names((SRC / "challenge/app.py").read_text())
    for name in names:
        monkeypatch.setenv(name, "7")
    assert names <= set(os.environ)  # control: the ambient environment is fully polluted
    scrub_environment(monkeypatch)
    assert names.isdisjoint(os.environ)
    config = Config.from_env()
    assert (config.slug, config.state_dir, config.master_url, config.netuid) == (
        "hypertrain",
        Path("/data"),
        "http://cortex-master:8080",
        100,
    )
    assert (
        config.internal_token_file,
        config.admin_token_file,
        config.worker_token_file,
        config.coord_key_file,
        config.owner_hotkey,
        config.role_dispatch_file,
        config.cuda_executor_file,
    ) == (None,) * 7


def test_a_service_the_lot_builds_ignores_a_polluted_environment(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """M-17: with a state directory, bearer files and a slug of somebody else in the environment,
    the world the lot builds keeps its own state directory, answers to its own bearer and slug,
    refuses the polluted ones, and writes nothing under the polluted directory."""
    poison = tmp_path / "poison"
    for name, value in polluted_environment(poison).items():
        monkeypatch.setenv(name, value)
    polluted = Config.from_env()  # control: the pollution is what a service built from env reads
    assert (polluted.slug, polluted.state_dir, polluted.netuid) == (
        POISONED_SLUG,
        poison / "state",
        7,
    )
    assert polluted.internal_token_file == poison / "internal"
    before = sorted(path.name for path in poison.rglob("*"))
    with service_world(tmp_path / "world") as world:
        assert world.store.state_dir.is_relative_to(tmp_path / "world")
        answer = world.get_weights(0)
        assert (answer.status_code, answer.json()["challenge_slug"]) == (200, SLUG)
        impostor = world.get_weights(
            0,
            headers={
                "Authorization": f"Bearer {POISONED_BEARER}",
                "x-platform-challenge-slug": POISONED_SLUG,
            },
        )
        assert impostor.status_code == 401
    assert sorted(path.name for path in poison.rglob("*")) == before


# --- GAP-10: a v1 MISMATCH is not downgraded by a claim copied from the published verdict ---------


def standing(net: Any, victim: Keypair) -> tuple[str, list[str], int]:
    """(status of the victim's round-0 row, hotkeys under a forfeit, roster blacklist flag)."""
    view = net.round(0)
    row = next(m for m in view["miners"] if m["hotkey"] == victim.ss58)
    store = net.c.app.state.store
    flag = store._db.execute(
        "SELECT blacklisted FROM roster WHERE run_id=? AND hotkey=?", (net.run_id, victim.ss58)
    ).fetchone()[0]
    return row["status"], [f["body"]["hotkey"] for f in view["forfeits"]], int(flag)


def rerun(net: Any, victim: Keypair, root: str) -> Any:
    body = {
        "w": 0,
        "hotkey": victim.ss58,
        "leaves_root": root,
        "sig": victim.sign(rerun_message(net.run_id, 0, victim.ss58, root)).hex(),
    }
    return net.post("rerun", body)


@pytest.mark.parametrize(
    ("which", "classification"),
    [("another root", "FAULT"), ("its own commit", "CONTEST")],
)
def test_a_rerun_that_is_not_the_auditors_root_keeps_the_forfeit_and_the_blacklist(
    tmp_path: Path, which: str, classification: str
) -> None:
    """GAP-10 (the half that holds): a rerun root that is neither the auditor's nor reproducible is
    a FAULT, one that stands by the commitment is a CONTEST; neither clears the forfeit or the
    blacklist, and a second rerun is refused."""
    with v1_mismatch(tmp_path) as (_, net, victim):
        row = next(m for m in net.round(0)["miners"] if m["hotkey"] == victim.ss58)
        committed = row["commit"]["body"]["leaves_root"]
        assert standing(net, victim) == ("MISMATCH", [victim.ss58], 1)
        root = committed if which == "its own commit" else "ab" * 32
        answer = rerun(net, victim, root)
        assert (answer.status_code, answer.json()["classification"]) == (200, classification)
        assert standing(net, victim) == ("MISMATCH", [victim.ss58], 1)
        again = rerun(net, victim, root)
        assert (again.status_code, again.json()["detail"]) == (409, "rerun already submitted")
        assert standing(net, victim) == ("MISMATCH", [victim.ss58], 1)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="GAP-10: a rerun claim that is only the recomputed_leaves_root printed in the "
    "published verdict is classified TRANSIENT, which clears the blacklist and deletes the "
    "forfeit; the route asks for no evidence the claimant could not copy. Decision needed "
    "(ultrabrain section 4): the evidence a legitimate rerun must carry; the existing "
    "tests/challenge/test_fix1.py::test_transient_rerun_route pins the copyable form "
    "[fix-lot: decision (product choice pending)]",
)
def test_a_rerun_claim_copied_from_the_published_verdict_does_not_clear_a_mismatch(
    tmp_path: Path,
) -> None:
    """GAP-10 (v1): the claim that reclassifies a MISMATCH is built from public data alone, so it
    must not clear the forfeit and the blacklist unless it carries evidence the claimant could not
    copy (which evidence is the decision; the copyable form must not be enough)."""
    with v1_mismatch(tmp_path) as (_, net, victim):
        row = next(m for m in net.round(0)["miners"] if m["hotkey"] == victim.ss58)
        published = row["verdict"]["body"]["recomputed_leaves_root"]
        # Control: the verdict is public, differs from the commitment, and the victim is
        # blacklisted under a forfeit; the claim below is signed by the victim and well formed.
        assert published != row["commit"]["body"]["leaves_root"]
        assert standing(net, victim) == ("MISMATCH", [victim.ss58], 1)
        answer = rerun(net, victim, published)
        assert answer.status_code == 200
        status, forfeited, blacklisted = standing(net, victim)
        assert (status, forfeited, blacklisted) == ("MISMATCH", [victim.ss58], 1), (
            f"classified {answer.json()['classification']}: status {status}, "
            f"forfeits {forfeited}, blacklisted {blacklisted}"
        )


# --- the store on a real audited and aggregated round (M-01, T-CRD-16, T-CRD-18, GAP-03) ---------


@pytest.fixture(scope="module")
def audited(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Frozen]:
    for round_ in build_audited_round(tmp_path_factory):
        yield Frozen.take(round_, tmp_path_factory.mktemp("v1-journal") / "ledger")


def test_an_identical_finalize_at_a_later_beacon_returns_the_stored_answer_and_changes_nothing(
    audited: Frozen,
) -> None:
    """M-01 (FIN-05, CRD-02): once round 0 is credited, the Finalize the coordinator sent is
    answered from the stored result whatever the beacon is now. The ledger could not do it: the
    vesting it would be asked for moves with the beacon, so a replay that reached it would be a
    different operation. Another body stays a conflict at the later beacon too."""
    round_ = audited.fresh()
    store, network = round_.store, round_.network
    escrow = store._services(round_.run_id)[0]
    request = round_.finalize_request()
    first = post_finalize(round_, request)
    assert first.status_code == 200 and first.json()["status"] == "SETTLED", first.text
    accepted = store._record_v2(round_.run_id, "replayed-finality", "0")
    assert (accepted["finalized_beacon"], event_count(escrow, "REWARD_FINALIZE")) == (
        network.now,
        1,
    )
    network.push(network.now + 3)
    settled = database_digest(store)

    again = post_finalize(round_, request)

    assert (again.status_code, again.json()) == (200, first.json())
    assert database_digest(store) == settled
    assert store._record_v2(round_.run_id, "replayed-finality", "0") == accepted
    assert event_count(escrow, "REWARD_FINALIZE") == 1 and len(credited(round_)) == 4
    other = round_.finalize(entitlements_root="22" * 32)
    assert (other.status_code, other.json()["detail"]) == (
        409,
        "conflicting signed operation nonce",
    )
    assert database_digest(store) == settled


def test_a_stored_finalize_is_not_answered_to_an_expired_envelope_and_is_to_a_fresh_one(
    audited: Frozen,
) -> None:
    """M-01 (the condition, FIN-05): the stored answer is returned to an envelope the coordinator
    still stands behind, not to one whose expiry has passed (authentication comes first); the same
    body signed again for later is answered, with nothing written either time."""
    round_ = audited.fresh()
    store, network = round_.store, round_.network
    short = network.signed(
        round_.fixtures.COORD,
        "Finalize",
        Finalize(**round_.finalize_request()["body"]),
        network.now + 2,
    )
    first = post_finalize(round_, short)
    assert first.status_code == 200, first.text
    network.push(network.now + 3)
    settled = database_digest(store)

    late = post_finalize(round_, short)

    assert (late.status_code, late.json()["detail"]) == (409, "envelope expired")
    assert database_digest(store) == settled
    renewed = post_finalize(round_, round_.finalize_request())
    assert (renewed.status_code, renewed.json()) == (200, first.json())
    assert database_digest(store) == settled


def test_the_store_vests_each_reward_origin_a_fixed_span_after_the_beacon_it_finalized_at(
    audited: Frozen,
) -> None:
    """GAP-03 (CRD-20, the store half): the ledger only refuses a maturity below the vesting the
    store hands it, so the span the store hands it is what a contest has to fit in. Every origin
    of a round matures E_vest_rounds beacons after the beacon of the Finalize, not after the
    audits or the opening; the Finalize is sent some beacons late so that those differ."""
    round_ = audited.fresh()
    network = round_.network
    network.push(network.now + 5)
    response = round_.finalize()
    assert response.status_code == 200, response.text
    span = network.manifest.training.verify.E_vest_rounds
    finalized = round_.store._record_v2(round_.run_id, "replayed-finality", "0")["finalized_beacon"]
    assert finalized == network.now > audited.now
    origins = live_origins(round_)
    assert len(origins) == 4 and {row["issued"] for row in origins} == {BUDGET // 4}
    assert {row["mature_at"] for row in origins} == {finalized + span}


def test_a_finalize_after_d_final_settles_the_round_or_retires_it_but_does_not_freeze_it(
    audited: Frozen,
) -> None:
    """T-CRD-16 (b, CRD-15): control, a Finalize at beacon d_final settles. A Finalize one beacon
    later, freshly signed, is either accepted or ends the round some other way (the audit
    challenge is judged at the beacon it was accepted at, ADR 0014 part 3); what it must not do is
    leave the round PENDING with every retry refused."""
    round_ = audited.fresh()
    d_final = int(round_body(round_)["d_final"])
    round_.network.push(d_final)
    control = round_.finalize()  # the last beacon the challenges cover: settled
    assert (control.status_code, control.json()["status"]) == (200, "SETTLED")

    round_ = audited.fresh()
    round_.network.push(d_final + 1)
    late = round_.finalize()
    disposition = finality(round_)["disposition"]
    settled = late.status_code == 200 and late.json()["status"] == "SETTLED"
    assert settled or disposition != "PENDING", (
        f"HTTP {late.status_code} {late.json().get('detail')}; the round is still {disposition} "
        f"and {len(credited(round_))} origins exist"
    )


# --- T-CRD-18 / CL-29: units earned before a hotkey rotation, on the real finalize path ---------


def rotate_first_miner(round_: Any) -> Any:
    """The signed rotation of the first miner's admission to the registered REFEREE hotkey, with
    the new hotkey's consent where the service asks for it."""
    fixtures, network = round_.fixtures, round_.network
    return network.client.post(
        network.url + "/admission/" + fixtures.HOT[0].ss58 + "/rotate",
        json=rotation_envelope(
            round_.run_id, fixtures.COLD[0], fixtures.HOT[0].ss58, fixtures.REFEREE, "df" * 32
        ),
    )


def current_hotkey(round_: Any, hotkey: str) -> str:
    """The hotkey the admission that `hotkey` was issued to holds now."""
    row = round_.store._db.execute(
        "SELECT a.hotkey FROM admission_history h JOIN admissions_v2 a "
        "ON a.admission_id=h.admission_id WHERE h.hotkey=?",
        (hotkey,),
    ).fetchone()
    assert row is not None, "the hotkey has no admission"
    return str(row[0])


def credited_and_rotated(audited: Frozen) -> Any:
    """Round 0 credited to the four miners, then the first miner's admission rotated to REFEREE
    (accepted today), and the beacon moved past the maturity of the four origins."""
    round_ = audited.fresh()
    finalized = post_finalize(round_, round_.finalize_request())
    assert finalized.status_code == 200, finalized.text
    mature = {row["mature_at"] for row in live_origins(round_)}
    assert len(mature) == 1 and len(credited(round_)) == 4
    rotation = rotate_first_miner(round_)
    assert rotation.status_code in (200, 409), rotation.text
    round_.network.push(mature.pop() + 1)
    return round_


def test_a_rotation_between_the_credit_and_the_pay_keeps_every_unit_with_the_coldkey_that_earned_it(
    audited: Frozen,
) -> None:
    """T-CRD-18 (CRD-12, the ownership half): whichever hotkey the weights name for a rotated
    miner, the units of its origin stay under its coldkey through the payment, the answer carries
    exactly the round's mass, and the ledger conserves."""
    round_ = credited_and_rotated(audited)
    fixtures = round_.fixtures
    escrow = round_.store._services(round_.run_id)[0]
    answer = weights_answer(round_, 0)
    assert sum(answer["weights"].values()) + answer["metadata"]["units_burned_this_epoch"] == BUDGET
    owners: dict[str, set[str]] = {}
    for row in escrow.db.execute(
        "SELECT o.hotkey,u.owner FROM escrow_units u JOIN escrow_origins o ON o.origin=u.origin "
        "WHERE o.hotkey IS NOT NULL"
    ):
        owners.setdefault(row["hotkey"], set()).add(row["owner"])
    assert owners == {
        key.ss58: {cold.ss58} for key, cold in zip(fixtures.HOT[:4], fixtures.COLD, strict=False)
    }
    assert escrow.balances().conserved()


def test_the_units_of_a_rotated_miner_are_emitted_to_the_hotkey_its_admission_holds_now(
    audited: Frozen,
) -> None:
    """T-CRD-18 (CRD-12, WGT-13, CL-29): four miners credited by the real Finalize, the first one's
    admission rotated to another registered hotkey, the origins matured. Whichever rule is chosen
    (pay the hotkey the admission holds now, or refuse the rotation while origins are owed), the
    quarter of the rotated miner is emitted to the hotkey the admission holds, and the round's
    mass is paid whole."""
    round_ = credited_and_rotated(audited)
    fixtures = round_.fixtures
    first = fixtures.HOT[0].ss58
    current = current_hotkey(round_, first)
    assert current in (first, fixtures.REFEREE.ss58)  # control: the rotation took one branch
    answer = weights_answer(round_, 0)
    expected = {
        (current if key.ss58 == first else key.ss58): float(BUDGET // 4) for key in fixtures.HOT[:4]
    }
    meta = answer["metadata"]
    assert (answer["weights"], meta["units_burned_this_epoch"]) == (expected, 0), (
        f"the admission holds {current}; weights {answer['weights']}, "
        f"burned {meta['units_burned_this_epoch']}"
    )


# --- T-CRD-04: a challenge issued for another round is not the audit of this one -----------------


@pytest.fixture(scope="module")
def setup(li_escrow: ModuleType) -> Any:
    return li_escrow.setup("production")


def rebind(plan: Plan, **challenge: Any) -> None:
    """Change the challenge and keep the verdict bound to it, so only the change can refuse."""
    item = plan.items[0]
    item.challenge = item.challenge.model_copy(update=challenge)
    item.verdict = item.verdict.model_copy(update={"challenge_hash": digest(item.challenge)})


@pytest.mark.parametrize("name", ["challenge_of_another_round", "control"])
def test_a_challenge_issued_for_another_round_is_no_audit_of_this_round_and_credits_nothing(
    li_escrow: ModuleType, setup: Any, name: str
) -> None:
    """T-CRD-04 (3, the round half of 'target or w'): the coordinator signed this challenge, the
    pinned auditor answered it with a MATCH, and the commit and the Finalize are of round 0; only
    the round the challenge was issued for differs. The ledger refuses it as a broken audit
    binding and writes nothing; the same evidence with the right round is credited."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    if name == "challenge_of_another_round":
        rebind(plan, w=1)
    reward, evidence = plan.build()
    escrow = ledger_for(li_escrow, setup, plan)
    if name == "control":
        escrow.reward_finalize(reward, evidence)
        assert escrow.balances().issued == BUDGET
        return
    rows = ledger_rows(escrow)
    assert refusal(lambda: escrow.reward_finalize(reward, evidence)) == "REWARD_AUDIT_BINDING"
    assert ledger_rows(escrow) == rows


# --- T-CRD-12: a repeated audit completion decrements the open-audit counter once ----------------


def test_a_repeated_audit_completion_neither_decrements_the_counter_again_nor_changes_anything(
    world: sa.World,
) -> None:
    """T-CRD-12 (4, the counter leg): the completion of an honest lease answers MATCH and takes the
    counter of open audits from five to four; the identical completion again answers the same and
    leaves four (a second decrement would hide until the last lease, where the counter is clamped
    at zero), and so do a contrary verdict (409) and a second scheduling of the audits (an empty
    job list). Not one of the three changes a lease, a record or the counter."""
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()

        def counter() -> int:
            return int(store._record_v2(run, "finality", "0")["unresolved_audits"])

        honest = next(r for h, r in sa.running(store).items() if h != cast.forger.ss58)
        key = cast.keys[honest["auditor"]]
        stored = store._record_v2(run, "executed-verdict", honest["id"])
        assert stored["result"] == "MATCH" and counter() == 5  # control: five open, ours executed

        first = sa.complete(n, key, honest["id"], stored)

        assert (first.status_code, first.json()) == (200, {"result": "MATCH"})
        assert counter() == 4 and sa.lease_rows(store)[honest["hotkey"]]["state"] == "completed"
        completed = (settled_state(store), sa.digest_of(store))
        repeated = sa.complete(n, key, honest["id"], stored)
        assert (repeated.status_code, repeated.json()) == (200, {"result": "MATCH"})
        assert counter() == 4 and (settled_state(store), sa.digest_of(store)) == completed
        contrary = {**stored, "result": "MISMATCH", "first_bad_leaf": 0}
        refused = sa.complete(n, key, honest["id"], contrary)
        assert sa.detail(refused) == (409, "conflicting signed operation nonce")
        assert counter() == 4 and (settled_state(store), sa.digest_of(store)) == completed
        scheduled = sa.schedule(n)
        assert (scheduled.status_code, scheduled.json()["jobs"]) == (200, [])
        assert counter() == 4 and (settled_state(store), sa.digest_of(store)) == completed


# --- T-CRD-13 / CL-04: the first round after a rollback is credited ----------------------------


def test_the_first_round_after_a_rollback_is_credited_through_the_finalize_route(
    audited: Frozen,
) -> None:
    """T-CRD-13 (CRD-04, the store half; about two minutes): rounds 0 and 1 are real, applied and
    audited, then closed without reward as a finished rollback leaves them; round 2 is run for the
    four remaining miners. Its Finalize is accepted: four origins of a quarter each, round 2
    settled. A round that is neither credited nor closed still blocks the next one (test_credit)."""
    round_ = audited.fresh()
    store, network = round_.store, round_.network
    execute_round(round_, 1)
    cause = "ab" * 32
    retire_like_a_rollback(round_, 0, cause)
    retire_like_a_rollback(round_, 1, cause)
    applied = execute_round(round_, 2)
    # Control: the two rounds are closed, round 2 is applied with nothing open, and nothing is paid.
    assert [finality(round_, w)["disposition"] for w in (0, 1, 2)] == ["EXCLUDED"] * 2 + ["PENDING"]
    assert all(finality(round_, w)["rollback_complete"] for w in (0, 1))
    assert (finality(round_, 2)["unresolved_audits"], credited(round_)) == (0, [])
    hotkeys = sorted(key.ss58 for key in round_.fixtures.HOT[:4])
    final = Finalize(
        w=2,
        final_theta_hash_w1=applied["theta_hash"],
        included=hotkeys,
        entitlements_root="11" * 32,
    )

    response = network.client.post(
        network.url + "/admin/rounds/2/finalize",
        json=network.signed(round_.fixtures.COORD, "Finalize", final),
        headers=round_.fixtures.admin(),
    )

    detail = response.json().get("detail")
    assert response.status_code == 200, f"HTTP {response.status_code} {detail}"
    assert response.json()["status"] == "SETTLED"
    assert [
        (row["hotkey"], row["reward_round"], row["issued"]) for row in live_origins(round_)
    ] == [(hotkey, 2, BUDGET // 4) for hotkey in hotkeys]
    assert store._record_v2(round_.run_id, "finality", "2")["disposition"] == "SETTLED"


# --- T-CRD-20: proven fraud against an admitted miner and its admission stake ----------------


@pytest.fixture
def rig(tmp_path: Path) -> Iterator[Rig]:
    with service_world(tmp_path) as world:
        yield Rig(world)


def lose_a_contest(rig: Rig, loser_index: int = 0) -> Any:
    """A staked miner opens a funded contest and the referee rules FRAUD against it. Returns
    (miner, its stake lock, the contest). The contest bond is burned by the real `settle_lock`."""
    miner = rig.join(loser_index)
    stake = rig.lock_stake(miner)
    contest = rig.open_contest(miner)
    response = rig.refereed(contest.dispute_id, "FRAUD", miner.hotkey)
    assert response.status_code == 200, response.text
    held = units_of(rig.escrow, miner.coldkey)
    assert held["dispute_locked"] == 0 and held["burned"] >= BOND  # control: the bond is gone
    return miner, stake, contest


def test_a_lost_fraud_contest_bans_only_the_loser_for_proven_training_fraud(rig: Rig) -> None:
    """T-CRD-20 (CRD-21): a staked ACTIVE bystander and a staked miner who loses a FRAUD contest.
    The loser is BANNED for proven training fraud; the bystander's units and eligibility are
    exactly what they were. (Before the admission-bond settlement of ADR 0010 the loser stayed
    SUSPENDED for ever: Admission.settle_fraud had no caller.)"""
    bystander = rig.admit(1)  # staked and ACTIVE
    kept = units_of(rig.escrow, bystander.coldkey)
    assert kept["admission_locked"] == STAKE and rig.status(bystander).eligible

    miner, _, _ = lose_a_contest(rig)

    record = rig.admission.store.record(miner.hotkey)
    assert (record.state, record.reason) == ("BANNED", "FINALIZED_TRAINING_FRAUD")
    assert units_of(rig.escrow, bystander.coldkey) == kept and rig.status(bystander).eligible
    assert rig.escrow.balances().conserved()


def test_the_stake_of_a_convicted_admission_is_burned_and_cannot_be_released_by_its_owner(
    rig: Rig,
) -> None:
    """T-CRD-20 (CRD-21, ES-04): after the loss the stake is no longer locked for a live
    admission (burned with the bond), and the owner's signed release with the terms of the
    closed FRAUD dispute is refused as disputed or unvested, moving nothing. (Burn rather than
    freeze is the choice of ADR 0010; a freeze would relax the equality on `burned`.)"""
    miner, stake, contest = lose_a_contest(rig)
    held = units_of(rig.escrow, miner.coldkey)
    assert (held["admission_locked"], held["burned"]) == (0, BOND + STAKE)
    terms = {
        "of the closed dispute": rig.disputes.settlement(contest.dispute_id),
        "of the lock": rig.admission.lock_settlement(stake.operation_id),
    }
    assert terms["of the lock"].outcome == "FRAUD"  # the settlement the lock now has

    for attempt, status in terms.items():
        refused = rig.release(
            miner, stake, status.finality_hash, status.closed_dispute_root, attempt=attempt
        )
        assert (refused.status_code, refused.json()["detail"]) == (409, "DISPUTED_OR_UNVESTED")
        assert units_of(rig.escrow, miner.coldkey) == held
    assert rig.escrow.balances().conserved()


# --- T-SHD-03: an ordinary trial creates no shadow, operator or runtime-custody record -------

WRITER_ONLY_KINDS = (
    "shadow-execution",
    "shadow-operator",
    "runtime-custody",
    "shadow-reservation",
    "shadow-custody",
)


def test_an_ordinary_trial_through_the_routes_creates_no_shadow_record_and_issues_nothing(
    network: Any,
) -> None:
    """T-SHD-03 (SHD-06, SHD-11, SHD-24, part 1): a miner that joins, stakes, is challenged, has the
    service run the reference, sends its proof and is finalized (the routes of an application, on
    a real CPU execution) leaves no record of the kinds only a trusted writer creates, no shadow
    reward, and the ledger issued exactly what its genesis did. The flow is the e2e test of that
    name, called as it is."""
    e2e = load_fixture_module(*E2E_FIXTURE)
    e2e.test_actual_shadow_trial_does_not_mutate_production(network)
    db = network.store._db
    kinds = {
        row[0]: row[1]
        for row in db.execute(
            "SELECT kind,COUNT(*) FROM records_v2 WHERE run_id=? GROUP BY kind",
            (network.manifest.run_id(),),
        )
    }
    assert db.execute("SELECT clean_count FROM admissions_v2").fetchone()[0] == 1  # control
    assert {kind: kinds.get(kind, 0) for kind in WRITER_ONLY_KINDS} == dict.fromkeys(
        WRITER_ONLY_KINDS, 0
    )
    escrow = network.store._services(network.manifest.run_id())[0]
    assert escrow.balances().issued == 40000
    assert db.execute("SELECT COUNT(*) FROM escrow_shadow_finalized").fetchone()[0] == 0
    assert event_count(escrow, "REWARD_FINALIZE") == 0


# --- M-02: the weights of a run whose round was repaired and so cannot be finalized -----------


def test_a_repaired_round_cannot_be_finalized_and_the_weights_stay_a_full_burn(
    world: sa.World,
) -> None:
    """M-02 (the blocker the weights wave does not play: a round the rollback repaired). The repair
    is model history only: the coordinator's Finalize of that round is refused 409 and mints
    nothing, so an epoch asked long after any maturity is the explicit full burn (no weight, no
    payment, the whole share burned), with the same bytes asked again."""
    with world.at("s4") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        escrow = store._services(run)[0]
        applied = store._record_v2(run, "applied", "0")
        request = sa.finalize_request(n, applied["theta_hash"], [k.ss58 for k in cast.honest])
        with store._tx():
            store._put_record_v2(run, "applied", "0", {**applied, "kind": "repair"})
        before = (escrow.balances(), sa.digest_of(store))

        with pytest.raises(ChallengeError, match="repair evidence is model-only") as refused:
            store.finalize_v2(run, 0, sa.canonical(request))

        assert refused.value.status == 409
        assert (escrow.balances(), sa.digest_of(store)) == before
        far = int(store.clock()) + 10_000
        first = store.weights(5, far, 1)
        answer = parse(first)
        meta = answer["metadata"]
        assert answer["weights"] == {} and meta["reason"] == "ok"
        assert (meta["units_paid"], meta["units_burned_this_epoch"]) == (0, FULL_SHARE_MASS)
        assert pay_events(escrow) == []
        assert escrow.balances().reward_pending == before[0].reward_pending
        assert store.weights(5, far + 500, 1) == first
        # Control: the round was finalizable before the repair mark, so the refusal is the mark's.
        with store._tx():
            store._put_record_v2(run, "applied", "0", applied)
        settled = n.client.post(
            n.url + "/admin/rounds/0/finalize", json=request, headers=sa.admin()
        )
        assert settled.status_code == 200, settled.text
