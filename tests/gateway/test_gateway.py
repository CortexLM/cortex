import asyncio
import sqlite3
from dataclasses import replace
from datetime import UTC, datetime
from hashlib import sha256
from types import SimpleNamespace
from uuid import UUID

import httpx
import pytest
from fastapi import FastAPI

from cortex.errors import ServiceError
from cortex.gateway import GatewayService, GatewayStore, create_router, leaf_request
from cortex.gateway import store as gateway_store_module
from cortex.http import OperatorAuth
from cortex.protocol import (
    Bundle,
    ChallengeEntry,
    MetagraphRow,
    NoScore,
    NoScoreReason,
    Score,
    TrustRoot,
    sign_leaf,
)
from cortex.protocol.crypto import encode_hotkey, public_key
from cortex.validator import ChainSnapshot, SubmissionJournal, Validator


async def test_trust_reload_rejects_rollback_even_after_restart(network):
    updated = replace(network.trust, challenges_version=2, measurements_version=3)
    network.service.trust_loader = lambda epoch: updated
    network.service.refresh_trust(12)
    assert network.service.trust == updated
    restarted = GatewayStore(network.db_path)
    try:
        with pytest.raises(ServiceError, match="rollback"):
            GatewayService(store=restarted, **network.settings)
    finally:
        restarted.close()


class FakeChain:
    def __init__(self, rows):
        self.rows = rows
        self.submissions = []
        self.tip_reads = 0
        self.barrier = None
        self.epoch = 12

    async def current_block(self):
        self.tip_reads += 1
        return 99

    async def current_epoch(self, netuid):
        assert netuid == 541
        return self.epoch

    async def snapshot(self, block, netuid):
        assert netuid == 541
        if self.barrier is not None:
            await self.barrier.wait()
        return ChainSnapshot(block, bytes([9]) * 32, self.rows, self.rows[0].hotkey, frozenset())

    async def submit(self, netuid, vector, version_key):
        self.submissions.append((netuid, vector, version_key))
        return True


@pytest.fixture
async def network(tmp_path):
    token_file = tmp_path / "operator-token"
    token_file.write_text("first-test-token")
    token_file.chmod(0o600)
    store = GatewayStore(tmp_path / "gateway.db")
    rows = tuple(MetagraphRow(bytes([tag]) * 32, tag) for tag in range(3))
    trust = TrustRoot(
        (
            ChallengeEntry(b"bounty", public_key(bytes([1]) * 32), 2000),
            ChallengeEntry(b"proof", public_key(bytes([2]) * 32), 8000),
        ),
        sha256(b"\x00").digest(),
        public_key(bytes([7]) * 32),
    )
    chain = FakeChain(rows)
    settings = dict(
        trust=trust,
        netuid=541,
        chain=chain,
        gateway_seed=lambda: bytes([7]) * 32,
        clock=lambda: datetime(2026, 9, 16, 12, tzinfo=UTC),
    )
    service = GatewayService(store=store, **settings)
    app = FastAPI()
    app.include_router(create_router(service, OperatorAuth(token_file)))
    journal = SubmissionJournal(tmp_path / "validator.db")
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app=app), base_url="https://master.invalid"
    ) as client:
        yield SimpleNamespace(
            store=store,
            service=service,
            chain=chain,
            rows=rows,
            trust=trust,
            client=client,
            settings=settings,
            token_file=token_file,
            db_path=tmp_path / "gateway.db",
            journal=journal,
            auth={"authorization": "Bearer first-test-token"},
        )
    store.close()
    journal.close()


def leaves(network, epoch=12, winner=1, reason=NoScoreReason.NOT_ATTEMPTED):
    return [
        sign_leaf(
            bytes([seed]) * 32,
            challenge,
            row.hotkey,
            epoch,
            Score(50) if row.uid == winner else NoScore(reason),
        )
        for challenge, seed in ((b"bounty", 1), (b"proof", 2))
        for row in network.rows
    ]


