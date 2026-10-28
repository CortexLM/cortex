"""A round the arithmetic or the finality rules refuse is a 409 with a code, never a 500.

A fault of the master's own artifacts is the opposite: a 500 that names itself."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)

from __future__ import annotations

import pytest
from fastapi.testclient import TestClient

from hypertrain.aggregator.core import FinalityError, TapeError, TapeIntegrityError
from hypertrain.aggregator.weighted_v2 import InsufficientEligibleWeight
from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import put_large
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2, StartStateV2
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

from . import test_service_network_v2 as fixtures
from .five_identity_network_v2 import five_identity_network
from .test_service_network_v2 import network  # noqa: F401  (pytest fixture import)


@pytest.fixture
def activated_network(tmp_path_factory):
    """Five ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures)


def refusing(net) -> TestClient:
    """The app of `net` as a client that answers an unhandled exception with its real 500."""
    return TestClient(net.client.app, raise_server_exceptions=False)


def answer(response) -> tuple[int, object]:
    """Status and body, whatever the body is: an unhandled exception answers plain text."""
    is_json = response.headers.get("content-type", "").startswith("application/json")
    return response.status_code, response.json() if is_json else response.text


def round_zero(net, hotkeys) -> RoundOpenV2:
    """The signed-ready opening of round 0 for `hotkeys`, with the anchors the store derives."""
    manifest = net.manifest
    admission = net.store._services(manifest.run_id())[1]
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    cache = AnchorCache()
    starts, roster = [], []
    for slot, key in enumerate(hotkeys):
        record = admission.status(key.ss58, now=net.now).record
        anchor = cache.genesis(manifest, key.ss58, theta)
        starts.append(
            StartStateV2(
                run_id=manifest.run_id(),
                w=0,
                hotkey=key.ss58,
                theta_hash=state_hash(theta),
                state_object_sha256=put_large(pack_state(theta, anchor.state), sha256_hex),
                opt_state_hash=optimizer_hash(anchor.state),
                ef_object_sha256=put_large(pack_state(anchor.ef), sha256_hex),
                ef_hash=state_hash(anchor.ef),
                parent_anchor_hash=anchor.anchor_hash,
                global_step0=anchor.state.step,
                anchor_verdict_hash=anchor.proof_hash,
            )
        )
        roster.append(
            {
                "hotkey": key.ss58,
                "slot": slot,
                "q_i": f32hex(1),
                "admission_id": record.admission_id,
                "coldkey_group": record.coldkey,
                "state": "ACTIVE",
                "eligible_weight": 4194304,
            }
        )
    return RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=net.now + 1,
        d_assign=net.now + 2,
        d_commit=net.now + 30,
        d_audit=net.now + 31,
        d_upload=net.now + 150,
        d_final=net.now + 200,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {name: getattr(manifest.network, name) for name in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
        audit_mode="anchored-full",
        roster=roster,
    )


def open_round(net, opening: RoundOpenV2, client: TestClient):
    return client.post(
        net.url + "/admin/rounds",
        json=net.signed(fixtures.COORD, "RoundOpenV2", opening),
        headers=fixtures.admin(),
    )


def test_opening_a_round_the_caps_cannot_carry_is_a_409_not_a_500(activated_network):
    n = activated_network
    opening = round_zero(n, fixtures.HOT[:3])

    refused = open_round(n, opening, refusing(n))

    assert answer(refused) == (
        409,
        {"detail": "ROUND_INFEASIBLE: INSUFFICIENT_ELIGIBLE_WEIGHT"},
    )
    assert (
        n.store._db.execute("SELECT COUNT(*) FROM records_v2 WHERE kind='round'").fetchone()[0] == 0
    )


def test_opening_a_round_after_a_skipped_one_is_a_409_not_a_500(activated_network):
    n = activated_network
    client = refusing(n)
    first = round_zero(n, fixtures.HOT)
    assert open_round(n, first, client).status_code == 200

    refused = open_round(n, first.model_copy(update={"w": 2}), client)

    assert answer(refused) == (
        409,
        {"detail": "ROUND_BLOCKED: opening requires complete sequential history"},
    )


# Each site of the store that can raise a refusal, over its real route. The store method is
# replaced by a raiser: the contract under test is the answer of the app. The two rollback routes
# already mapped a TapeError to a bare 409 before the handlers existed, so it is not a case here.
EVERY = (InsufficientEligibleWeight, FinalityError, TapeError)
NOT_TAPE = (InsufficientEligibleWeight, FinalityError)
ROUTES = {
    "aggregate": ("/admin/rounds/0/aggregate", "aggregate_v2", EVERY),
    "finalize": ("/admin/rounds/0/finalize", "finalize_v2", EVERY),
    "rollback": ("/admin/rollback", "rollback_v2", NOT_TAPE),
    "rollback-preview": ("/admin/rollback-preview", "rollback_preview_v2", NOT_TAPE),
}
CASES = [(site, error) for site, (_, _, errors) in ROUTES.items() for error in errors]
CODES = {
    InsufficientEligibleWeight: "ROUND_INFEASIBLE",
    FinalityError: "ROUND_BLOCKED",
    TapeError: "ROUND_BLOCKED",
}


@pytest.mark.parametrize(
    ("site", "error"), CASES, ids=[f"{site}-{error.__name__}" for site, error in CASES]
)
def test_a_refusal_raised_by_a_store_method_is_a_409_with_its_code(
    network, monkeypatch, site, error
):
    path, method, _ = ROUTES[site]

    def raiser(*_args, **_kwargs):
        raise error("the reason")

    monkeypatch.setattr(ChallengeStore, method, raiser)

    refused = refusing(network).post(network.url + path, json={}, headers=fixtures.admin())

    assert answer(refused) == (409, {"detail": f"{CODES[error]}: the reason"})


@pytest.mark.parametrize("site", list(ROUTES))
def test_a_fault_of_the_masters_own_tape_is_a_500_that_names_itself(network, monkeypatch, site):
    path, method, _ = ROUTES[site]

    def raiser(*_args, **_kwargs):
        raise TapeIntegrityError("the reason")

    monkeypatch.setattr(ChallengeStore, method, raiser)

    refused = refusing(network).post(network.url + path, json={}, headers=fixtures.admin())

    assert answer(refused) == (500, {"detail": "TAPE_INTEGRITY: the reason"})
