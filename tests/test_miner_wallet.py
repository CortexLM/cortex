"""Real Bittensor wallet files and wire signatures; HTTP remains a local fixture."""

import hashlib
import io
import json
import os
import stat
import tarfile
from email.parser import BytesParser
from email.policy import default

import httpx
import pytest
from bittensor_wallet import Keypair, Wallet

from cortex.bounty.service import pair_payload
from cortex.cli import parser
from cortex.errors import ServiceError
from cortex.miner import MinerClient
from cortex.proof.models import Baseline, Metric, Submission, Topic, digest
from cortex.proof.service import SUBMIT_DOMAIN, sign_topic
from cortex.protocol.crypto import public_key, verify_raw, verify_substrate
from cortex.wallet import HotkeySigner, load_wallet_hotkey


@pytest.fixture
def wallet(tmp_path):
    wallet = Wallet(name="research", hotkey="worker", path=str(tmp_path / "wallets"))
    wallet.set_hotkey(Keypair.create_from_uri("//Alice//research"), encrypt=False)
    return wallet


@pytest.fixture(scope="module")
def encrypted_wallet(tmp_path_factory):
    path = tmp_path_factory.mktemp("encrypted-wallet")
    wallet = Wallet(name="research", hotkey="worker", path=str(path))
    wallet.set_hotkey(
        Keypair.create_from_uri("//Bob//research"),
        encrypt=True,
        hotkey_password="fixture-password-only",
    )
    password = path / "password"
    password.write_text("fixture-password-only")
    password.chmod(0o600)
    return wallet, password


def load(wallet, **kwargs):
    return load_wallet_hotkey(name="research", hotkey="worker", path=wallet.path, **kwargs)


def test_derived_hotkey_signs_both_contexts_without_a_coldkey(wallet):
    signer = load(wallet)
    payload, domain = b"identity-proof", b"base-proof-submit-v1"

    cortex_signature = signer.sign(domain, payload)
    substrate_signature = signer.sign_substrate(payload)

    assert signer.public_key == wallet.hotkey.public_key
    assert verify_raw(signer.public_key, domain, payload, cortex_signature)
    assert verify_substrate(signer.public_key, payload, substrate_signature)
    assert not verify_substrate(signer.public_key, payload, cortex_signature)
    assert not verify_raw(signer.public_key, domain, payload, substrate_signature)
    assert not wallet.coldkey_file.exists_on_device()


def test_encrypted_hotkey_loads_without_rewriting_wallet(encrypted_wallet):
    wallet, password = encrypted_wallet
    original = wallet.hotkey_file.data

    signer = load(wallet, password_file=password)

    assert verify_raw(signer.public_key, b"test", b"body", signer.sign(b"test", b"body"))
    assert wallet.hotkey_file.data == original
    assert wallet.hotkey_file.is_encrypted()


def test_encrypted_hotkey_requires_explicit_private_password_file(encrypted_wallet):
    wallet, _ = encrypted_wallet

    with pytest.raises(ServiceError, match="password file required"):
        load(wallet)


def test_wrong_wallet_password_is_redacted(encrypted_wallet, tmp_path, capfd):
    wallet, _ = encrypted_wallet
    password = tmp_path / "password"
    password.write_text("wrong-private-password")
    password.chmod(0o600)

    with pytest.raises(ServiceError) as error:
        load(wallet, password_file=password)

    assert error.value.reason == "Bittensor hotkey unavailable"
    assert "wrong-private-password" not in str(error.value)
    captured = capfd.readouterr()
    assert "wrong-private-password" not in captured.out + captured.err


def test_wallet_password_file_must_be_private(encrypted_wallet, tmp_path):
    wallet, _ = encrypted_wallet
    password = tmp_path / "password"
    password.write_text("fixture-password-only")
    password.chmod(0o644)

    with pytest.raises(ServiceError, match="credential file must be private"):
        load(wallet, password_file=password)


def test_signer_rejects_mismatched_expanded_secret():
    import sr25519

    _, secret = sr25519.pair_from_seed(bytes([1]) * 32)
    with pytest.raises(ValueError, match="public key mismatch"):
        HotkeySigner(public_key(bytes([2]) * 32), secret)


def test_loading_unknown_hotkey_does_not_create_a_wallet(tmp_path):
    path = tmp_path / "wallets"
    with pytest.raises(ServiceError, match="Bittensor hotkey unavailable"):
        load_wallet_hotkey(name="unknown", hotkey="worker", path=path)
    assert not path.exists()


async def test_proof_without_pinned_owner_key_fails_before_network(wallet):
    def handle(request):
        pytest.fail("unpinned Proof request reached network")

    async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as http:
        miner = MinerClient(base_url="https://gateway.fixture", signer=load(wallet), client=http)
        with pytest.raises(ServiceError, match="Proof owner public key required"):
            await miner.topic("research-topic")