async def intake(network, epoch=12, winner=1, reason=NoScoreReason.NOT_ATTEMPTED):
    for leaf in leaves(network, epoch, winner, reason):
        response = await network.client.post("/v1/weights/raw", json=leaf_request(leaf))
        assert response.status_code == 202, response.text


async def seal(network, epoch=12, **extra):
    return await network.client.post(
        "/v1/admin/seal", headers=network.auth, json={"epoch": epoch, **extra}
    )


async def test_signed_intake_seal_latest_validator_and_chain_dispatch(network):
    await intake(network)
    response = await seal(network)
    assert response.status_code == 200, response.text
    assert network.chain.tip_reads == 1
    latest = (await network.client.get("/v1/weights/latest")).json()
    assert latest["sealed"] is True
    assert latest["emission_shares"] == {"bounty": 0.2, "proof": 0.8}
    assert latest["uids"] == [1] and latest["weights"] == [1.0]
    assert [source["weights"] for source in latest["source_challenges"]] == [
        {encode_hotkey(network.rows[1].hotkey): 50.0},
        {encode_hotkey(network.rows[1].hotkey): 50.0},
    ]
    assert latest["source_outcomes"][0]["outcome"] == "accepted"
    validator = Validator(
        gateway_url="https://master.invalid",
        netuid=541,
        trust=network.trust,
        chain=network.chain,
        journal=network.journal,
        http=network.client,
    )
    assert (await validator.run_once()).outcome == "submitted"
    assert network.chain.submissions == [(541, ((1, 65535),), 1)]


async def test_unsealed_burn_is_available_without_owner_wallet_or_chain(network):
    response = await network.client.get("/v1/weights/latest")
    assert response.status_code == 200
    body = response.json()
    assert body["sealed"] is False and body["epoch"] is None
    assert body["uids"] == [0] and body["weights"] == [1.0]
    assert body["merkle_root"] == "" and body["vector_digest"] is None
    assert body["metagraph_updated_at"] is None
    assert network.chain.tip_reads == 0


async def test_signed_noscore_epoch_seals_and_burns(network):
    await intake(network, winner=None)
    assert (await seal(network)).status_code == 200
    latest = (await network.client.get("/v1/weights/latest")).json()
    assert latest["sealed"] is True and latest["burn_outcome"] is True
    assert latest["final_vector"] == [[0, 65535]]


async def test_signed_challenge_internal_snapshot_is_reported_as_accepted(network):
    await intake(network, winner=None, reason=NoScoreReason.CHALLENGE_INTERNAL)
    assert (await seal(network)).status_code == 200

    latest = (await network.client.get("/v1/weights/latest")).json()

    assert [source["ok"] for source in latest["source_challenges"]] == [True, True]
    assert [source["error"] for source in latest["source_challenges"]] == [None, None]
    assert [outcome["outcome"] for outcome in latest["source_outcomes"]] == [
        "accepted",
        "accepted",
    ]


