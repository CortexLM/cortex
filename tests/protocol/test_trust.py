from pathlib import Path

import pytest

from cortex.protocol import ProtocolError
from cortex.protocol.crypto import TRUST_ROOT_DOMAIN, decode_hotkey, public_key, sign_raw
from cortex.protocol.trust import load_trust_root, signing_payload

REFERENCE = Path(__file__).parent / "vectors/trust"


def arguments():
    return dict(
        challenges_path=REFERENCE / "challenges.toml",
        challenges_signature=REFERENCE / "challenges.toml.sig",
        measurements_path=REFERENCE / "measurements.toml",
        measurements_signature=REFERENCE / "measurements.toml.sig",
        owner_public=decode_hotkey((REFERENCE / "owner.pubkey").read_text().strip()),
        gateway_public=public_key(bytes([7]) * 32),
        epoch=123,
    )


def test_verifies_existing_independent_owner_signed_trust_documents():
    root = load_trust_root(**arguments())
    assert root.shares == ((b"bounty", 2000), (b"proof", 8000))
    assert root.measurements_digest.hex() == (
        "6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d"
    )


def test_cannot_resign_trust_by_substituting_challenge_key(tmp_path):
    values = arguments()
    path = tmp_path / "tampered.toml"
    original = values["challenges_path"].read_text()
    path.write_text(
        original.replace(
            "743688a1e1b2848b309205706b4dcae54bffe4233a5d7018053471e1dce45c21",
            public_key(bytes([55]) * 32).hex(),
        )
    )
    values["challenges_path"] = path
    with pytest.raises(ProtocolError, match="owner signature"):
        load_trust_root(**values)


def test_local_version_pin_prevents_rollback():
    with pytest.raises(ProtocolError, match="rollback"):
        load_trust_root(**arguments(), minimum_challenges_version=2)


@pytest.mark.parametrize("version,epoch,valid", [(1, 123, False), (2, 122, False), (2, 123, True)])
def test_owner_signed_proportional_profile_requires_version_and_activation(
    tmp_path, version, epoch, valid
):
    values = arguments()
    seed = bytes([31]) * 32
    values.update(owner_public=public_key(seed), epoch=epoch)
    for kind in ("challenges", "measurements"):
        path = tmp_path / f"{kind}.toml"
        source = values[f"{kind}_path"].read_text()
        if kind == "challenges":
            source = (
                source.replace("version = 1", f"version = {version}")
                .replace("introduced_epoch = 0", "introduced_epoch = 123")
                .replace("2000", "3000")
                .replace("8000", "7000")
            )
        path.write_text(source)
        signature = path.with_suffix(".sig")
        signature.write_bytes(sign_raw(seed, TRUST_ROOT_DOMAIN, signing_payload(path, kind)))
        values.update({f"{kind}_path": path, f"{kind}_signature": signature})
    if valid:
        assert load_trust_root(**values).algorithm_version == 2
    else:
        with pytest.raises(ProtocolError, match="version|activation"):
            load_trust_root(**values)
