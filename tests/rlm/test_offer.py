import pytest

from cortex.protocol.crypto import public_key
from cortex.rlm import AgentLimits
from cortex.rlm.offer import InferenceOffer, sign_offer


def offer(**updates):
    seed = bytes([8]) * 32
    document = InferenceOffer(
        **{
            "model": "test/research-model",
            "limits": AgentLimits(wall_seconds=7200.0, tool_timeout_seconds=3600.0),
            "issuer_public_key": public_key(seed).hex(),
            "status": "open",
            "valid_from_unix": 100,
            "valid_until_unix": 4102444800,
            "signature": "0" * 128,
            **updates,
        }
    )
    return sign_offer(document, seed)


def test_offer_signature_binds_model_limits_origin_and_issuer():
    document = offer()
    document.verify(document.commitment(), now=101)

    for field, value in [
        ("model", "other/model"),
        ("limits", AgentLimits()),
        ("issuer_public_key", public_key(bytes([9]) * 32).hex()),
    ]:
        changed = document.model_copy(update={field: value})
        with pytest.raises(ValueError, match="commitment mismatch"):
            changed.verify(document.commitment(), now=101)
        with pytest.raises(ValueError, match="signature invalid"):
            changed.verify(changed.commitment(), now=101)


@pytest.mark.parametrize("moment", [99, 4102444800, 4102444801])
def test_offer_outside_validity_window_cannot_authorize_inference(moment):
    document = offer()

    with pytest.raises(ValueError, match="validity window"):
        document.verify(document.commitment(), now=moment)


def test_closed_offer_and_changed_runtime_cannot_authorize_inference():
    closed = offer(status="closed")
    with pytest.raises(ValueError, match="closed"):
        closed.verify(closed.commitment(), now=101)

    document = offer()
    with pytest.raises(ValueError, match="runtime differs"):
        document.verify_runtime("other/model", document.limits, document.commitment())


def test_pinned_offer_load_rejects_modified_file(tmp_path):
    document = offer()
    path = tmp_path / "offer.json"
    path.write_text(document.model_dump_json())
    assert InferenceOffer.load(path, document.commitment()) == document
    path.write_text(document.model_copy(update={"model": "other/model"}).model_dump_json())

    with pytest.raises(ValueError, match="commitment mismatch"):
        InferenceOffer.load(path, document.commitment())