async def test_sealed_burn_exposes_the_public_weights_contract(network):
    network.service.chain_endpoint = "wss://entrypoint-finney.opentensor.ai:443"
    await intake(network, winner=None)
    assert (await seal(network)).status_code == 200
    network.service.clock = lambda: datetime(2026, 9, 16, 13, tzinfo=UTC)

    latest = (await network.client.get("/v1/weights/latest")).json()
    bundle = Bundle.decode(network.service.bundle_bytes(12))
    required = {
        "protocol_version",
        "algorithm_version",
        "vector_id",
        "vector_digest",
        "epoch",
        "revision",
        "netuid",
        "chain_endpoint",
        "uids",
        "weights",
        "hotkey_weights",
        "chain_domain_bytes",
        "computed_at",
        "expires_at",
        "source_challenges",
        "source_snapshots",
        "source_outcomes",
        "emission_policy_version",
        "emission_shares",
        "burn_policy_version",
        "mapping_policy_version",
        "metagraph_identity",
        "metagraph_hash",
        "metagraph_block",
        "burn_outcome",
        "metagraph_updated_at",
        "merkle_root",
        "final_vector",
        "sealed",
    }

    assert set(latest) == required
    assert latest["protocol_version"] == "1.0"
    assert latest["algorithm_version"] == 1
    assert UUID(latest["vector_id"]).version == 5
    assert latest["vector_digest"] == sha256(bundle.body.encode()).hexdigest()
    assert latest["epoch"] == 12 and latest["revision"] == 1
    assert latest["netuid"] == 541
    assert latest["chain_endpoint"] == "wss://entrypoint-finney.opentensor.ai:443"
    assert latest["uids"] == [0] and latest["weights"] == [1.0]
    assert latest["hotkey_weights"] == {}
    assert latest["chain_domain_bytes"] == '{"netuid":541,"uids":[0],"weights":[1.0]}'
    assert latest["computed_at"] == "2026-09-16T12:00:00.000000Z"
    assert latest["expires_at"] == "2026-09-16T12:12:00.000000Z"
    assert latest["source_challenges"] == [
        {"slug": "bounty", "emission_percent": 20.0, "weights": {}, "ok": True, "error": None},
        {"slug": "proof", "emission_percent": 80.0, "weights": {}, "ok": True, "error": None},
    ]
    assert [snapshot["challenge_slug"] for snapshot in latest["source_snapshots"]] == [
        "bounty",
        "proof",
    ]
    assert all(snapshot["outcome"] == "accepted" for snapshot in latest["source_snapshots"])
    assert all(len(snapshot["payload_digest"]) == 64 for snapshot in latest["source_snapshots"])
    assert all(
        UUID(snapshot["snapshot_id"]).version == 5 for snapshot in latest["source_snapshots"]
    )
    snapshots = {snapshot["challenge_slug"]: snapshot for snapshot in latest["source_snapshots"]}
    assert latest["source_outcomes"] == [
        {
            "challenge_slug": slug,
            "outcome": "accepted",
            "reason_code": "accepted",
            "snapshot_id": snapshots[slug]["snapshot_id"],
            "payload_digest": snapshots[slug]["payload_digest"],
            "revision": None,
        }
        for slug in ("bounty", "proof")
    ]
    assert latest["emission_policy_version"] == "emission-shares.absolute.v1"
    assert latest["emission_shares"] == {"bounty": 0.2, "proof": 0.8}
    assert latest["burn_policy_version"] == "burn-uid0.v1"
    assert latest["mapping_policy_version"] == "hotkey-to-uid.v1"
    assert latest["metagraph_identity"] == {
        "hash": bundle.body.metagraph_root.hex(),
        "block": 99,
        "uid_count": 3,
        "burn_uid": 0,
    }
    assert latest["metagraph_hash"] == bundle.body.metagraph_root.hex()
    assert latest["metagraph_block"] == 99
    assert latest["burn_outcome"] is True
    assert latest["metagraph_updated_at"] == "2026-09-16T12:00:00.000000Z"
    assert latest["merkle_root"] == bundle.body.merkle_root.hex()
    assert latest["final_vector"] == [[0, 65535]]
    assert latest["sealed"] is True


async def test_seal_requires_exact_participant_coverage(network):
    for leaf in leaves(network)[:-1]:
        assert (
            await network.client.post("/v1/weights/raw", json=leaf_request(leaf))
        ).status_code == 202
    response = await seal(network)
    assert response.status_code == 409
    assert response.json()["code"] == "incomplete_participant_set"
    assert network.store.bundle(12) is None
    assert network.service.latest()["sealed"] is False


async def test_validly_signed_extra_participant_cannot_enter_the_seal(network):
    await intake(network)
    extra = sign_leaf(bytes([1]) * 32, b"bounty", bytes([99]) * 32, 12, Score(1))
    assert (
        await network.client.post("/v1/weights/raw", json=leaf_request(extra))
    ).status_code == 202
    assert (await seal(network)).status_code == 409


