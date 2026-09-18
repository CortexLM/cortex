from dataclasses import replace
from hashlib import sha256
from types import SimpleNamespace

import httpx
import pytest

from cortex.protocol import (
    Bundle,
    ChallengeEntry,
    MetagraphRow,
    NoScore,
    ProtocolError,
    Score,
    TrustRoot,
    aggregate_leaves,
    build_bundle,
    bundle_digest,
    sign_leaf,
)
from cortex.protocol.crypto import BUNDLE_DOMAIN, public_key, sign_raw
from cortex.validator import ChainSnapshot, SubmissionJournal, Validator
from cortex.validator.service import DispatchNotBroadcast, DispatchUncertain


class FakeChain:
    """Only the external chain boundary is substituted; seals use real crypto."""

    def __init__(self, rows):
        self.view = ChainSnapshot(99, bytes([9]) * 32, rows, rows[0].hotkey, frozenset())
        self.submissions = []
        self.success = True
        self.error = None
        self.preflight_error = None
        self.preflights = []

    async def snapshot(self, block, netuid):
        assert block == 99 and netuid == 541
        return self.view

    async def current_block(self):
        return 100

    async def submit(self, netuid, vector, version_key):
        self.submissions.append((netuid, vector, version_key))
        if self.error:
            raise self.error
        return self.success

    async def preflight(self, netuid, vector, version_key):
        self.preflights.append((netuid, vector, version_key))
        if self.preflight_error:
            raise self.preflight_error


@pytest.fixture
def network(tmp_path):
    rows = tuple(MetagraphRow(public_key(bytes([20 + tag]) * 32), tag) for tag in range(3))
    trust = TrustRoot(
        (
            ChallengeEntry(b"bounty", public_key(bytes([1]) * 32), 2000),
            ChallengeEntry(b"proof", public_key(bytes([2]) * 32), 8000),
        ),
        sha256(b"\x00").digest(),
        public_key(bytes([7]) * 32),
    )

    def seal(winner=None):
        leaves = tuple(
            sign_leaf(
                bytes([seed]) * 32,
                name,
                row.hotkey,
                12,
                Score(50) if row.uid == winner else NoScore(),
            )
            for name, seed in ((b"bounty", 1), (b"proof", 2))
            for row in rows
        )
        return build_bundle(
            gateway_seed=bytes([7]) * 32,
            epoch=12,
            netuid=541,
            block_b=99,
            block_hash=bytes([9]) * 32,
            rows=rows,
            leaves=leaves,
            trust=trust,
        )

    result = SimpleNamespace(
        bundle=seal(1),
        seal=seal,
        latest_overrides={},
        paths=[],
        chain=FakeChain(rows),
        journal=SubmissionJournal(tmp_path / "state.db"),
        db_path=tmp_path / "state.db",
    )

    def transport(request):
        assert request.method == "GET"
        result.paths.append(request.url.path)
        if request.url.path == "/v1/weights/latest":
            floats = aggregate_leaves(
                result.bundle.body.leaves, trust.shares, result.bundle.body.uid_map
            )
            latest = dict(
                sealed=True,
                epoch=12,
                netuid=541,
                merkle_root=result.bundle.body.merkle_root.hex(),
                bundle_digest=bundle_digest(result.bundle),
                uids=list(floats.uids),
                weights=list(floats.weights),
            )
            latest.update(result.latest_overrides)
            return httpx.Response(200, json=latest)
        assert request.url.path == "/v1/bundle/12"
        return httpx.Response(200, content=result.bundle.encode())

    client = httpx.AsyncClient(transport=httpx.MockTransport(transport))
    result.handler = transport
    result.validator = Validator(
        gateway_url="https://master.invalid",
        netuid=541,
        trust=trust,
        chain=result.chain,
        journal=result.journal,
        http=client,
    )
    return result


