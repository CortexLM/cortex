import copy
from typing import Any

import pytest

from hypertrain.protocol.envelope import (
    ExpiredError,
    Intake,
    MalformedEnvelope,
    ReplayError,
    RunMismatchError,
    SignatureError,
    UnauthorizedSigner,
    seal,
    signing_message,
    verify_envelope,
)
from hypertrain.protocol.keys import Keypair, decode_hotkey

RUN = "ab" * 32
OTHER_RUN = "cd" * 32
MINER = Keypair(bytes([7]) * 32)
COORD = Keypair(bytes([9]) * 32)


def commit_body(w: int = 3) -> dict[str, Any]:
    return {
        "w": w,
        "hotkey": MINER.ss58,
        "leaf_scheme": "ht-leaf-v1",
        "n_leaves": 7,
        "leaves_root": "11" * 32,
        "metrics_root": "22" * 32,
        "final_theta_hash": "33" * 32,
        "ef_in_hash": "44" * 32,
        "ef_out_hash": "55" * 32,
        "delta_hash": "66" * 32,
        "delta_bytes": 1024,
        "tokens": 7680,
    }


def sealed(w: int = 3, exp: int = 100) -> dict[str, Any]:
    return seal(MINER, "Commit", RUN, commit_body(w), exp)


def test_roundtrip_and_ss58() -> None:
    env = sealed()
    assert verify_envelope(env)
    assert decode_hotkey(env["signer"]) == MINER.public
    model = Intake(RUN).accept(env, now_round=50)
    assert model.model_dump(mode="json") == commit_body()


def test_preimage_layout() -> None:
    msg = signing_message("Commit", commit_body(), 100, RUN)
    assert msg.startswith(b"hypertrain/1|Commit|")
    assert msg.split(b"|")[3:] == [b"100", RUN.encode()]


def test_flipped_body_byte_rejected_with_signature_error() -> None:
    env = sealed()
    tampered = copy.deepcopy(env)
    h = tampered["body"]["leaves_root"]
    tampered["body"]["leaves_root"] = ("0" if h[0] != "0" else "1") + h[1:]
    assert not verify_envelope(tampered)
    with pytest.raises(SignatureError):
        Intake(RUN).accept(tampered, now_round=50)


def test_wrong_type_rejected() -> None:
    env = sealed()
    env["type"] = "Accept"
    assert not verify_envelope(env)
    with pytest.raises(SignatureError):
        Intake(RUN).accept(env, now_round=50)


def test_expired_rejected() -> None:
    env = sealed(exp=100)
    Intake(RUN).accept(env, now_round=100)
    with pytest.raises(ExpiredError):
        Intake(RUN).accept(env, now_round=101)


def test_exp_drand_is_signed() -> None:
    env = sealed(exp=100)
    env["exp_drand"] = 10_000
    with pytest.raises(SignatureError):
        Intake(RUN).accept(env, now_round=50)


def test_replay_rejected_same_key() -> None:
    intake = Intake(RUN)
    intake.accept(sealed(w=3), now_round=50)
    with pytest.raises(ReplayError):
        intake.accept(sealed(w=3), now_round=50)
    other = commit_body(3)
    other["tokens"] = 1
    with pytest.raises(ReplayError):
        intake.accept(seal(MINER, "Commit", RUN, other, 100), now_round=50)
    intake.accept(sealed(w=4), now_round=50)


def test_wrong_run_id_rejected() -> None:
    env = sealed()
    with pytest.raises(RunMismatchError):
        Intake(OTHER_RUN).accept(env, now_round=50)
    moved = dict(env, run_id=OTHER_RUN)
    assert not verify_envelope(moved)
    with pytest.raises(SignatureError):
        Intake(OTHER_RUN).accept(moved, now_round=50)


def test_signer_swap_rejected() -> None:
    env = dict(sealed(), signer=COORD.ss58)
    with pytest.raises(SignatureError):
        Intake(RUN).accept(env, now_round=50)


def test_unauthorized_signer() -> None:
    intake = Intake(RUN, allowed_signers={"Commit": lambda s: s == COORD.ss58})
    with pytest.raises(UnauthorizedSigner):
        intake.accept(sealed(), now_round=50)


@pytest.mark.parametrize(
    "mutate",
    [
        lambda e: e.pop("sig"),
        lambda e: e.update(v="ht/2"),
        lambda e: e.update(type="Nope"),
        lambda e: e.update(exp_drand="100"),
        lambda e: e.update(exp_drand=True),
        lambda e: e.update(sig="zz"),
        lambda e: e.update(extra=1),
        lambda e: e.update(run_id=RUN.upper()),
        lambda e: e.update(body=[1]),
    ],
)
def test_malformed_envelopes(mutate: Any) -> None:
    env = sealed()
    mutate(env)
    assert not verify_envelope(env)
    with pytest.raises(MalformedEnvelope):
        Intake(RUN).accept(env, now_round=50)


def test_signed_but_invalid_body_rejected() -> None:
    bad = commit_body()
    bad["leaf_scheme"] = "ht-leaf-v1"
    bad["tokens"] = -1
    with pytest.raises(ValueError):
        seal(MINER, "Commit", RUN, bad, 100)
    raw = dict(sealed())
    raw["body"] = bad
    raw["sig"] = MINER.sign(signing_message("Commit", bad, 100, RUN)).hex()
    with pytest.raises(MalformedEnvelope):
        Intake(RUN).accept(raw, now_round=50)


def test_float_in_body_cannot_be_signed() -> None:
    body = commit_body()
    body["tokens"] = 1.0
    with pytest.raises(ValueError):
        seal(MINER, "Commit", RUN, body, 100)


def test_bool_cannot_alias_int_round() -> None:
    intake = Intake(RUN)
    intake.accept(sealed(w=1), now_round=50)
    body = commit_body(1)
    body["w"] = True
    forged = dict(
        sealed(w=1), body=body, sig=MINER.sign(signing_message("Commit", body, 100, RUN)).hex()
    )
    assert verify_envelope(forged)
    with pytest.raises(MalformedEnvelope):
        intake.accept(forged, now_round=50)
    with pytest.raises(MalformedEnvelope):
        Intake(RUN).accept(forged, now_round=50)


def test_replay_key_from_validated_model() -> None:
    intake = Intake(RUN)
    intake.accept(sealed(w=1), now_round=50)
    body = commit_body(1)
    body["tokens"] = 2
    with pytest.raises(ReplayError):
        intake.accept(seal(MINER, "Commit", RUN, body, 100), now_round=50)


def test_body_hotkey_must_equal_signer() -> None:
    env = seal(COORD, "Commit", RUN, commit_body(), 100)
    assert verify_envelope(env)
    with pytest.raises(UnauthorizedSigner):
        Intake(RUN).accept(env, now_round=50)


def test_one_dispute_decision_per_verdict() -> None:
    intake = Intake(RUN)
    d = {"hotkey": MINER.ss58, "verdict_hash": "77" * 32, "action": "accept"}
    intake.accept(seal(MINER, "Dispute", RUN, d, 100), now_round=50)
    with pytest.raises(ReplayError):
        intake.accept(seal(MINER, "Dispute", RUN, {**d, "action": "contest"}, 100), now_round=50)
    intake.accept(seal(MINER, "Dispute", RUN, {**d, "verdict_hash": "78" * 32}, 100), now_round=50)