@pytest.mark.parametrize(
    "mutation,status",
    [
        ("signature", 401),
        ("retired", 404),
        ("bool", 400),
        ("unknown_field", 400),
        ("reason", 400),
        ("missing", 400),
    ],
)
async def test_invalid_intake_leaves_no_row(network, mutation, status):
    body = leaf_request(leaves(network)[0])
    if mutation == "signature":
        body["challenge_sig"] = "00" * 64
    elif mutation == "retired":
        body["challenge_id"] = "relearn"
    elif mutation == "bool":
        body["epoch"] = True
    elif mutation == "unknown_field":
        body["extra"] = "ignored?"
    elif mutation == "reason":
        body["score_or_absence"] = {"no_score": {"reason": 8}}
    else:
        del body["miner_hotkey"]
    response = await network.client.post("/v1/weights/raw", json=body)
    assert response.status_code == status
    assert network.store.leaves(12) == ()


async def test_duplicate_and_nonpositive_leaves_cannot_revoke_positive_score(network):
    positive = leaves(network)[1]
    first = await network.client.post("/v1/weights/raw", json=leaf_request(positive))
    repeated = sign_leaf(bytes([1]) * 32, b"bounty", positive.miner_hotkey, 12, Score(50))
    burns = [
        sign_leaf(
            bytes([1]) * 32,
            b"bounty",
            positive.miner_hotkey,
            12,
            NoScore(reason),
        )
        for reason in NoScoreReason
    ]
    zero = sign_leaf(bytes([1]) * 32, b"bounty", positive.miner_hotkey, 12, Score(0))
    for incoming in (repeated, *burns, zero):
        response = await network.client.post("/v1/weights/raw", json=leaf_request(incoming))
        assert response.status_code == 409
        assert response.json()["original"]["id"] == first.json()["id"]
        assert response.json()["original"]["score"] == 50
    assert network.store.leaves(12)[0].score == Score(50)


def bounty_leaves(network, *, winner, reason=NoScoreReason.CHALLENGE_INTERNAL):
    return tuple(
        sign_leaf(
            bytes([1]) * 32,
            b"bounty",
            row.hotkey,
            12,
            Score(50) if row.uid == winner else NoScore(reason),
        )
        for row in network.rows
    )


def test_complete_challenge_snapshot_can_replace_a_positive_with_an_outage_burn(network):
    expected = {row.hotkey for row in network.rows}
    network.service.replace_challenge_leaves(
        b"bounty", 12, expected, bounty_leaves(network, winner=1)
    )

    network.service.replace_challenge_leaves(
        b"bounty", 12, expected, bounty_leaves(network, winner=None)
    )

    stored = [leaf for leaf in network.store.leaves(12) if leaf.challenge_id == b"bounty"]
    assert len(stored) == len(expected)
    assert all(leaf.score == NoScore(NoScoreReason.CHALLENGE_INTERNAL) for leaf in stored)


def test_complete_challenge_snapshot_moves_the_winner_without_leaving_stale_scores(network):
    expected = {row.hotkey for row in network.rows}
    network.service.replace_challenge_leaves(
        b"bounty", 12, expected, bounty_leaves(network, winner=1)
    )

    network.service.replace_challenge_leaves(
        b"bounty", 12, expected, bounty_leaves(network, winner=2)
    )

    scores = {
        leaf.miner_hotkey: leaf.score
        for leaf in network.store.leaves(12)
        if leaf.challenge_id == b"bounty"
    }
    assert scores[network.rows[1].hotkey] == NoScore(NoScoreReason.CHALLENGE_INTERNAL)
    assert scores[network.rows[2].hotkey] == Score(50)