async def test_wallet_bounty_pair_uses_substrate_signature_without_proof_key(wallet):
    signer = load(wallet)
    requests = []

    def handle(request):
        body = json.loads(request.content)
        requests.append(body)
        assert request.url.path == "/challenge/bounty/v1/pair"
        assert body["hotkey"] == signer.ss58_address
        assert body["terms_accepted"] is True
        assert verify_substrate(
            signer.public_key,
            pair_payload(body["account_id"], body["nonce"], body["exp"]),
            bytes.fromhex(body["signature"]),
        )
        return httpx.Response(200, json={"session": "fixture-session"})

    async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as http:
        miner = MinerClient(base_url="https://gateway.fixture", signer=signer, client=http)
        assert await miner.pair_bounty(account_id="researcher", accept_terms=True) == {
            "session": "fixture-session"
        }
    assert len(requests) == 1


async def test_wallet_proof_submit_keeps_context_artifact_and_unsigned_env(wallet, tmp_path):
    signer = load(wallet)
    owner_seed = bytes([7]) * 32
    metrics = {"quality": 0.5}
    topic = sign_topic(
        Topic(
            id="research-topic",
            statement="Fixture research",
            status="open",
            metric=Metric(
                family="custom",
                primary="quality",
                direction="max",
                epsilon=0.1,
                custom_id="fixture-runner",
            ),
            flops_budget=100,
            wall_budget_s=60,
            baseline=Baseline(
                script_sha256="aa" * 32,
                metrics=metrics,
                metrics_commitment=digest(metrics),
                evidence_digest="bb" * 32,
                flops_budget=100,
                wall_budget_s=60,
            ),
            holdout_commitment="cc" * 32,
            eval_image_digest="sha256:" + "dd" * 32,
            inference_offer_commitment="ee" * 32,
        ),
        owner_seed,
    )
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        entry = tarfile.TarInfo("research.txt")
        entry.size = 4
        archive.addfile(entry, io.BytesIO(b"data"))
    artifact = output.getvalue()
    received = []
    receipt = tmp_path / "submission.receipt.json"

    def handle(request):
        if request.method == "GET":
            return httpx.Response(200, json=topic.model_dump())
        message = BytesParser(policy=default).parsebytes(
            b"Content-Type: "
            + request.headers["content-type"].encode()
            + b"\r\n\r\n"
            + request.content
        )
        parts = {
            part.get_param("name", header="content-disposition"): part.get_payload(decode=True)
            for part in message.iter_parts()
        }
        body = json.loads(parts["json"])
        submission = Submission.model_validate(body)
        assert request.url.path == "/challenge/proof/v1/submissions"
        assert parts["artifact"] == artifact
        assert body["env"] == {"FIXTURE_KEY": "private-fixture"}
        assert receipt.is_file()
        assert stat.S_IMODE(receipt.stat().st_mode) == 0o600
        receipt_body = json.loads(receipt.read_text())
        assert "env" not in receipt_body
        assert "artifact_uri" not in receipt_body
        assert receipt_body["hotkey_signature"] == body["hotkey_signature"]
        assert submission.artifact_digest == hashlib.sha256(artifact).hexdigest()
        assert submission.miner_hotkey == signer.public_key.hex()
        signature = bytes.fromhex(submission.hotkey_signature)
        assert verify_raw(signer.public_key, SUBMIT_DOMAIN, submission.signing_payload(), signature)
        assert not verify_substrate(signer.public_key, submission.signing_payload(), signature)
        changed_env = submission.model_copy(update={"env": {"FIXTURE_KEY": "rotated"}})
        assert changed_env.signing_payload() == submission.signing_payload()
        received.append(submission)
        return httpx.Response(201, json={"status": "accepted"})

    async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as http:
        miner = MinerClient(
            base_url="https://gateway.fixture",
            signer=signer,
            proof_public_key=public_key(owner_seed),
            client=http,
        )
        result = await miner.submit_proof(
            topic_id=topic.id,
            artifact=artifact,
            claim="Measured improvement",
            env={"FIXTURE_KEY": "private-fixture"},
            nonce="ff" * 32,
            receipt_path=receipt,
        )
    assert result == {"status": "accepted"}
    assert len(received) == 1


async def test_wallet_proof_lookup_replays_only_the_private_signed_receipt(wallet, tmp_path):
    signer = load(wallet)
    signed = Submission.model_validate(
        {
            "topic_id": "research-topic",
            "miner_hotkey": signer.public_key.hex(),
            "artifact_digest": "ab" * 32,
            "claim": "Recover this submission",
            "submit_nonce": "cd" * 32,
            "hotkey_signature": "00" * 64,
        }
    )
    signed = signed.model_copy(
        update={"hotkey_signature": signer.sign(SUBMIT_DOMAIN, signed.signing_payload()).hex()}
    )
    receipt = tmp_path / "submission.receipt.json"
    receipt.write_text(signed.model_dump_json(exclude={"env", "artifact_uri"}))
    receipt.chmod(0o600)

    def handle(request):
        assert request.url.path == "/challenge/proof/v1/submissions/lookup"
        body = json.loads(request.content)
        assert "env" not in body
        assert "artifact_uri" not in body
        assert body["hotkey_signature"] == signed.hotkey_signature
        return httpx.Response(200, json={"status": "pending"})

    async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as http:
        miner = MinerClient(base_url="https://gateway.fixture", signer=signer, client=http)
        result = await miner.lookup_proof(receipt)

    assert result == {"status": "pending"}


