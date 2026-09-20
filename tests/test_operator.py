from pathlib import Path

import pytest

from cortex.cli import main
from cortex.operator import generate_key, sign_trust_document
from cortex.protocol import ProtocolError
from cortex.protocol.crypto import decode_hotkey, public_key
from cortex.protocol.trust import load_trust_root
from cortex.validator import SubmissionJournal


def _document(path: Path, body: str) -> Path:
    path.write_text(body)
    return path


def test_generated_key_signs_both_documents_for_runtime_verification(tmp_path):
    seed_path, owner_path = tmp_path / "owner.seed", tmp_path / "owner.pubkey"
    generated = generate_key(seed_path, owner_path)
    challenge_seed = bytes([4]) * 32
    challenges = _document(
        tmp_path / "challenges.toml",
        f"""version = 1
introduced_epoch = 0
[[challenges]]
id = "bounty"
public_key = "{public_key(challenge_seed).hex()}"
emission_share_bps = 2000
policy = "all_metagraph_hotkeys"
[[challenges]]
id = "proof"
public_key = "{public_key(bytes([5]) * 32).hex()}"
emission_share_bps = 8000
policy = "all_metagraph_hotkeys"
""",
    )
    measurements = _document(
        tmp_path / "measurements.toml", "version = 1\nintroduced_epoch = 0\nmeasurements = []\n"
    )
    for kind, document in (("challenges", challenges), ("measurements", measurements)):
        sign_trust_document(
            input_path=document,
            kind=kind,
            seed_path=seed_path,
            signature_path=Path(str(document) + ".sig"),
        )

    trust = load_trust_root(
        challenges_path=challenges,
        challenges_signature=Path(str(challenges) + ".sig"),
        measurements_path=measurements,
        measurements_signature=Path(str(measurements) + ".sig"),
        owner_public=decode_hotkey(owner_path.read_text().strip()),
        gateway_public=public_key(bytes([7]) * 32),
        epoch=0,
    )

    assert generated == owner_path.read_text().strip()
    assert seed_path.stat().st_mode & 0o777 == 0o600
    assert trust.shares == ((b"bounty", 2000), (b"proof", 8000))


def test_key_generation_refuses_to_overwrite_operator_material(tmp_path):
    seed_path, owner_path = tmp_path / "owner.seed", tmp_path / "owner.pubkey"
    generate_key(seed_path, owner_path)

    with pytest.raises(FileExistsError):
        generate_key(seed_path, tmp_path / "other.pubkey")

    assert len(seed_path.read_bytes()) == 32


def test_proportional_template_requires_an_explicit_activation_epoch_before_signing(tmp_path):
    seed_path = tmp_path / "owner.seed"
    generate_key(seed_path, tmp_path / "owner.pubkey")
    template = Path(__file__).resolve().parents[1] / "config/challenges-v2.example.toml"
    signature = tmp_path / "challenges.toml.sig"

    with pytest.raises(ProtocolError):
        sign_trust_document(
            input_path=template,
            kind="challenges",
            seed_path=seed_path,
            signature_path=signature,
        )

    assert not signature.exists()
    selected = _document(
        tmp_path / "challenges.toml",
        template.read_text().replace('"CHOOSE_ACTIVATION_EPOCH"', "123"),
    )
    sign_trust_document(
        input_path=selected,
        kind="challenges",
        seed_path=seed_path,
        signature_path=signature,
    )
    assert len(bytes.fromhex(signature.read_text())) == 64


def test_cli_verifies_the_signed_files_before_operator_install(tmp_path, capsys):
    seed_path, owner_path = tmp_path / "owner.seed", tmp_path / "owner.pubkey"
    generate_key(seed_path, owner_path)
    challenges = tmp_path / "challenges.toml"
    challenges.write_text(
        f'''version = 1
introduced_epoch = 0
[[challenges]]
id = "bounty"
public_key = "{public_key(bytes([4]) * 32).hex()}"
emission_share_bps = 2000
policy = "all_metagraph_hotkeys"
[[challenges]]
id = "proof"
public_key = "{public_key(bytes([5]) * 32).hex()}"
emission_share_bps = 8000
policy = "all_metagraph_hotkeys"
'''
    )
    measurements = tmp_path / "measurements.toml"
    measurements.write_text("version = 1\nmeasurements = []\n")
    for kind, document in (("challenges", challenges), ("measurements", measurements)):
        sign_trust_document(
            input_path=document,
            kind=kind,
            seed_path=seed_path,
            signature_path=Path(str(document) + ".sig"),
        )

    main(
        [
            "trust-verify",
            "--challenges",
            str(challenges),
            "--measurements",
            str(measurements),
            "--owner-public",
            str(owner_path),
            "--gateway-public",
            public_key(bytes([7]) * 32).hex(),
            "--epoch",
            "1",
        ]
    )

    assert '"challenges": {"bounty": 2000, "proof": 8000}' in capsys.readouterr().out


def test_cli_reconciles_only_the_exact_pending_validator_attempt(tmp_path, capsys):
    state = tmp_path / "validator.db"
    journal = SubmissionJournal(state)
    digest = "ab" * 32
    attempt_id = journal.claim(541, 12, digest)
    journal.record_uncertain(
        541,
        12,
        attempt_id,
        extrinsic_hash="0x" + "cd" * 32,
        nonce=17,
    )
    journal.close()

    main(
        [
            "validator-reconcile",
            "--state-db",
            str(state),
            "--netuid",
            "541",
            "--epoch",
            "12",
            "--digest",
            digest,
            "--attempt-id",
            attempt_id,
            "--result",
            "submitted",
            "--evidence-digest",
            "ef" * 32,
        ]
    )

    output = capsys.readouterr().out
    assert '"state": "reconciled_submitted"' in output
    reopened = SubmissionJournal(state)
    assert reopened.pending(541, 12) is None
    assert reopened.connection.execute(
        "SELECT state FROM weight_submissions WHERE netuid=541 AND epoch=12"
    ).fetchone() == ("submitted",)
    reopened.close()