@pytest.mark.parametrize("mutation", ["missing", "duplicate", "wrong_challenge", "wrong_sig"])
def test_invalid_complete_challenge_snapshot_never_mutates_existing_rows(network, mutation):
    expected = {row.hotkey for row in network.rows}
    original = bounty_leaves(network, winner=1)
    network.service.replace_challenge_leaves(b"bounty", 12, expected, original)
    incoming = list(bounty_leaves(network, winner=2))
    if mutation == "missing":
        incoming.pop()
    elif mutation == "duplicate":
        incoming[-1] = incoming[0]
    elif mutation == "wrong_challenge":
        incoming[-1] = sign_leaf(
            bytes([2]) * 32,
            b"proof",
            network.rows[-1].hotkey,
            12,
            Score(50),
        )
    else:
        incoming[-1] = replace(incoming[-1], challenge_sig=b"\x00" * 64)

    with pytest.raises(ServiceError):
        network.service.replace_challenge_leaves(b"bounty", 12, expected, tuple(incoming))

    assert network.store.leaves(12) == original


def test_complete_challenge_snapshot_rolls_back_if_a_batch_insert_fails(network, monkeypatch):
    expected = {row.hotkey for row in network.rows}
    original = bounty_leaves(network, winner=1)
    network.service.replace_challenge_leaves(b"bounty", 12, expected, original)
    monkeypatch.setattr(gateway_store_module, "uuid4", lambda: "same-row-id")

    with pytest.raises(ServiceError, match="store unavailable"):
        network.service.replace_challenge_leaves(
            b"bounty", 12, expected, bounty_leaves(network, winner=2)
        )

    assert network.store.leaves(12) == original


def test_identical_complete_challenge_snapshot_retry_is_idempotent(network):
    expected = {row.hotkey for row in network.rows}
    snapshot = bounty_leaves(network, winner=1)

    first = network.service.replace_challenge_leaves(b"bounty", 12, expected, snapshot)
    second = network.service.replace_challenge_leaves(b"bounty", 12, expected, snapshot)

    assert second == first
    assert network.store.leaves(12) == snapshot


async def test_complete_challenge_snapshot_cannot_change_a_sealed_epoch(network):
    await intake(network)
    assert (await seal(network)).status_code == 200
    original = network.service.bundle_bytes(12)

    with pytest.raises(ServiceError, match="sealed"):
        network.service.replace_challenge_leaves(
            b"bounty",
            12,
            {row.hotkey for row in network.rows},
            bounty_leaves(network, winner=2),
        )

    assert network.service.bundle_bytes(12) == original


async def test_bounty_outage_burn_preserves_the_positive_proof_share(network):
    await intake(network)
    network.service.replace_challenge_leaves(
        b"bounty",
        12,
        {row.hotkey for row in network.rows},
        bounty_leaves(network, winner=None),
    )

    assert (await seal(network)).status_code == 200

    assert network.service.latest()["final_vector"] == [[0, 13107], [1, 52428]]


async def test_changed_payload_supersedes_and_sealed_bytes_stay_immutable(network):
    await intake(network)
    assert (await seal(network)).status_code == 200
    original = (await network.client.get("/v1/bundle/12")).content
    changed = sign_leaf(bytes([1]) * 32, b"bounty", network.rows[1].hotkey, 12, Score(100))
    response = await network.client.post("/v1/weights/raw", json=leaf_request(changed))
    assert response.status_code == 202 and response.json()["superseded"] is True
    assert (await seal(network)).status_code == 200
    assert (await network.client.get("/v1/bundle/12")).content == original
    assert (await seal(network, block_b=100)).status_code == 409


async def test_seal_and_raw_rows_survive_restart_byte_identically(network):
    await intake(network)
    second = GatewayStore(network.db_path)
    restarted = GatewayService(store=second, **network.settings)
    assert len(second.leaves(12)) == 6
    sealed = await restarted.seal(12, block_b=99)
    assert (await seal(network)).status_code == 200
    assert network.service.bundle_bytes(12) == sealed.encode()
    second.close()
    third = GatewayStore(network.db_path)
    restarted_again = GatewayService(store=third, **network.settings)
    assert restarted_again.bundle_bytes(12) == sealed.encode()
    assert restarted_again.latest() == network.service.latest()
    third.close()