async def test_valid_seal_submits_recomputed_vector_once_across_restart(network):
    result = await network.validator.run_once()
    assert result.outcome == "submitted"
    assert network.chain.submissions == [(541, ((1, 65535),), 1)]
    network.journal.close()
    network.validator.journal = SubmissionJournal(network.db_path)
    assert (await network.validator.run_once()).outcome == "already_submitted_or_pending"
    assert len(network.chain.submissions) == 1


async def test_verify_only_checks_the_seal_without_claiming_or_submitting(network):
    network.validator.verify_only = True

    assert (await network.validator.run_once()).outcome == "verified"
    assert network.chain.submissions == []
    assert network.chain.preflights == [(541, ((1, 65535),), 1)]

    network.validator.verify_only = False
    assert (await network.validator.run_once()).outcome == "submitted"


async def test_verify_only_refuses_a_live_chain_preflight_failure(network):
    network.validator.verify_only = True
    network.chain.preflight_error = DispatchNotBroadcast("weight rate limit not elapsed")

    assert (await network.validator.run_once()).outcome == "chain_preflight_failed"
    assert network.chain.submissions == []


@pytest.mark.parametrize(
    "gateway_url",
    [
        "http://master.invalid",
        "https://user:secret@master.invalid",
        "https://master.invalid/path",
        "https://master.invalid?token=secret",
        "https://master.invalid#fragment",
    ],
)
def test_validator_requires_a_credential_free_https_gateway_origin(network, gateway_url):
    with pytest.raises(ValueError, match="gateway URL requires HTTPS"):
        Validator(
            gateway_url=gateway_url,
            netuid=541,
            trust=network.validator.trust,
            chain=network.chain,
            journal=network.journal,
            http=network.validator.http,
        )


async def test_unsealed_latest_never_uses_previous_verified_seal(network):
    await network.validator.run_once()
    network.chain.submissions.clear()
    network.paths.clear()
    network.latest_overrides["sealed"] = False
    assert (await network.validator.run_once()).outcome == "unsealed"
    assert network.chain.submissions == []
    assert network.paths == ["/v1/weights/latest"]


async def test_sealed_burn_uid_zero_must_be_submitted(network):
    network.bundle = network.seal()
    network.latest_overrides["burn_outcome"] = True
    assert (await network.validator.run_once()).outcome == "submitted"
    assert network.chain.submissions == [(541, ((0, 65535),), 1)]


@pytest.mark.parametrize("role", ["owner", "validator"])
async def test_nonzero_owner_or_validator_monopoly_is_not_submitted(network, role):
    view = network.chain.view
    network.chain.view = (
        replace(view, owner_hotkey=view.rows[1].hotkey)
        if role == "owner"
        else replace(view, validator_permits=frozenset({1}))
    )
    assert (await network.validator.run_once()).outcome == "owner_or_validator_monopoly"
    assert network.chain.submissions == []


@pytest.mark.parametrize(
    "changes,reason",
    [
        ({"merkle_root": "00" * 32}, "Merkle root"),
        ({"bundle_digest": "00" * 32}, "digest"),
        ({"weights": [0.5]}, "float vector"),
        ({"netuid": 1}, "subnet"),
    ],
)
async def test_latest_projection_tampering_refuses_dispatch(network, changes, reason):
    network.latest_overrides.update(changes)
    with pytest.raises(ProtocolError, match=reason):
        await network.validator.run_once()
    assert network.chain.submissions == []


async def test_dishonest_signed_final_vector_is_never_dispatched(network):
    body = replace(network.bundle.body, final_vector=((2, 65535),))
    network.bundle = Bundle(body, sign_raw(bytes([7]) * 32, BUNDLE_DOMAIN, body.encode()))
    with pytest.raises(ProtocolError, match="final vector"):
        await network.validator.run_once()
    assert network.chain.submissions == []


async def test_known_dispatch_rejection_can_retry(network):
    network.chain.success = False
    assert (await network.validator.run_once()).outcome == "dispatch_failed"
    network.chain.success = True
    assert (await network.validator.run_once()).outcome == "submitted"


