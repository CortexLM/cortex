"""Economics at the store and HTTP boundary (BND-03 .. BND-25): who may issue, who may influence,
what a service run may spend.

Each test states a defence of the hosted run: production influence needs the signed economic-bound
record, issuance routes need the operator bearer and the coordinator, bootstrap units never reach
the epoch weights, a production run gets services only with an owner-signed CUDA qualification,
and the service-capacity ledger charges once, within its budget, and never grants heavy work.
"""

from __future__ import annotations

import json
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import CorruptObjectError, ObjectNotFound
from hypertrain.ledger.escrow_v2 import genesis_allocation_hash
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope_v2 import seal, verify_envelope
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Receipt, f32hex
from hypertrain.protocol.messages_v2 import CommitV2, EconomicsPolicyV2

from .support import (
    SERVICE_FIXTURE,
    error_code,
    load_fixture_module,
    mature,
    memory_db,
    service_world,
    table_names,
)
from .support_bounds import (
    MATURE_AT,
    activate_identity,
    capacity_run,
    creation,
    genesis_request,
    hosting,
    pinned_bootstrap,
    production_run,
    repinned,
    roster_entry,
    round_opening,
    synthetic_round,
    worker,
)

# BND-03: production influence is fail-closed on the coordinator-signed economic-bound record.


def test_a_production_miner_is_not_eligible_until_the_coordinator_wrote_the_bound(
    tmp_path: Path,
) -> None:
    with production_run(tmp_path, G_max_units=1100) as run:
        run.bootstrap()
        run.admit_and_lock(1100)
        # Condition broken: the funded miner holds exactly its floor, yet no record exists.
        before = run.status()
        assert (before.funding.locked_units, before.funding.conservative_bound) == (1100, False)
        assert before.eligible is False
        # Condition met: the record is coordinator-signed, written once, and opens eligibility.
        record = run.store.economic_bound_v2(run.run_id)
        assert record["receipt"]["signer"] == run.eco.E.COORD.ss58 and verify_envelope(
            record["receipt"]
        )
        after = run.status()
        assert (after.funding.conservative_bound, after.eligible) == (True, True)
        assert run.store.economic_bound_v2(run.run_id) == record


def test_a_bound_that_fails_writes_no_record_and_nobody_becomes_eligible(tmp_path: Path) -> None:
    # The goal 1101 exceeds the capacity 1100 by one unit: a stake equal to the floor is not enough.
    with production_run(tmp_path, G_max_units=1101) as run:
        run.bootstrap()
        run.admit_and_lock(1101)
        with pytest.raises(ChallengeError) as refused:
            run.store.economic_bound_v2(run.run_id)
        assert (refused.value.status, refused.value.detail) == (
            409,
            "economic bound q*s*beta*(R+S)>=G fails",
        )
        rows = run.store._db.execute("SELECT COUNT(*) FROM records_v2 WHERE kind='economic-bound'")
        assert rows.fetchone()[0] == 0
        assert run.status().eligible is False


def test_a_test_mode_run_has_no_economic_bound_record_to_write(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        before = world.db_digest()
        with pytest.raises(ChallengeError) as refused:
            world.store.economic_bound_v2(world.run_id)
        assert (refused.value.status, refused.value.detail) == (
            409,
            "economic-bound record is production only",
        )
        assert world.db_digest() == before


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="BND-03+BND-D06: the economic-bound gate must reject a record bound to another policy "
    "or without a coordinator receipt, not accept any row of that kind [candidate] "
    "[fix-lot: none (no design yet)]",
)
def test_a_forged_economic_bound_row_does_not_open_production_influence(tmp_path: Path) -> None:
    with production_run(tmp_path, G_max_units=1100) as run:
        run.bootstrap()
        run.admit_and_lock(1100)
        assert run.status().eligible is False  # control: funded, but no bound yet
        forged = {
            "run_id": run.run_id,
            "economics_policy_hash": "ff" * 32,
            "bound": "q*s*beta*(R+S)>=G",
        }
        with run.store._tx():
            run.store._put_record_v2(run.run_id, "economic-bound", "run", forged)
        assert run.status().eligible is False