async def test_profile_rotation_preserves_archives_and_waits_for_new_sealed_epoch(network):
    await intake(network)
    assert (await seal(network)).status_code == 200
    original = network.service.bundle_bytes(12)
    original_root = Bundle.decode(original).body.merkle_root.hex()
    trust = replace(
        network.trust,
        challenges=tuple(
            replace(entry, emission_share_bps=3000 if entry.id == b"bounty" else 7000)
            for entry in network.trust.challenges
        ),
        challenges_version=2,
        introduced_epoch=13,
    )
    network.service.trust_loader = lambda epoch: trust
    network.service.refresh_trust(13)
    assert network.service.latest()["sealed"] is False
    assert (await network.client.get("/v1/bundle/12")).content == original
    archived = await network.client.get(f"/v1/bundle/root/{original_root}")
    assert archived.status_code == 200
    assert archived.content == original
    validator = Validator(
        gateway_url="https://master.invalid",
        netuid=541,
        trust=trust,
        chain=network.chain,
        journal=network.journal,
        http=network.client,
    )
    assert (await validator.run_once()).outcome == "unsealed"
    assert network.chain.submissions == []
    await intake(network, epoch=13)
    assert (await seal(network, epoch=13)).status_code == 200
    assert network.service.latest()["algorithm_version"] == 2
    network.chain.epoch = 13
    assert (await validator.run_once()).outcome == "submitted"
    second = GatewayStore(network.db_path)
    try:
        restarted = GatewayService(store=second, **{**network.settings, "trust": trust})
        assert restarted.latest()["sealed"] is True
        assert restarted.latest()["algorithm_version"] == 2
        assert restarted.bundle_bytes(12) == original
        assert restarted.bundle_by_root(original_root) == original
    finally:
        second.close()


async def test_corrupt_latest_seal_burns_without_falling_back_to_older_seal(network):
    for epoch in (12, 13):
        await intake(network, epoch=epoch)
        assert (await seal(network, epoch=epoch)).status_code == 200
    with sqlite3.connect(network.db_path) as connection:
        connection.execute(
            "UPDATE gateway_bundles SET bundle_scale=? WHERE epoch=?", (b"corrupt", f"{13:020d}")
        )
    response = await network.client.get("/v1/weights/latest")
    assert response.status_code == 200
    assert response.json()["sealed"] is False
    assert response.json()["final_vector"] == [[0, 65535]]


async def test_operator_token_is_required_and_reread_on_rotation(network):
    await intake(network)
    path = "/v1/admin/seal"
    assert (await network.client.post(path, json={"epoch": 12})).status_code == 401
    network.token_file.write_text("rotated-token")
    assert (await seal(network)).status_code == 401
    response = await network.client.post(
        path, json={"epoch": 12}, headers={"authorization": "Bearer rotated-token"}
    )
    assert response.status_code == 200
    network.token_file.chmod(0o644)
    assert (await seal(network)).status_code == 503


async def test_simultaneous_seals_converge_on_the_same_immutable_bundle(network):
    await intake(network)
    network.chain.barrier = asyncio.Barrier(2)
    first, second = await asyncio.gather(seal(network), seal(network))
    assert first.status_code == second.status_code == 200
    assert first.json() == second.json()


async def test_wrong_subnet_and_unavailable_signing_key_never_persist_a_seal(network):
    await intake(network)
    assert (await seal(network, netuid=1)).status_code == 400
    network.service.gateway_seed = lambda: bytes([88]) * 32
    response = await seal(network)
    assert response.status_code == 503
    assert network.store.bundle(12) is None


async def test_corrupted_signature_in_latest_never_projects_as_a_sealed_vector(network):
    await intake(network)
    assert (await seal(network)).status_code == 200
    raw = bytearray(network.service.bundle_bytes(12))
    root = Bundle.decode(bytes(raw)).body.merkle_root.hex()
    raw[-1] ^= 1
    with sqlite3.connect(network.db_path) as connection:
        connection.execute("UPDATE gateway_bundles SET bundle_scale=?", (bytes(raw),))
    assert network.service.latest()["sealed"] is False
    assert (await network.client.get(f"/v1/bundle/root/{root}")).status_code == 503


