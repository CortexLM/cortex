import json
from dataclasses import replace
from hashlib import sha256
from types import SimpleNamespace

import httpx
import pytest

from cortex.protocol import (
    Bundle,
    ChallengeEntry,
    Leaf,
    MetagraphRow,
    NoScore,
    ProtocolError,
    Score,
    TrustRoot,
    aggregate_leaves,
    build_bundle,
    bundle_digest,
)
from cortex.protocol.crypto import public_key
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
            ChallengeEntry(b"bounty", 2000),
            ChallengeEntry(b"proof", 8000),
        ),
        sha256(b"\x00").digest(),
    )

    def seal(winner=None):
        leaves = tuple(
            Leaf(name, row.hotkey, 12, Score(50) if row.uid == winner else NoScore())
            for name in (b"bounty", b"proof")
            for row in rows
        )
        return build_bundle(
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


async def test_dishonest_final_vector_is_never_dispatched(network):
    body = replace(network.bundle.body, final_vector=((2, 65535),))
    network.bundle = Bundle(body)
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


def signed_peers(network, *, root=None, signer=22, epoch=12, offline=False, peer_consensus=True):
    from cortex.protocol.consensus import RootStatement

    validator = network.validator
    validator.consensus_seed = lambda: bytes([20]) * 32
    validator.peer_consensus = peer_consensus
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
        peer_consensus=True,
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


async def test_centralized_gateway_default_submits_without_a_peer_root_sample(network):
    """The gateway is authoritative: another permitted validator is not a precondition."""
    network.validator.consensus_seed = lambda: bytes([20]) * 32
    network.chain.view = replace(network.chain.view, validator_permits=frozenset({0, 2}))
    assert network.validator.peer_consensus is False

    result = await network.validator.run_once()

    assert result.outcome == "submitted"
    assert network.chain.submissions == [(541, ((1, 65535),), 1)]
    assert network.journal.evidence.local_root(12).merkle_root == network.bundle.body.merkle_root
    assert not network.journal.evidence.dissents()


async def test_local_equivocation_still_refuses_dispatch_without_peer_consensus(network):
    from cortex.protocol.consensus import DissentReason, RootStatement

    network.validator.consensus_seed = lambda: bytes([20]) * 32
    assert network.validator.peer_consensus is False
    network.journal.evidence.root(
        RootStatement.sign(bytes([20]) * 32, 12, bytes([99]) * 32), local=True
    )

    assert (await network.validator.run_once()).outcome == "peer_consensus_unavailable"
    assert not network.chain.submissions
    assert network.journal.evidence.dissents()[-1].reason == DissentReason.PEER_ROOT_CONFLICT


async def test_class_a_submits_independent_vector_and_signed_dissent_after_peer_agreement(network):
    from cortex.protocol.consensus import DissentReason
    from cortex.protocol.models import encode_final_vector

    body = replace(network.bundle.body, final_vector=((2, 65535),))
    network.bundle = Bundle(body)
    signed_peers(network)
    assert (await network.validator.run_once()).outcome == "submitted"
    assert network.chain.submissions == [(541, ((1, 65535),), 1)]
    dissent = network.journal.evidence.dissents()[-1]
    assert dissent.reason == DissentReason.VECTOR_MISMATCH
    assert dissent.expected_vector_hash == sha256(encode_final_vector(((1, 65535),))).digest()
    assert dissent.actual_vector_hash != dissent.expected_vector_hash


async def test_verify_only_reports_vector_dissent_as_a_degraded_outcome(network):
    body = replace(network.bundle.body, final_vector=((2, 65535),))
    network.bundle = Bundle(body)
    signed_peers(network)
    network.validator.verify_only = True

    result = await network.validator.run_once()

    assert result.outcome == "dissent_vector_mismatch"
    assert network.chain.submissions == []


@pytest.mark.parametrize("challenge,can_submit", [(b"bounty", True), (b"proof", False)])
async def test_quarantine_uses_surviving_mass_threshold(network, challenge, can_submit):
    from cortex.protocol.merkle import merkle_root

    leaves = tuple(
        replace(leaf, epoch=13) if leaf.challenge_id == challenge else leaf
        for leaf in network.bundle.body.leaves
    )
    body = replace(
        network.bundle.body,
        leaves=leaves,
        merkle_root=merkle_root(leaf.encode() for leaf in leaves),
    )
    network.bundle = Bundle(body)
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


def reset(db_path, *extra):
    from cortex.cli import main

    main(["validator-watermark-reset", "--state-db", str(db_path), *extra])


async def test_cli_watermark_reset_lowers_a_wrong_version_and_audits_it(network, capsys):
    import sqlite3

    original = network.validator.trust
    network.validator.trust_loader = lambda _: replace(original, challenges_version=2)
    await network.validator.run_once()
    network.journal.close()
    args = ("--name", "challenges_version", "--to", "1")
    for refused in (
        args,  # no --confirm
        ("--name", "challenges_version", "--to", "3", "--confirm"),  # raising
        ("--name", "netuid", "--to", "1", "--confirm"),
        ("--name", "challenges_version", "--to", "-1", "--confirm"),
    ):
        with pytest.raises(SystemExit):
            reset(network.db_path, *refused)
    capsys.readouterr()
    reset(network.db_path, *args, "--confirm")
    assert json.loads(capsys.readouterr().out) == {
        "name": "challenges_version",
        "old_value": 2,
        "new_value": 1,
    }
    with sqlite3.connect(network.db_path) as connection:
        assert connection.execute(
            "SELECT name, old_value, new_value FROM consensus_watermark_resets"
        ).fetchall() == [("challenges_version", "2", "1")]
    network.validator.journal = SubmissionJournal(network.db_path)
    network.validator.trust_loader = lambda _: original
    assert (await network.validator.run_once()).outcome == "already_submitted_or_pending"


def test_cli_watermark_reset_refuses_an_unset_watermark_and_allows_equal(network, capsys):
    network.journal.evidence.watermark("epoch", 12)
    network.journal.close()
    with pytest.raises(SystemExit):
        reset(network.db_path, "--name", "measurements_version", "--to", "0", "--confirm")
    reset(network.db_path, "--name", "epoch", "--to", "12", "--confirm")
    assert json.loads(capsys.readouterr().out)["new_value"] == 12


def test_cli_watermark_reset_reports_a_non_sqlite_state_file_without_traceback(tmp_path):
    state = tmp_path / "state.db"
    state.write_bytes(b"not a database" * 100)
    with pytest.raises(SystemExit, match="Cortex command failed"):
        reset(state, "--name", "epoch", "--to", "0", "--confirm")


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


async def test_v1_bundle_dissents_as_unsupported_version_before_decoding(network):
    from cortex.protocol.consensus import DissentReason

    real = network.bundle
    # v1 layout differs after the version, so decoding it as v2 would fail with another error.
    network.bundle = SimpleNamespace(body=real.body, encode=lambda: b"\x01\x00" + b"\xff" * 64)
    signed_peers(network)
    with pytest.raises(ProtocolError, match="unsupported bundle version"):
        await network.validator.run_once()
    assert network.chain.submissions == []
    reason = network.journal.evidence.dissents()[-1].reason
    assert reason == DissentReason.PROTOCOL_VERSION_UNSUPPORTED


async def test_hostile_trust_profile_does_not_move_the_version_watermark(network):
    from cortex.protocol.trust import trust_to_json

    hostile = {**trust_to_json(network.validator.trust), "challenges_version": 2**32 - 1}
    served = {"profile": hostile}

    def transport(request):
        if request.url.path == "/v1/trust":
            return httpx.Response(200, json=served["profile"])
        return network.handler(request)

    validator = Validator(
        gateway_url="https://master.invalid",
        netuid=541,
        chain=network.chain,
        journal=network.journal,
        http=httpx.AsyncClient(transport=httpx.MockTransport(transport)),
    )
    with pytest.raises(ProtocolError):  # profile does not match the bundle's shares/version
        await validator.run_once()
    assert network.chain.submissions == []
    served["profile"] = trust_to_json(network.validator.trust)
    assert (await validator.run_once()).outcome == "submitted"


async def test_cli_validator_reconcile_resolves_an_uncertain_dispatch(network, capsys):
    from cortex.cli import main

    network.chain.error = DispatchUncertain("RPC disconnected", extrinsic_hash="0x" + "ab" * 32)
    result = await network.validator.run_once()
    assert result.outcome == "dispatch_pending"
    main(
        [
            "validator-reconcile",
            "--state-db", str(network.db_path),
            "--netuid", "541",
            "--epoch", "12",
            "--digest", bundle_digest(network.bundle),
            "--attempt-id", result.attempt_id,
            "--result", "not_broadcast",
            "--evidence-digest", "cd" * 32,
        ]
    )  # fmt: skip
    assert '"state": "reconciled_not_broadcast"' in capsys.readouterr().out
    assert network.journal.pending(541, 12) is None
    network.chain.error = None
    assert (await network.validator.run_once()).outcome == "submitted"


def test_cli_validator_reconcile_refuses_an_unknown_dispatch(network):
    from cortex.cli import main

    with pytest.raises(SystemExit, match="Cortex command failed"):
        main(
            [
                "validator-reconcile",
                "--state-db", str(network.db_path),
                "--netuid", "541",
                "--epoch", "12",
                "--digest", "ab" * 32,
                "--attempt-id", "nope",
                "--result", "submitted",
                "--evidence-digest", "cd" * 32,
            ]
        )  # fmt: skip


@pytest.mark.parametrize("value", [True, 1.0, "1", None, [1], 2**32, 2**32 - 1, "max+1"], ids=repr)
async def test_unsupported_measurements_version_never_moves_the_watermark(network, value):
    import sqlite3

    from cortex.protocol.models import MAX_MEASUREMENTS_VERSION
    from cortex.protocol.trust import trust_to_json

    if value == "max+1":
        value = MAX_MEASUREMENTS_VERSION + 1
    honest = trust_to_json(network.validator.trust)
    served = {"profile": {**honest, "measurements_version": value}}

    def transport(request):
        if request.url.path == "/v1/trust":
            return httpx.Response(200, json=served["profile"])
        return network.handler(request)

    validator = Validator(
        gateway_url="https://master.invalid",
        netuid=541,
        chain=network.chain,
        journal=network.journal,
        http=httpx.AsyncClient(transport=httpx.MockTransport(transport)),
    )
    with pytest.raises(ProtocolError, match="invalid trust profile"):
        await validator.run_once()
    assert network.chain.submissions == []
    with sqlite3.connect(network.db_path) as connection:
        assert (
            connection.execute(
                "SELECT value FROM consensus_watermarks WHERE name='measurements_version'"
            ).fetchall()
            == []
        )
    served["profile"] = honest
    assert (await validator.run_once()).outcome == "submitted"


def test_measurements_version_at_the_supported_maximum_is_accepted(network):
    from cortex.protocol.models import MAX_MEASUREMENTS_VERSION

    replace(network.validator.trust, measurements_version=MAX_MEASUREMENTS_VERSION).validate()


def serve_v3_profile(network, challenges_version):
    """A v3-sealed epoch whose /v1/trust answer carries the given challenges_version."""
    from cortex.protocol.trust import trust_to_json

    honest = replace(network.validator.trust, challenges_version=3)
    leaves = tuple(
        Leaf(name, row.hotkey, 12, Score(50) if row.uid == 1 else NoScore())
        for name in (b"bounty", b"proof")
        for row in network.chain.view.rows
    )
    network.bundle = build_bundle(
        epoch=12,
        netuid=541,
        block_b=99,
        block_hash=bytes([9]) * 32,
        rows=network.chain.view.rows,
        leaves=leaves,
        trust=honest,
    )
    floats = aggregate_leaves(
        network.bundle.body.leaves, honest.shares, network.bundle.body.uid_map, algorithm_version=3
    )
    network.latest_overrides.update(uids=list(floats.uids), weights=list(floats.weights))
    served = {"profile": {**trust_to_json(honest), "challenges_version": challenges_version}}

    def transport(request):
        if request.url.path == "/v1/trust":
            return httpx.Response(200, json=served["profile"])
        return network.handler(request)

    validator = Validator(
        gateway_url="https://master.invalid",
        netuid=541,
        chain=network.chain,
        journal=network.journal,
        http=httpx.AsyncClient(transport=httpx.MockTransport(transport)),
    )
    return validator, served, trust_to_json(honest)


@pytest.mark.parametrize("value", [1001, 2**32 - 1, True, None, "5", 1.0, -1], ids=repr)
async def test_challenges_version_above_cap_is_refused_before_watermark(network, value):
    import sqlite3

    validator, served, honest = serve_v3_profile(network, value)
    with pytest.raises(ProtocolError, match="invalid trust profile"):
        await validator.run_once()
    assert network.chain.submissions == []
    with sqlite3.connect(network.db_path) as connection:
        assert connection.execute("SELECT name FROM consensus_watermarks").fetchall() == [
            ("netuid",)
        ]
    served["profile"] = honest
    assert (await validator.run_once()).outcome == "submitted"


async def test_challenges_version_boundary_1000_accepted(network):
    from cortex.protocol.models import MAX_CHALLENGES_VERSION

    assert MAX_CHALLENGES_VERSION == 1000
    validator, _, _ = serve_v3_profile(network, MAX_CHALLENGES_VERSION)
    assert (await validator.run_once()).outcome == "submitted"


def snapshot_file(path):
    import sqlite3

    stat = path.stat()
    # immutable=1 reads without creating -shm/-wal sidecars, unlike mode=ro.
    connection = sqlite3.connect(f"{path.resolve().as_uri()}?immutable=1", uri=True)
    try:
        mode = connection.execute("PRAGMA journal_mode").fetchone()[0]
        tables = connection.execute("SELECT name FROM sqlite_master ORDER BY name").fetchall()
    finally:
        connection.close()
    siblings = sorted(item.name for item in path.parent.iterdir())
    return path.read_bytes(), stat.st_mtime_ns, mode, tables, siblings


@pytest.mark.parametrize("kind", ["empty", "foreign", "foreign-wal", "partial"])
def test_cli_watermark_reset_never_mutates_a_foreign_state_file(tmp_path, kind):
    import sqlite3

    state = tmp_path / "other" / "state.db"
    state.parent.mkdir()
    if kind == "empty":
        state.touch()
    else:
        connection = sqlite3.connect(state)
        if kind == "foreign-wal":
            connection.execute("PRAGMA journal_mode=WAL")
        if kind == "partial":  # the watermark row alone, without the journal's other tables
            connection.execute("CREATE TABLE consensus_watermarks (name TEXT, value TEXT)")
            connection.execute("INSERT INTO consensus_watermarks VALUES ('epoch', '12')")
        else:
            connection.execute("CREATE TABLE notes (body TEXT)")
            connection.execute("INSERT INTO notes VALUES ('kept')")
        connection.commit()
        connection.close()
    before = snapshot_file(state)
    with pytest.raises(SystemExit, match="not a validator state database"):
        reset(state, "--name", "epoch", "--to", "0", "--confirm")
    assert snapshot_file(state) == before


def test_cli_watermark_reset_reports_a_locked_state_db(network):
    import sqlite3

    network.journal.evidence.watermark("epoch", 12)
    network.journal.close()
    holder = sqlite3.connect(network.db_path, isolation_level=None)
    holder.execute("BEGIN IMMEDIATE")
    try:
        with pytest.raises(SystemExit, match="state db locked; stop the validator first"):
            reset(network.db_path, "--name", "epoch", "--to", "11", "--confirm")
    finally:
        holder.execute("ROLLBACK")
        holder.close()
    with sqlite3.connect(network.db_path) as connection:
        value = connection.execute("SELECT value FROM consensus_watermarks WHERE name='epoch'")
        assert value.fetchone() == ("12",)


def test_cli_watermark_reset_checks_the_recorded_netuid(network, capsys):
    network.journal.evidence.watermark("epoch", 12)
    network.journal.close()
    with pytest.raises(SystemExit, match="belongs to another subnet"):
        reset(network.db_path, "--name", "epoch", "--to", "11", "--netuid", "7", "--confirm")
    reset(network.db_path, "--name", "epoch", "--to", "11", "--netuid", "541", "--confirm")
    assert json.loads(capsys.readouterr().out)["new_value"] == 11