async def test_pre_broadcast_dispatch_failure_releases_claim(network):
    network.chain.error = DispatchNotBroadcast("validator hotkey is not registered")
    assert (await network.validator.run_once()).outcome == "dispatch_failed"

    network.chain.error = None
    assert (await network.validator.run_once()).outcome == "submitted"
    assert len(network.chain.submissions) == 2


async def test_classified_ambiguous_dispatch_keeps_claim_pending(network):
    network.chain.error = DispatchUncertain(
        "RPC disconnected after broadcasting",
        extrinsic_hash="0x" + "ab" * 32,
        nonce=17,
    )
    result = await network.validator.run_once()
    assert result.outcome == "dispatch_pending"
    assert result.attempt_id
    assert result.extrinsic_hash == "0x" + "ab" * 32
    assert result.nonce == 17
    assert network.journal.pending(541, 12) == {
        "attempt_id": result.attempt_id,
        "digest": bundle_digest(network.bundle),
        "extrinsic_hash": "0x" + "ab" * 32,
        "nonce": 17,
        "state": "uncertain",
    }

    network.chain.error = None
    assert (await network.validator.run_once()).outcome == "already_submitted_or_pending"
    assert len(network.chain.submissions) == 1


async def test_operator_reconciliation_can_release_a_proven_not_broadcast_attempt(network):
    network.chain.error = DispatchUncertain("RPC disconnected")
    pending = await network.validator.run_once()

    reconciled = network.journal.reconcile(
        netuid=541,
        epoch=12,
        digest=bundle_digest(network.bundle),
        attempt_id=pending.attempt_id,
        result="not_broadcast",
        evidence_digest="cd" * 32,
    )
    network.chain.error = None

    assert reconciled["state"] == "reconciled_not_broadcast"
    assert (await network.validator.run_once()).outcome == "submitted"
    assert len(network.chain.submissions) == 2


def test_reconciliation_refuses_the_wrong_bundle_digest_or_attempt(network):
    digest = bundle_digest(network.bundle)
    attempt_id = network.journal.claim(541, 12, digest)

    with pytest.raises(ProtocolError, match="reconciliation identity"):
        network.journal.reconcile(
            netuid=541,
            epoch=12,
            digest="00" * 32,
            attempt_id=attempt_id,
            result="submitted",
            evidence_digest="cd" * 32,
        )

    assert network.journal.pending(541, 12)["attempt_id"] == attempt_id


def test_reconciliation_cannot_release_an_active_dispatch(network):
    digest = bundle_digest(network.bundle)
    attempt_id = network.journal.claim(541, 12, digest)

    with pytest.raises(ProtocolError, match="reconciliation identity"):
        network.journal.reconcile(
            netuid=541,
            epoch=12,
            digest=digest,
            attempt_id=attempt_id,
            result="not_broadcast",
            evidence_digest="cd" * 32,
        )

    network.journal.complete(541, 12, attempt_id)
    assert network.journal.claim(541, 12, digest) is None


def test_validator_startup_marks_an_interrupted_dispatch_uncertain(network):
    digest = bundle_digest(network.bundle)
    attempt_id = network.journal.claim(541, 12, digest)
    assert network.journal.pending(541, 12)["state"] == "dispatching"

    Validator(
        gateway_url="https://master.invalid",
        netuid=541,
        trust=network.validator.trust,
        chain=network.chain,
        journal=network.journal,
        http=network.validator.http,
    )

    assert network.journal.pending(541, 12) == {
        "attempt_id": attempt_id,
        "digest": digest,
        "extrinsic_hash": None,
        "nonce": None,
        "state": "uncertain",
    }


async def test_ambiguous_dispatch_exception_requires_reconciliation(network):
    network.chain.error = TimeoutError("RPC disconnected after broadcasting")
    with pytest.raises(TimeoutError):
        await network.validator.run_once()
    network.chain.error = None
    assert (await network.validator.run_once()).outcome == "already_submitted_or_pending"
    assert len(network.chain.submissions) == 1