@pytest.mark.parametrize("profile", [None, "null", "true", "3", '"text"', "[]", "{}", "{", b"{}"])
async def test_historical_root_refuses_corrupt_or_missing_accepted_profile(network, profile):
    await intake(network)
    assert (await seal(network)).status_code == 200
    root = network.service.latest()["merkle_root"]
    with sqlite3.connect(network.db_path) as connection:
        if profile is None:
            connection.execute("DELETE FROM gateway_trust_profiles")
        else:
            raw = profile.encode() if isinstance(profile, str) else profile
            connection.execute(
                "UPDATE gateway_trust_profiles SET profile=?,digest=?",
                (profile, sha256(raw).digest()),
            )

    response = await network.client.get(f"/v1/bundle/root/{root}")

    assert response.status_code == 503


async def test_historical_root_uses_accepted_challenge_keys_after_key_rotation(network):
    await intake(network)
    assert (await seal(network)).status_code == 200
    original = network.service.bundle_bytes(12)
    root = Bundle.decode(original).body.merkle_root.hex()
    trust = replace(
        network.trust,
        challenges=tuple(
            replace(entry, public_key=public_key(bytes([70 + index]) * 32))
            for index, entry in enumerate(network.trust.challenges)
        ),
        challenges_version=2,
        introduced_epoch=13,
    )
    network.service.trust_loader = lambda epoch: trust
    network.service.refresh_trust(13)
    second = GatewayStore(network.db_path)
    try:
        restarted = GatewayService(store=second, **{**network.settings, "trust": trust})
        assert restarted.latest()["sealed"] is False
        assert restarted.bundle_by_root(root) == original
        assert restarted.trust == trust
    finally:
        second.close()


async def test_duplicate_json_and_oversized_request_are_rejected_before_store(network):
    duplicate = await network.client.post("/v1/weights/raw", content='{"epoch": 1, "epoch": 2}')
    oversized = await network.client.post("/v1/weights/raw", content=b"x" * 4097)
    assert duplicate.status_code == 400
    assert oversized.status_code == 413
    assert network.store.leaves(12) == ()


async def test_gateway_database_cannot_be_reused_for_another_subnet(network):
    from cortex.errors import ServiceError

    second = GatewayStore(network.db_path)
    try:
        with pytest.raises(ServiceError, match="another subnet"):
            GatewayService(store=second, **{**network.settings, "netuid": 1})
    finally:
        second.close()


async def test_validator_rechecks_latest_after_bundle_verification(network):
    await intake(network)
    assert (await seal(network)).status_code == 200
    sealed = network.service.latest()
    bundle = network.service.bundle_bytes(12)
    reads = 0

    def transport(request):
        nonlocal reads
        if request.url.path == "/v1/weights/latest":
            reads += 1
            return httpx.Response(200, json=sealed if reads == 1 else {"sealed": False})
        return httpx.Response(200, content=bundle)

    async with httpx.AsyncClient(transport=httpx.MockTransport(transport)) as client:
        validator = Validator(
            gateway_url="https://master.invalid",
            netuid=541,
            trust=network.trust,
            chain=network.chain,
            journal=network.journal,
            http=client,
        )
        assert (await validator.run_once()).outcome == "unsealed"
    assert reads == 2
    assert network.chain.submissions == []


async def test_content_addressed_bundle_lookup_returns_identical_signed_bytes(network):
    await intake(network)
    assert (await seal(network)).status_code == 200
    root = network.service.latest()["merkle_root"]
    response = await network.client.get(f"/v1/bundle/root/{root}")
    assert response.status_code == 200
    assert response.content == network.service.bundle_bytes(12)
    assert (await network.client.get("/v1/bundle/root/" + "00" * 32)).status_code == 404
    assert (await network.client.get("/v1/bundle/root/invalid")).status_code == 400