# BND-05: issuance, genesis, bound and run-creation routes need the operator bearer.


def admin_routes(run_id: str) -> list[tuple[str, str]]:
    base = f"/v2/runs/{run_id}/admin"
    return [
        ("POST", "/v2/admin/runs"),
        ("POST", f"{base}/bootstrap"),
        ("POST", f"{base}/genesis"),
        ("POST", f"{base}/economic-bound"),
        ("PUT", f"{base}/dataset"),
    ]


def test_issuance_bound_dataset_and_run_creation_routes_refuse_every_bearer_but_the_operators(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        before = world.db_digest()
        for method, path in admin_routes(world.run_id):
            for label, headers in {
                "no bearer": {},
                "wrong bearer": {"Authorization": "Bearer not-the-operator"},
                "empty bearer": {"Authorization": "Bearer "},
                "internal bearer": world.internal,
                "worker bearer": world.worker,
            }.items():
                refused = world.client.request(method, path, headers=headers, content=b"{}")
                assert (refused.status_code, refused.json()) == (401, {"detail": "unauthorized"}), (
                    f"{method} {path} {label}"
                )
            # Control: the operator bearer passes the bearer gate and is refused by the handler.
            reached = world.client.request(method, path, headers=world.admin, content=b"{}")
            assert reached.status_code in (409, 422), f"{method} {path}"
        assert world.db_digest() == before


def test_the_operator_bearer_is_read_on_every_request_and_no_usable_file_means_no_access(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        token = tmp_path / "admin"
        route = f"/v2/runs/{world.run_id}/admin/economic-bound"
        assert world.client.post(route, headers=world.admin).status_code == 409
        before = world.db_digest()
        for content in ("", None):  # an empty file, then no file: not configured, never open
            if content is None:
                token.unlink()
            else:
                token.write_text(content)
            for method, path in admin_routes(world.run_id):
                for headers in (world.admin, {"Authorization": "Bearer "}, {}):
                    refused = world.client.request(method, path, headers=headers, content=b"{}")
                    assert (refused.status_code, refused.json()) == (
                        503,
                        {"detail": "this route is not configured"},
                    ), f"{method} {path}"
        token.write_text("rotated-operator-value")
        assert world.client.post(route, headers=world.admin).status_code == 401
        rotated = {"Authorization": "Bearer rotated-operator-value"}
        assert world.client.post(route, headers=rotated).status_code == 409
        assert world.db_digest() == before


def test_a_genesis_envelope_not_signed_by_the_coordinator_is_refused_with_the_operator_bearer(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        service = load_fixture_module(*SERVICE_FIXTURE)
        run_id = world.run_id
        pin = EconomicsPolicyV2.model_validate_json(
            world.store.objects.get(world.network.manifest.network.economics_policy_hash)
        ).genesis_allocation_hash
        allocation = [(key.ss58, 10_000, 1) for key in service.COLD]
        genesis = genesis_request(service.fixture, run_id, allocation, pin=pin)
        before = world.db_digest()
        refused = world.client.post(
            f"/v2/runs/{run_id}/admin/genesis",
            json=seal(service.OWNER, "TestGenesis", run_id, genesis, 10_000),
            headers=world.admin,
        )
        assert (refused.status_code, refused.json()) == (
            409,
            {"detail": "signer is not authorized for message role"},
        )
        assert world.db_digest() == before


def test_a_production_bootstrap_whose_authority_signature_is_not_the_coordinators_issues_nothing(
    tmp_path: Path,
) -> None:
    with production_run(tmp_path) as run:
        E = run.eco.E
        run.eco.beacon(run.store, 1, bls=True)
        forged = run.bootstrap_body(signer=E.OTHER)  # the envelope below is the coordinator's
        raw = canonicalize(seal(E.COORD, "ProductionBootstrap", run.run_id, forged, 10_000))
        assert error_code(lambda: run.store.production_bootstrap_v2(run.run_id, raw)) == (
            "ORIGIN_AUTHORITY"
        )
        assert run.eco.issued(run.store) == 0
        nonces = run.store._db.execute("SELECT COUNT(*) FROM accepted_v2").fetchone()[0]
        assert nonces == 0  # the refused body did not reserve the nonce
        run.store.production_bootstrap_v2(run.run_id, run.signed_bootstrap())
        assert run.eco.issued(run.store) == 30_000


# BND-09 at the store: the signed body is keyed per run, before the ledger is reached.


def test_a_second_signed_bootstrap_is_refused_by_the_store_and_the_first_replays_unchanged(
    tmp_path: Path,
) -> None:
    with production_run(tmp_path) as run:
        run.eco.beacon(run.store, 1, bls=True)
        raw = run.signed_bootstrap()
        first = run.store.production_bootstrap_v2(run.run_id, raw)
        assert run.store.production_bootstrap_v2(run.run_id, raw) == first
        second = run.signed_bootstrap(operation="55" * 32)
        with pytest.raises(ChallengeError) as refused:
            run.store.production_bootstrap_v2(run.run_id, second)
        assert (refused.value.status, refused.value.detail) == (
            409,
            "conflicting signed operation nonce",
        )
        events = run.escrow.db.execute("SELECT kind FROM escrow_events").fetchall()
        assert [row["kind"] for row in events] == ["PRODUCTION_BOOTSTRAP"]
        assert run.eco.issued(run.store) == 30_000


# BND-04 at the store: an out-of-band edit of the policy object cannot become the run's policy.


def test_a_policy_object_edited_in_the_object_store_cannot_become_the_policy_of_the_run(
    tmp_path: Path,
) -> None:
    with production_run(tmp_path) as run:
        manifest = run.store._run_v2(run.run_id)
        pinned = manifest.network.economics_policy_hash
        path = run.store.objects._path(pinned)
        original = path.read_bytes()
        edited = json.loads(original)
        edited["G_max_units"] += 1
        path.write_bytes(canonicalize(edited))
        with pytest.raises(CorruptObjectError):
            run.store._services(run.run_id)
        assert run.store._services_v2 == {}
        assert "escrow_events" not in table_names(run.store._db)
        path.write_bytes(original)  # control: the pinned bytes build the services
        assert run.escrow.policy.G_max_units == edited["G_max_units"] - 1


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="BND-04+BND-D07: the dataset linkage of a run must be fixed once a round exists; the "
    "operator route replaces it at any time [candidate] "
    "[fix-lot: decision (product choice pending)]",
)
def test_the_dataset_linkage_cannot_be_replaced_once_a_round_exists(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        route = f"/v2/runs/{world.run_id}/admin/dataset"
        linkage = world.store._record_v2(world.run_id, "dataset", "inputs")
        with world.store._tx():
            world.store._put_record_v2(world.run_id, "round", "0", {"opened": "before the change"})
        # Control: the same linkage again is accepted, so only a different pair is at issue.
        assert world.client.put(route, json=linkage, headers=world.admin).status_code == 200
        other = {
            "samples_hash": world.store.objects.put(b"other samples"),
            "proofs_hash": world.store.objects.put(b"other proofs"),
        }
        changed = world.client.put(route, json=other, headers=world.admin)
        # Any refusal is a correct fix; what must hold is that the linkage did not move.
        assert 400 <= changed.status_code < 500
        assert world.store._record_v2(world.run_id, "dataset", "inputs") == linkage


# BND-07: audit probability is one for every roster entry of a live round.


def test_a_round_opening_with_a_roster_entry_below_certainty_or_ceiling_is_refused(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        service = load_fixture_module(*SERVICE_FIXTURE)
        identity = activate_identity(world)
        network = world.network

        def post(**changes: Any) -> tuple[int, str]:
            roster = [roster_entry(world, 0, identity, **changes)]
            response = network.client.post(
                network.url + "/admin/rounds",
                json=network.signed(service.COORD, "RoundOpenV2", round_opening(world, roster)),
                headers=world.admin,
            )
            return response.status_code, response.json()["detail"]

        before = world.db_digest()
        # Control: the honest entry passes both roster gates and stops at the start-state index.
        assert post() == (422, "start-state index differs from authenticated anchors")
        assert post(q_i=f32hex(0.5)) == (403, "roster lacks current funded admission authority")
        assert post(q_i=f32hex(0.25)) == (403, "roster lacks current funded admission authority")
        assert post(eligible_weight=2_097_152) == (422, "live eligible ceiling must be Q/4")
        assert post(eligible_weight=4_194_305) == (422, "live eligible ceiling must be Q/4")
        assert world.db_digest() == before


def test_every_roster_entry_of_a_round_gets_one_full_replay_audit_lease(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        service = load_fixture_module(*SERVICE_FIXTURE)
        network, store, run_id = world.network, world.store, world.run_id
        roster = [
            roster_entry(world, i, sha256_hex(str(i).encode()), q_i=f32hex(1)) for i in range(3)
        ]
        opening = round_opening(world, roster)
        network.push(opening.d_audit)
        with store._tx():
            store._put_record_v2(
                run_id, "round", "0", network.signed(service.COORD, "RoundOpenV2", opening)
            )
            for i in range(3):
                commit = CommitV2(
                    w=0,
                    hotkey=service.HOT[i].ss58,
                    leaf_scheme="ht-leaf-v1",
                    n_leaves=2,
                    leaves_root="11" * 32,
                    metrics_root="11" * 32,
                    final_theta_hash="11" * 32,
                    ef_in_hash="11" * 32,
                    ef_out_hash="11" * 32,
                    delta_hash="11" * 32,
                    delta_bytes=1,
                    tokens=4,
                )
                identity = f"0:{service.HOT[i].ss58}"
                signed = network.signed(service.HOT[i], "CommitV2", commit)
                store._put_record_v2(run_id, "commit", identity, signed)
                store._put_record_v2(run_id, "start", identity, {"anchor": i})
        jobs = store.schedule_audits_v2(run_id, 0)
        assert len(jobs) == len(set(jobs)) == 3
        leases = store._db.execute(
            "SELECT hotkey, state, challenge FROM audit_leases_v2"
        ).fetchall()
        assert sorted(row["hotkey"] for row in leases) == sorted(k.ss58 for k in service.HOT[:3])
        for lease in leases:
            body = json.loads(lease["challenge"])["body"]
            assert (lease["state"], body["mode"], body["audit_mode"]) == (
                "queued",
                "full",
                "anchored-full",
            )
            assert body["reasons"] == ["random", "final"] and body["target"] == lease["hotkey"]
        assert store.schedule_audits_v2(run_id, 0) == []  # nothing new, nothing skipped


# BND-12 and BND-15: what the epoch weights pay.


def test_bootstrap_units_never_reach_the_epoch_weights_only_the_matured_live_reward_does(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    hot, cold = worker(0)
    allocation = [(cold.ss58, 20_000, 1), (worker(1)[1].ss58, 10_000, 1)]
    setup = repinned(
        li_escrow.setup("production"), bootstrap_allocation_hash=genesis_allocation_hash(allocation)
    )
    live = synthetic_round(li_escrow, setup, 0, [(hot, cold)])
    with hosting(tmp_path, li_escrow, setup, [live]) as host:
        bootstrap = pinned_bootstrap(li_escrow, host.run_id, allocation)
        host.escrow.production_bootstrap(bootstrap)
        host.escrow.mature("66" * 32, tuple(o.origin_id for o in bootstrap.origins), 1)
        host.escrow.reward_finalize(live.reward, live.evidence)
        mature(host.escrow, live.reward)
        # The bootstrap units are free and matured, the live reward too; only one of them pays.
        assert host.escrow.balances().available == 30_000 + 1_000_000
        answer = host.weights(0)
        assert answer["weights"] == {hot.ss58: 1_000_000.0}
        assert answer["metadata"]["units_paid"] == 1_000_000
        assert host.escrow.balances().available == 30_000
        assert host.weights(0) == answer


def test_test_genesis_units_are_collateral_and_never_payable(li_escrow: ModuleType) -> None:
    escrow = li_escrow.ledger(li_escrow.setup("test"), memory_db())
    li_escrow.fund(escrow)
    assert escrow.balances().available == 30_000
    assert escrow.payable(10**9) == []


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="BND-15+BND-D02: a test-ledger run must never emit real weight through get_weights; its "
    "matured live reward is paid like a production one today (documented in network-v2.md) "
    "[candidate] [fix-lot: decision (product choice pending)]",
)
def test_a_test_ledger_reward_never_reaches_the_epoch_weights(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    hot, cold = worker(0)
    setup = li_escrow.setup("test")
    live = synthetic_round(li_escrow, setup, 0, [(hot, cold)])
    with hosting(tmp_path, li_escrow, setup, [live]) as host:
        host.escrow.reward_finalize(live.reward, live.evidence)
        mature(host.escrow, live.reward)
        # Control: the reward is matured, free and payable; the bridge decides whether it pays.
        assert [r["origin"] for r in host.escrow.payable(MATURE_AT)] == live.reward.origin_ids
        answer = host.weights(0)
        assert answer["weights"] == {} and answer["metadata"]["units_paid"] == 0
        assert host.escrow.balances().paid == 0


# BND-17 and BND-25: what run creation refuses, and what it leaves behind.


def production_policy(world: Any) -> EconomicsPolicyV2:
    pinned = EconomicsPolicyV2.model_validate_json(
        world.store.objects.get(world.network.manifest.network.economics_policy_hash)
    )
    return EconomicsPolicyV2.model_validate(
        {**pinned.body(), "ledger_mode": "production", "genesis_allocation_hash": None}
    )


def test_a_manifest_whose_economics_policy_object_is_missing_creates_no_run(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path / "world") as world:
        with creation(world, tmp_path / "zero", None, pinned="00" * 32) as zero:
            with pytest.raises(ObjectNotFound):
                zero.store.create_run_v2(zero.raw)
            assert zero.run_rows() == 0
        # Control: the same manifest with a stored policy creates the run.
        stored = EconomicsPolicyV2.model_validate_json(
            world.store.objects.get(world.network.manifest.network.economics_policy_hash)
        )
        with creation(world, tmp_path / "stored", stored) as ok:
            assert ok.store.create_run_v2(ok.raw)["status"] == "created"
            assert ok.run_rows() == 1


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="BND-25+BND-D05: a refused run creation must leave no run row, so the same envelope can "
    "be posted again once the cause is fixed; the row is committed before the services are built "
    "[fix-lot: none (no design yet)]",
)
def test_a_production_run_without_a_cuda_qualification_is_refused_and_leaves_no_run_row(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path / "world") as world:
        with creation(world, tmp_path / "prod", production_policy(world)) as prod:
            with pytest.raises(ChallengeError) as refused:
                prod.store.create_run_v2(prod.raw)
            assert (refused.value.status, refused.value.detail) == (
                409,
                "production CUDA qualification required",
            )
            assert prod.store._services_v2 == {}
            assert prod.run_rows() == 0
            # A second POST of the same envelope must be refused for the same reason, not as a
            # duplicate of a run that was never created.
            with pytest.raises(ChallengeError) as again:
                prod.store.create_run_v2(prod.raw)
            assert again.value.detail == "production CUDA qualification required"


# BND-18: production services exist only behind an owner-signed CUDA qualification.


def install_qualification(run: Any, signer: Any, **evidence_changes: Any) -> dict[str, Any]:
    """Write the record `_bootstrap_backend_v2` would write for GPU evidence this test does not
    have: the exact-evidence object and a Receipt over its hash signed by `signer`. The hosts are
    attested, as a production run demands; the lists are empty because no host signed anything."""
    store = run.store
    manifest = store._run_v2(run.run_id)
    reference = manifest.training.reference_spec
    evidence = {
        "run_id": run.run_id,
        "backend": "cuda",
        "reference_hash": sha256_hex(canonicalize(reference.model_dump(mode="json"))),
        "layout_hash": reference.layout.model_dump_json(),
        "host_attestation": {"host_keys": [], "statements": [], "signatures": []},
        **evidence_changes,
    }
    authority = store.objects.put(canonicalize(evidence))
    receipt = Receipt(w=0, commit_hash=authority, received_round=1)
    record = {
        **evidence,
        "authority_hash": authority,
        "receipt": seal(signer, "Receipt", run.run_id, receipt, 10_000),
    }
    with store._tx():
        store._put_record_v2(run.run_id, "qualification", reference.image_digest, record)
    return record


def test_production_services_exist_only_behind_an_owner_signed_cuda_qualification(
    tmp_path: Path,
) -> None:
    with production_run(tmp_path, cuda=False, gpu="sm_103") as run:
        service = load_fixture_module(*SERVICE_FIXTURE)
        owner = service.OWNER
        with pytest.raises(ChallengeError, match="production CUDA qualification required"):
            run.store._services(run.run_id)
        # The owner is the store's configured owner hotkey; a qualification signed by anyone else
        # or describing a CPU backend is refused, and none of them builds a service.
        run.store.owner_hotkey = owner.ss58
        install_qualification(run, run.eco.E.COORD)
        with pytest.raises(ChallengeError, match="backend qualification authority differs"):
            run.store._services(run.run_id)
        install_qualification(run, owner, backend="cpu")
        with pytest.raises(ChallengeError, match="backend qualification authority differs"):
            run.store._services(run.run_id)
        assert run.store._services_v2 == {}
        assert "escrow_events" not in table_names(run.store._db)
        # Condition met: the owner-signed CUDA record builds the services and freezes the backend.
        install_qualification(run, owner)
        assert run.store._services(run.run_id)[1].backend == "cuda"
        frozen = run.store._record_v2(run.run_id, "execution-backend", "run")
        assert frozen["backend"] == "cuda"


def test_the_execution_backend_of_a_run_is_frozen_once_chosen(tmp_path: Path) -> None:
    with production_run(tmp_path, cuda=False, gpu="sm_103") as run:
        service = load_fixture_module(*SERVICE_FIXTURE)
        run.store.owner_hotkey = service.OWNER.ss58
        first = install_qualification(run, service.OWNER)
        run.store._services(run.run_id)
        # Another owner-signed CUDA record (other evidence) cannot replace the frozen authority.
        install_qualification(run, service.OWNER, notes="a second qualification")
        run.store._services_v2.clear()
        with pytest.raises(ChallengeError, match="frozen execution backend cannot change"):
            run.store._services(run.run_id)
        assert (
            run.store._record_v2(run.run_id, "execution-backend", "run")["authority_hash"]
            == (first["authority_hash"])
        )


# BND-21, BND-22 and BND-23: the service-capacity ledger of a 32-worker run.


def charge_records(store: Any) -> int:
    return int(
        store._db.execute("SELECT COUNT(*) FROM records_v2 WHERE kind='service-charge'").fetchone()[
            0
        ]
    )


CHARGE_FAULTS = {
    "zero units": ("job", 1, 0),
    "negative units": ("job", 1, -1),
    "boolean units": ("job", 1, True),
    "float units": ("job", 1, 1.0),
    "attempt zero": ("job", 0, 1),
    "attempt three": ("job", 3, 1),
    "boolean attempt": ("job", True, 1),
    "empty operation": ("", 1, 1),
    "operation of 257 bytes": ("x" * 257, 1, 1),
    "operation of 258 bytes in 129 characters": ("é" * 129, 1, 1),
}


def test_a_service_charge_outside_its_bounds_is_refused_and_records_nothing(tmp_path: Path) -> None:
    with capacity_run(tmp_path, reserve=True) as (_, store, manifest):
        run_id = manifest.run_id()
        for label, (operation, attempt, units) in CHARGE_FAULTS.items():
            with pytest.raises(ChallengeError) as refused:
                store._service_charge_v2(run_id, 0, operation, attempt, kind="audit", units=units)
            assert (refused.value.status, refused.value.detail) == (
                422,
                "SERVICE_CHARGE_BOUNDS",
            ), label
        assert charge_records(store) == 0
        # Control: the boundary values themselves are charged.
        assert store._service_charge_v2(run_id, 0, "x" * 256, 2, kind="audit", units=1)
        assert charge_records(store) == 1


def test_the_service_budget_is_run_wide_per_kind_a_repeat_is_free_and_equality_is_allowed(
    tmp_path: Path,
) -> None:
    with capacity_run(tmp_path, reserve=True) as (_, store, manifest):
        run_id = manifest.run_id()
        with store._tx():  # a second round with its own roster and reservation
            for kind in ("round", "round-resource"):
                store._put_record_v2(run_id, kind, "1", store._record_v2(run_id, kind, "0"))
        assert store._service_charge_v2(run_id, 0, "a", 1, kind="audit", units=300)
        # The same charge again grants no second execution and costs nothing more.
        assert not store._service_charge_v2(run_id, 0, "a", 1, kind="audit", units=300)
        with pytest.raises(ChallengeError, match="SERVICE_CHARGE_CONFLICT"):
            store._service_charge_v2(run_id, 0, "a", 1, kind="audit", units=301)
        # Another round draws on the same run-wide limit of 512 audit units, up to equality.
        assert store._service_charge_v2(run_id, 1, "b", 1, kind="audit", units=212)
        with pytest.raises(ChallengeError, match="SERVICE_WORK_BUDGET_EXHAUSTED"):
            store._service_charge_v2(run_id, 1, "c", 1, kind="audit", units=1)
        # Another kind has its own limit; nothing is ever refunded.
        assert store._service_charge_v2(run_id, 0, "d", 1, kind="repair", units=1024)
        assert charge_records(store) == 3


def test_a_service_charge_after_the_reservation_deadline_is_refused_and_at_it_is_allowed(
    tmp_path: Path,
) -> None:
    with capacity_run(tmp_path, reserve=True) as (_, store, manifest):
        run_id = manifest.run_id()
        now = store._now(store._db)
        reservation = store._record_v2(run_id, "round-resource", "0")

        def reserve_until(deadline: int) -> None:
            with store._tx():
                store._put_record_v2(
                    run_id, "round-resource", "0", {**reservation, "deadline": deadline}
                )

        reserve_until(now - 1)
        with pytest.raises(ChallengeError) as late:
            store._service_charge_v2(run_id, 0, "late", 1, kind="audit", units=1)
        assert (late.value.status, late.value.detail) == (409, "SERVICE_RESOURCE_DEADLINE")
        assert charge_records(store) == 0
        reserve_until(now)
        assert store._service_charge_v2(run_id, 0, "on time", 1, kind="audit", units=1)


def test_a_service_profile_is_admitted_before_the_first_round_and_at_most_once(
    tmp_path: Path,
) -> None:
    with capacity_run(tmp_path, reserve=False) as (capacity, store, manifest):
        run_id = manifest.run_id()
        service = capacity.service
        body, raw = capacity.signed_profile(store, manifest)

        def admissions() -> int:
            sql = "SELECT COUNT(*) FROM records_v2 WHERE kind='service-admission'"
            return int(store._db.execute(sql).fetchone()[0])

        # A receipt that is not at round 0, not at the current round, or not over this profile.
        for fault in (
            Receipt(w=1, commit_hash=sha256_hex(body), received_round=1),
            Receipt(w=0, commit_hash=sha256_hex(body), received_round=2),
            Receipt(w=0, commit_hash="22" * 32, received_round=1),
        ):
            bad = canonicalize(envelope_v2.seal(service.OWNER, "Receipt", run_id, fault, 100))
            with pytest.raises(ChallengeError) as refused:
                store.bootstrap_service_capacity_v2(run_id, body, bad)
            assert (refused.value.status, refused.value.detail) == (409, "SERVICE_PROFILE_RECEIPT")
            assert admissions() == 0
        # Condition met: admitted once; the same receipt replays; another valid receipt conflicts.
        assert store.bootstrap_service_capacity_v2(run_id, body, raw) == sha256_hex(body)
        assert store.bootstrap_service_capacity_v2(run_id, body, raw) == sha256_hex(body)
        stored = store._record_v2(run_id, "service-admission", "run")
        later = Receipt(w=0, commit_hash=sha256_hex(body), received_round=1)
        second = canonicalize(envelope_v2.seal(service.OWNER, "Receipt", run_id, later, 101))
        with pytest.raises(ChallengeError, match="SERVICE_PROFILE_IMMUTABLE"):
            store.bootstrap_service_capacity_v2(run_id, body, second)
        assert store._record_v2(run_id, "service-admission", "run") == stored


def test_a_service_profile_is_refused_once_a_round_exists(tmp_path: Path) -> None:
    with capacity_run(tmp_path, reserve=False) as (capacity, store, manifest):
        run_id = manifest.run_id()
        body, raw = capacity.signed_profile(store, manifest)
        with store._tx():
            store._put_record_v2(run_id, "round", "0", {"opened": "before the profile"})
        with pytest.raises(ChallengeError) as refused:
            store.bootstrap_service_capacity_v2(run_id, body, raw)
        assert (refused.value.status, refused.value.detail) == (409, "SERVICE_PROFILE_BEFORE_OPEN")
        sql = "SELECT COUNT(*) FROM records_v2 WHERE kind='service-admission'"
        assert store._db.execute(sql).fetchone()[0] == 0


def test_a_changed_capacity_implementation_voids_the_admitted_profile_and_cannot_replace_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    with capacity_run(tmp_path, reserve=False) as (capacity, store, manifest):
        run_id = manifest.run_id()
        body, raw = capacity.signed_profile(store, manifest)
        store.bootstrap_service_capacity_v2(run_id, body, raw)
        roster = capacity.roster(32)
        assert store.require_roster_v2(run_id, roster=roster)["count"] == 32  # condition met
        admitted = store._record_v2(run_id, "service-admission", "run")
        # Condition broken: the code the profile was bound to is no longer the running code.
        monkeypatch.setattr(
            ChallengeStore,
            "_service_implementation_v2",
            staticmethod(lambda source, paths, pin: "ef" * 32),
        )
        with pytest.raises(ChallengeError, match="SERVICE_PROFILE_AUTHORITY"):
            store.require_roster_v2(run_id, roster=roster)
        new_body, new_raw = capacity.signed_profile(store, manifest)
        assert new_body != body
        with pytest.raises(ChallengeError, match="SERVICE_PROFILE_IMMUTABLE"):
            store.bootstrap_service_capacity_v2(run_id, new_body, new_raw)
        assert store._record_v2(run_id, "service-admission", "run") == admitted