def signed_peers(network, *, root=None, signer=22, epoch=12, offline=False):
    from cortex.protocol.consensus import RootStatement

    validator = network.validator
    validator.consensus_seed = lambda: bytes([20]) * 32
    validator.peers = {public_key(bytes([22]) * 32): "https://peer.invalid"}
    network.chain.view = replace(network.chain.view, validator_permits=frozenset({0, 2}))

    def respond(request):
        if request.url.host == "peer.invalid":
            if offline:
                return httpx.Response(503)
            return httpx.Response(
                200,
                json=RootStatement.sign(
                    bytes([signer]) * 32, epoch, root or network.bundle.body.merkle_root
                ).to_json(),
            )
        return network.handler(request)

    validator.http = httpx.AsyncClient(transport=httpx.MockTransport(respond))


async def test_authenticated_peer_root_and_bundle_persist_before_submission(network):
    signed_peers(network)
    assert (await network.validator.run_once()).outcome == "submitted"
    assert network.journal.evidence.local_root(12).merkle_root == network.bundle.body.merkle_root
    network.journal.close()
    restarted = SubmissionJournal(network.db_path)
    assert restarted.evidence.local_root(12).merkle_root == network.bundle.body.merkle_root
    assert restarted.connection.execute("SELECT count(*) FROM consensus_roots").fetchone()[0] == 2
    restarted.close()


@pytest.mark.parametrize("failure", ["offline", "impostor", "wrong_epoch", "zero_sample"])
async def test_multi_validator_requires_metagraph_authenticated_peer_sample(network, failure):
    from cortex.protocol.consensus import DissentReason

    signed_peers(
        network,
        signer=23 if failure == "impostor" else 22,
        epoch=11 if failure == "wrong_epoch" else 12,
        offline=failure == "offline",
    )
    if failure == "zero_sample":
        network.validator.min_peer_sample = 0
    assert (await network.validator.run_once()).outcome == "peer_consensus_unavailable"
    assert not network.chain.submissions
    dissent = network.journal.evidence.dissents()[-1]
    dissent.verify()
    assert dissent.reason == DissentReason.PEER_SAMPLE_INSUFFICIENT


async def test_peer_equivocation_persists_both_signed_roots_and_refuses_dispatch(network):
    from cortex.protocol.consensus import DissentReason, RootStatement

    signed_peers(network)
    previous = RootStatement.sign(bytes([22]) * 32, 12, bytes([99]) * 32)
    network.journal.evidence.root(previous)
    assert (await network.validator.run_once()).outcome == "peer_consensus_unavailable"
    assert not network.chain.submissions
    assert network.journal.evidence.dissents()[-1].reason == DissentReason.PEER_ROOT_CONFLICT
    assert (
        network.journal.connection.execute(
            "SELECT count(*) FROM consensus_roots WHERE hotkey=?", (previous.hotkey,)
        ).fetchone()[0]
        == 2
    )


async def test_class_a_submits_independent_vector_and_signed_dissent_after_peer_agreement(network):
    from cortex.protocol.consensus import DissentReason
    from cortex.protocol.models import encode_final_vector

    body = replace(network.bundle.body, final_vector=((2, 65535),))
    network.bundle = Bundle(body, sign_raw(bytes([7]) * 32, BUNDLE_DOMAIN, body.encode()))
    signed_peers(network)
    assert (await network.validator.run_once()).outcome == "submitted"
    assert network.chain.submissions == [(541, ((1, 65535),), 1)]
    dissent = network.journal.evidence.dissents()[-1]
    assert dissent.reason == DissentReason.VECTOR_MISMATCH
    assert dissent.expected_vector_hash == sha256(encode_final_vector(((1, 65535),))).digest()
    assert dissent.actual_vector_hash != dissent.expected_vector_hash