async def test_existing_proof_receipt_blocks_submission_before_post(wallet, tmp_path):
    signer = load(wallet)
    owner_seed = bytes([7]) * 32
    topic = sign_topic(
        Topic(
            id="research-topic",
            statement="Fixture research",
            status="open",
            metric=Metric(
                family="custom",
                primary="quality",
                direction="max",
                epsilon=0.1,
                custom_id="fixture-runner",
            ),
            flops_budget=100,
            wall_budget_s=60,
            baseline=Baseline(
                script_sha256="aa" * 32,
                metrics={"quality": 0.5},
                metrics_commitment=digest({"quality": 0.5}),
                evidence_digest="bb" * 32,
                flops_budget=100,
                wall_budget_s=60,
            ),
            holdout_commitment="cc" * 32,
            eval_image_digest="sha256:" + "dd" * 32,
            inference_offer_commitment="ee" * 32,
        ),
        owner_seed,
    )
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        entry = tarfile.TarInfo("research.txt")
        entry.size = 4
        archive.addfile(entry, io.BytesIO(b"data"))
    receipt = tmp_path / "submission.receipt.json"
    receipt.write_text("do not overwrite")
    receipt.chmod(0o600)
    posts = []

    def handle(request):
        if request.method == "POST":
            posts.append(request)
            pytest.fail("submission POST happened before exclusive receipt creation")
        return httpx.Response(200, json=topic.model_dump())

    async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as http:
        miner = MinerClient(
            base_url="https://gateway.fixture",
            signer=signer,
            proof_public_key=public_key(owner_seed),
            client=http,
        )
        with pytest.raises(FileExistsError):
            await miner.submit_proof(
                topic_id=topic.id,
                artifact=output.getvalue(),
                claim="Measured improvement",
                receipt_path=receipt,
            )

    assert receipt.read_text() == "do not overwrite"
    assert posts == []


def test_proof_receipt_syncs_file_and_directory_before_return(wallet, tmp_path, monkeypatch):
    from cortex.miner import write_submission_receipt
    from cortex.proof.models import SubmissionLookup

    signer = load(wallet)
    unsigned = SubmissionLookup(
        topic_id="research-topic",
        miner_hotkey=signer.public_key.hex(),
        artifact_digest="ab" * 32,
        claim="Durable receipt",
        submit_nonce="cd" * 32,
        hotkey_signature="00" * 64,
    )
    envelope = unsigned.model_copy(
        update={"hotkey_signature": signer.sign(SUBMIT_DOMAIN, unsigned.signing_payload()).hex()}
    )
    synced = []
    real_fsync = os.fsync

    def record_fsync(descriptor):
        synced.append(stat.S_ISDIR(os.fstat(descriptor).st_mode))
        real_fsync(descriptor)

    monkeypatch.setattr("cortex.miner.os.fsync", record_fsync)
    receipt = tmp_path / "submission.receipt.json"

    write_submission_receipt(receipt, envelope)

    assert synced == [False, True]
    assert stat.S_IMODE(receipt.stat().st_mode) == 0o600


def test_miner_cli_requires_receipt_for_submit_and_accepts_lookup():
    command = parser()
    common = [
        "miner",
        "--gateway",
        "https://gateway.fixture",
        "--dev-seed-file",
        "/tmp/seed",
    ]
    with pytest.raises(SystemExit):
        command.parse_args(
            [
                *common,
                "proof-submit",
                "--topic",
                "research-topic",
                "--artifact",
                "/tmp/artifact.tar",
                "--claim-file",
                "/tmp/claim.txt",
            ]
        )
    args = command.parse_args(
        [*common, "proof-lookup", "--receipt", "/tmp/submission.receipt.json"]
    )
    assert args.action == "proof-lookup"
    assert args.receipt.name == "submission.receipt.json"


def test_miner_cli_accepts_wallet_and_requires_explicit_dev_seed():
    command = parser()
    action = ["bounty-pair", "--account-id", "researcher", "--session-file", "/tmp/session"]
    args = command.parse_args(
        [
            "miner",
            "--gateway",
            "https://gateway.fixture",
            "--wallet-name",
            "research",
            "--wallet-hotkey",
            "worker",
            *action,
        ]
    )
    assert args.wallet_name == "research"
    assert args.wallet_hotkey == "worker"
    assert args.proof_public is None
    assert (
        command.parse_args(
            [
                "miner",
                "--gateway",
                "https://gateway.fixture",
                "--dev-seed-file",
                "/tmp/seed",
                *action,
            ]
        ).dev_seed_file
        is not None
    )
    with pytest.raises(SystemExit):
        command.parse_args(
            [
                "miner",
                "--gateway",
                "https://gateway.fixture",
                "--seed-file",
                "/tmp/seed",
                *action,
            ]
        )