async def test_verify_only_reports_vector_dissent_as_a_degraded_outcome(network):
    body = replace(network.bundle.body, final_vector=((2, 65535),))
    network.bundle = Bundle(body, sign_raw(bytes([7]) * 32, BUNDLE_DOMAIN, body.encode()))
    signed_peers(network)
    network.validator.verify_only = True

    result = await network.validator.run_once()

    assert result.outcome == "dissent_vector_mismatch"
    assert network.chain.submissions == []


@pytest.mark.parametrize("challenge,can_submit", [(b"bounty", True), (b"proof", False)])
async def test_quarantine_uses_surviving_signed_mass_threshold(network, challenge, can_submit):
    from cortex.protocol.merkle import merkle_root

    leaves = tuple(
        replace(leaf, challenge_sig=bytes(64)) if leaf.challenge_id == challenge else leaf
        for leaf in network.bundle.body.leaves
    )
    body = replace(
        network.bundle.body,
        leaves=leaves,
        merkle_root=merkle_root(leaf.encode() for leaf in leaves),
    )
    network.bundle = Bundle(body, sign_raw(bytes([7]) * 32, BUNDLE_DOMAIN, body.encode()))
    signed_peers(network)
    if can_submit:
        assert (await network.validator.run_once()).outcome == "submitted"
        assert network.chain.submissions == [(541, ((1, 65535),), 1)]
    else:
        with pytest.raises(ProtocolError, match="share mass"):
            await network.validator.run_once()
        assert not network.chain.submissions


async def test_seal_older_than_chain_freshness_window_refuses_dispatch(network):
    async def current():
        return 99 + 257

    network.chain.current_block = current
    with pytest.raises(ProtocolError, match="stale"):
        await network.validator.run_once()
    assert not network.chain.submissions


async def test_reorg_after_peer_check_refuses_dispatch(network):
    reads = 0

    async def snapshot(block, netuid):
        nonlocal reads
        reads += 1
        return (
            network.chain.view if reads == 1 else replace(network.chain.view, block_hash=bytes(32))
        )

    network.chain.snapshot = snapshot
    with pytest.raises(ProtocolError, match="changed before dispatch"):
        await network.validator.run_once()
    assert not network.chain.submissions


async def test_future_or_invented_epoch_is_not_signed_or_dispatched(network):
    network.chain.view = replace(network.chain.view, epoch=11)
    network.validator.consensus_seed = lambda: bytes([20]) * 32
    with pytest.raises(ProtocolError, match="epoch does not match"):
        await network.validator.run_once()
    assert not network.chain.submissions
    assert network.journal.evidence.local_root(12) is None


def test_validator_journal_cannot_mix_subnets(network):
    with pytest.raises(ProtocolError, match="another subnet"):
        network.journal.bind_netuid(1)


async def test_trust_version_rollback_is_refused_across_restart(network):
    original = network.validator.trust
    network.validator.trust_loader = lambda _: replace(original, challenges_version=2)
    await network.validator.run_once()
    network.journal.close()
    network.validator.journal = SubmissionJournal(network.db_path)
    network.validator.trust_loader = lambda _: original
    with pytest.raises(ProtocolError, match="rollback"):
        await network.validator.run_once()
    assert len(network.chain.submissions) == 1


async def test_peer_api_serves_signed_evidence_and_never_claims_dcap_verified(network):
    from cortex.protocol.consensus import RootStatement
    from cortex.validator.evidence import peer_app

    signed_peers(network)
    await network.validator.run_once()
    app = peer_app(network.journal.evidence)
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="https://peer"
    ) as http:
        assert (await http.get("/livez")).json() == {"ok": True, "role": "validator"}
        response = await http.get("/v1/consensus/root/12")
        statement = RootStatement.from_json(response.json())
        assert statement.hotkey == public_key(bytes([20]) * 32)
        root = network.bundle.body.merkle_root.hex()
        assert (await http.get(f"/v1/bundle/root/{root}")).content == network.bundle.encode()
        for path in ("nonce", "submit"):
            refused = await http.post(f"/v1/attest/{path}", json={})
            assert refused.status_code == 503
            assert refused.json()["verified"] is False
