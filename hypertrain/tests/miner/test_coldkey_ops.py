"""Operator-side coldkey signing; real CLI, protocol models and sr25519 signatures."""

from __future__ import annotations

import json
import os
from dataclasses import asdict
from types import SimpleNamespace

import httpx
import pytest

from env_pin_fixtures import honest_env
from hypertrain.challenge.admission import AdmissionStatus
from hypertrain.challenge.admission_store import AdmissionRecord
from hypertrain.challenge.trust_v2 import FundedStatus
from hypertrain.ledger.escrow_v2 import genesis_allocation_hash
from hypertrain.miner import cli
from hypertrain.miner.admission import sign_join
from hypertrain.protocol.envelope_v2 import (
    join_signing_message,
    parse_envelope,
    seal,
    verify_envelope,
    verify_join,
)
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair, verify
from hypertrain.protocol.messages_v2 import (
    REQUEST_HORIZON,
    AdmissionPolicyV2,
    ArtifactLimits,
    BondRequest,
    EconomicsPolicyV2,
    EscrowLock,
    HardwareHint,
    JoinRequest,
    RunManifestV2,
)

API = "https://service.test"
HOT_SEED = bytes(range(32, 64))
COLD_SEED = bytes(range(64, 96))
REQUEST_ID = "11" * 32
ADMISSION_ID = "44" * 32
EVENT_HASH = "55" * 32
ORIGINS = ["33" * 32, "22" * 32]
NOW = 37
BOND_TERMS = {"locks": [{"lock_id": "66" * 32, "origin_ids": ORIGINS, "units": 1000}]}
EXPIRY = 5000
UNITS = 1250


@pytest.fixture
def operator(tmp_path, monkeypatch):
    for name in list(os.environ):
        if name.startswith("HYPERTRAIN_MINER_"):
            monkeypatch.delenv(name)
    hot, cold = Keypair(HOT_SEED), Keypair(COLD_SEED)
    coord, other = Keypair(bytes(range(32))), Keypair(bytes(range(96, 128)))
    hotfile, coldfile = tmp_path / "hot.key", tmp_path / "cold.key"
    for path, seed in ((hotfile, HOT_SEED), (coldfile, COLD_SEED)):
        path.write_text(seed.hex())
        path.chmod(0o600)
    admission = AdmissionPolicyV2(
        q_base="0000803e",
        E=4,
        clean_finalizations=12,
        # Only its digest enters the manifest: the faked service sets screen_until itself.
        work_screen_epoch_rounds=1000,
        max_pending=32,
        max_trial_replays=2,
        max_trials_per_beacon=2,
        join_per_beacon=2,
        join_burst=4,
        ip_prefix_per_beacon=32,
        ip_prefix_burst=64,
        challenge_rounds=100,
        artifact_limits=ArtifactLimits(
            max_object_bytes=1_000_000, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
        ),
    )
    policy = EconomicsPolicyV2(
        ledger_mode="test",
        G_max_units=100,
        R_collectible_units=100,
        S_min_units=1000,
        beta_ppm=1_000_000,
        gammaV_units=0,
        s_lower_ppm=1_000_000,
        q_floor=1_000_000,
        genesis_allocation_hash=genesis_allocation_hash(
            [(cold.ss58, 20000, 1), (other.ss58, 10000, 1)]
        ),
        reward_authority=coord.ss58,
        round_reward_units=1_000_000,
        max_total_issuance=20_000_000,
        shadow_bootstrap_rounds=12,
    )
    training = example_manifest().body()
    training["model"]["param_count"] = 4_329_216  # v2 shape count; no tensors needed.
    training["reference_spec"]["layout"].update(n_gpus=2, dp_size=2)
    manifest = RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": training,
            "network": {
                "admission_policy_hash": admission.digest(),
                "economics_policy_hash": policy.digest(),
                "aggregation_policy_hash": REQUEST_ID,
                "dispute_policy_hash": REQUEST_ID,
                "audit_policy_hash": REQUEST_ID,
                "relay_registry_hash": REQUEST_ID,
                "audit_mode": "anchored-full",
                "full_anchor_version": 1,
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )
    config = tmp_path / "miner.toml"
    config.write_text(
        "".join(
            f"{name} = {json.dumps(str(value))}\n"
            for name, value in {
                "api": API,
                "keyfile": hotfile,
                "workdir": tmp_path / "gpu-work",
                "state_source": "unused",
                "image_digest": manifest.training.reference_spec.image_digest,
                "run_id": manifest.run_id(),
                "owner_hotkey": coord.ss58,
                "device": "cpu",
            }.items()
        )
    )
    return SimpleNamespace(
        hot=hot,
        cold=cold,
        other=other,
        coord=coord,
        hotfile=hotfile,
        coldfile=coldfile,
        config=config,
        manifest=manifest,
        policy=policy,
        run_id=manifest.run_id(),
    )


@pytest.fixture
def invoke(capsys, caplog):
    def call(argv, *, parser_error=False):
        try:
            try:
                code = cli.main(argv)
            except SystemExit as error:
                if not parser_error:
                    raise
                code = error.code
        finally:
            captured = capsys.readouterr()
            if any(COLD_SEED.hex() in text for text in (captured.out, captured.err, caplog.text)):
                pytest.fail("coldkey seed exposed in command output", pytrace=False)
        return code, captured.out, captured.err + caplog.text

    return call


def _record(op, owner=None):
    return AdmissionRecord(
        admission_id=ADMISSION_ID,
        hotkey=op.hot.ss58,
        coldkey=owner or op.cold.ss58,
        state="PROBATION",
        clean_count=4,
        canary_blocks=(1,),
        strikes=0,
        pending_dispute=False,
        reason="FULL_ROUND_MATCH",
        policy_hash=op.manifest.network.admission_policy_hash,
        screen_until=EXPIRY,
        challenge_json=None,
    )


def _service(op, monkeypatch, *, post_status=200, owner=None, post_content=None):
    requests = []
    root = f"/v2/runs/{op.run_id}"
    manifest_envelope = seal(op.coord, "RunManifestV2", op.run_id, op.manifest, EXPIRY)

    def route(request):
        requests.append(request)
        assert request.url.host == "service.test"
        if request.method == "GET" and request.url.path == root:
            return httpx.Response(
                200,
                json={
                    "run_id": op.run_id,
                    "manifest_envelope": manifest_envelope,
                    "now_round": NOW,
                },
            )
        if request.method == "GET" and request.url.path == root + "/objects/" + op.policy.digest():
            return httpx.Response(200, content=canonicalize(op.policy.body()))
        if request.method == "GET" and request.url.path == root + "/admission/" + op.hot.ss58:
            locked = 1475 if any(r.method == "POST" for r in requests) else 125
            funding = FundedStatus(
                op.run_id, op.hot.ss58, ADMISSION_ID, "test", locked, 0, EVENT_HASH, True
            )
            return httpx.Response(
                200,
                json=asdict(
                    AdmissionStatus(
                        _record(op, owner),
                        funding,
                        250_000,
                        1_000_000,
                        locked >= 1000,
                        locked < 1000,
                    )
                ),
            )
        if request.method == "POST" and request.url.path == root + f"/join/{ADMISSION_ID}/bond":
            return httpx.Response(200, json=BOND_TERMS)
        if request.method == "POST" and request.url.path in (root + "/join", root + "/escrow/lock"):
            if not 200 <= post_status < 300:
                return httpx.Response(post_status, text="blocked-by-service")
            if post_content is not None:
                return httpx.Response(post_status, content=post_content)
            body = (
                asdict(_record(op))
                if request.url.path.endswith("/join")
                else {"event_hash": EVENT_HASH}
            )
            return httpx.Response(post_status, json=body)
        pytest.fail(f"unexpected HTTP request: {request.method} {request.url.path}")

    real_client = httpx.Client
    transport = httpx.MockTransport(route)
    monkeypatch.setattr(
        httpx, "Client", lambda *args, **kwargs: real_client(*args, transport=transport, **kwargs)
    )
    return requests


def _single_join(op):
    return sign_join(
        op.hot,
        op.cold,
        run_id=op.run_id,
        request_id=REQUEST_ID,
        expires_beacon=EXPIRY,
        policy_hash=op.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cpu", device_count=2, driver="advisory"),
        env=honest_env(op.manifest.training.reference_spec.env.pin()),
    )


def _prepared(op, directory):
    body = {**_single_join(op).body(), "cold_sig": "0" * 128}
    path = directory / "join.json"
    path.write_bytes(canonicalize(body))
    return path


def _prepare_args(op, out):
    return [
        "join-v2",
        "--config",
        str(op.config),
        "--request-id",
        REQUEST_ID,
        "--expiry",
        str(EXPIRY),
        "--coldkey-ss58",
        op.cold.ss58,
        "--prepare",
        str(out),
    ]


def _cosign_args(op, request):
    return [
        "join-v2-cosign",
        "--request",
        str(request),
        "--coldkey",
        str(op.coldfile),
        "--api",
        API,
    ]


def _lock_args(op, origins=ORIGINS):
    return [
        "escrow-lock",
        "--api",
        API,
        "--run-id",
        op.run_id,
        "--coldkey",
        str(op.coldfile),
        "--hotkey-ss58",
        op.hot.ss58,
        "--units",
        str(UNITS),
        *[arg for origin in origins for arg in ("--origin", origin)],
    ]


def test_two_step_join_equals_single_step(operator, tmp_path, monkeypatch, invoke):
    op = operator
    requests = _service(op, monkeypatch, post_status=201)
    artifact = tmp_path / "prepared.json"
    code, out, _ = invoke(_prepare_args(op, artifact))
    assert code == 0
    prepared = JoinRequest.model_validate_json(artifact.read_bytes())
    message = join_signing_message(prepared)
    assert json.loads(out) == {"prepared": str(artifact), "message_sha256": sha256_hex(message)}
    assert prepared.cold_sig == "0" * 128
    assert verify(op.hot.public, message, bytes.fromhex(prepared.hot_sig))
    assert not any(r.method == "POST" for r in requests)

    # The operator has neither miner.toml nor the GPU host's hotkey seed.
    op.config.unlink()
    op.hotfile.unlink()
    code, out, _ = invoke(_cosign_args(op, artifact))
    assert code == 0 and json.loads(out)["admission_id"] == ADMISSION_ID
    posts = [r for r in requests if r.method == "POST"]
    assert len(posts) == 1 and posts[0].url.path == f"/v2/runs/{op.run_id}/join"
    submitted = JoinRequest.model_validate_json(posts[0].content)
    single = _single_join(op)
    assert canonicalize(submitted.model_dump(exclude={"hot_sig", "cold_sig"})) == canonicalize(
        single.model_dump(exclude={"hot_sig", "cold_sig"})
    )
    assert submitted.hot_sig == prepared.hot_sig
    assert verify_join(single, op.run_id, NOW)
    assert verify_join(submitted, op.run_id, NOW)


@pytest.mark.parametrize("fault", ["hot_sig", "coldkey"])
def test_cosign_rejects_bad_hot_signature_or_wrong_coldkey(
    operator, tmp_path, monkeypatch, invoke, fault
):
    op = operator
    requests = _service(op, monkeypatch)
    artifact = _prepared(op, tmp_path)
    body = json.loads(artifact.read_bytes())
    if fault == "hot_sig":
        body["hot_sig"] = "0" * 128
    else:
        body["coldkey"] = op.other.ss58
        unsigned = JoinRequest.model_validate(body)
        body["hot_sig"] = op.hot.sign(join_signing_message(unsigned)).hex()
    artifact.write_bytes(canonicalize(body))
    code, _, _ = invoke(_cosign_args(op, artifact))
    assert code == 2
    assert not any(r.method == "POST" for r in requests)


def test_prepare_refuses_existing_output(operator, tmp_path, monkeypatch, invoke):
    requests = _service(operator, monkeypatch)
    artifact = tmp_path / "prepared.json"
    artifact.write_bytes(b"existing operator data")
    code, _, _ = invoke(_prepare_args(operator, artifact))
    assert code == 2
    assert artifact.read_bytes() == b"existing operator data"
    assert not any(r.method == "POST" for r in requests)


def test_prepare_and_coldkey_are_mutually_exclusive(operator, tmp_path, monkeypatch, capsys):
    requests = _service(operator, monkeypatch)
    with pytest.raises(SystemExit) as error:
        cli.main(
            _prepare_args(operator, tmp_path / "join.json") + ["--coldkey", str(operator.coldfile)]
        )
    captured = capsys.readouterr()
    if COLD_SEED.hex() in captured.out + captured.err:
        pytest.fail("coldkey seed exposed in command output", pytrace=False)
    assert error.value.code == 2 and "not allowed with argument" in captured.err
    assert not any(r.method == "POST" for r in requests)
    assert not (tmp_path / "join.json").exists()


def test_escrow_lock_signs_owner_and_reuses_operation_id_on_reordered_retry(
    operator, monkeypatch, invoke
):
    op = operator
    requests = _service(op, monkeypatch)
    for origins, expiry in ((ORIGINS, None), (list(reversed(ORIGINS)), EXPIRY)):
        args = _lock_args(op, origins)
        if expiry is not None:
            args += ["--expiry", str(expiry)]
        start = len(requests)
        code, out, _ = invoke(args)
        assert code == 0
        assert json.loads(out) == {
            "event_hash": EVENT_HASH,
            "locked_units": 1475,
            "required_units": 1000,
            "eligible": True,
        }
        call = requests[start:]
        index = next(i for i, r in enumerate(call) if r.method == "POST")
        assert any(
            r.method == "GET" and r.url.path.endswith("/admission/" + op.hot.ss58)
            for r in call[index + 1 :]
        )
    posts = [r for r in requests if r.method == "POST"]
    assert len(posts) == 2
    assert canonicalize(parse_envelope(posts[0].content).body) == canonicalize(
        parse_envelope(posts[1].content).body
    )
    expected_id = sha256_hex(
        canonicalize(
            ["ht-escrow-lock-cli", op.run_id, ADMISSION_ID, op.cold.ss58, UNITS, sorted(ORIGINS)]
        )
    )
    for request, expiry in zip(posts, (NOW + 1000, EXPIRY), strict=True):
        assert request.url.path == f"/v2/runs/{op.run_id}/escrow/lock"
        assert verify_envelope(request.content)
        envelope = parse_envelope(request.content)
        assert (envelope.type, envelope.run_id, envelope.signer, envelope.exp_drand) == (
            "EscrowLock",
            op.run_id,
            op.cold.ss58,
            expiry,
        )
        lock = EscrowLock.model_validate(envelope.body)
        assert lock.operation_id == expected_id
        assert lock.owner == op.cold.ss58 and lock.units == UNITS
        assert sorted(lock.origin_ids) == sorted(ORIGINS)
        assert (lock.kind, lock.admission_id, lock.dispute_id) == (
            "LOCK_ADMISSION",
            ADMISSION_ID,
            None,
        )


def test_escrow_lock_refuses_another_admission_owner(operator, monkeypatch, invoke):
    requests = _service(operator, monkeypatch, owner=operator.other.ss58)
    code, _, _ = invoke(_lock_args(operator))
    assert code == 2
    assert not any(r.method == "POST" for r in requests)


def test_escrow_dry_run_emits_only_unsigned_operation(operator, monkeypatch, invoke):
    op = operator
    requests = _service(op, monkeypatch)
    code, out, _ = invoke(_lock_args(op) + ["--dry-run"])
    assert code == 0
    lock = EscrowLock.model_validate_json(out)
    assert (lock.owner, lock.units, lock.admission_id, lock.dispute_id, lock.kind) == (
        op.cold.ss58,
        UNITS,
        ADMISSION_ID,
        None,
        "LOCK_ADMISSION",
    )
    assert sorted(lock.origin_ids) == sorted(ORIGINS)
    assert not any(r.method == "POST" for r in requests)


@pytest.mark.parametrize(
    ("changes", "locked", "expected"),
    [
        ({}, 0, 1000),  # max(1000, 10 * 100, ceil(100 / 1))
        ({"S_min_units": 2000}, 0, 2000),  # Policy floor wins.
        ({"R_collectible_units": 150}, 0, 1500),  # Training floor: 10 * 150.
        ({"G_max_units": 1001, "s_lower_ppm": 333333}, 0, 3004),  # ceil(1001 / .333333)
        ({"G_max_units": 1000, "s_lower_ppm": 400000, "beta_ppm": 1500000}, 0, 1667),
        # Zero denominator: G + already locked + 1 = 100 + 5000 + 1, never fundable.
        ({"s_lower_ppm": 0}, 5000, 5101),
        ({}, 5000, 1000),  # Total requirement, not additional units after existing locks.
    ],
)
def test_required_units_matches_admission_arithmetic(operator, changes, locked, expected):
    from hypertrain.miner.coldkey_ops import required_units

    policy = EconomicsPolicyV2.model_validate({**operator.policy.body(), **changes})
    assert operator.manifest.training.verify.s_min_reward_multiple == 10
    assert required_units(policy, operator.manifest, locked) == expected


@pytest.mark.parametrize("command", ["join", "escrow"])
@pytest.mark.parametrize("status", [202, 302, 422])
def test_http_status_maps_to_exit_code_and_preserves_error_body(
    operator, tmp_path, monkeypatch, invoke, command, status
):
    op = operator
    requests = _service(op, monkeypatch, post_status=status)
    args = _cosign_args(op, _prepared(op, tmp_path)) if command == "join" else _lock_args(op)
    code, out, diagnostic = invoke(args)
    assert code == (0 if 200 <= status < 300 else 1)
    assert len([r for r in requests if r.method == "POST"]) == 1
    if not 200 <= status < 300:
        assert str(status) in out + diagnostic
        assert "blocked-by-service" in out + diagnostic


@pytest.mark.parametrize(("status", "body"), [(204, b""), (200, b"accepted"), (200, b"[]")])
def test_cosign_accepts_all_success_response_bodies(
    operator, tmp_path, monkeypatch, invoke, status, body
):
    requests = _service(operator, monkeypatch, post_status=status, post_content=body)
    code, out, _ = invoke(_cosign_args(operator, _prepared(operator, tmp_path)))
    assert code == 0
    assert json.loads(out) == ([] if body == b"[]" else body.decode())
    assert len([r for r in requests if r.method == "POST"]) == 1


def test_invalid_api_returns_miner_error(operator, tmp_path, monkeypatch, invoke):
    requests = _service(operator, monkeypatch)
    args = _cosign_args(operator, _prepared(operator, tmp_path))
    args[args.index("--api") + 1] = "https://service.test:invalid"
    code, _, diagnostic = invoke(args)
    assert code == 2 and "MinerError" in diagnostic
    assert requests == []


@pytest.mark.parametrize("command", ["prepare", "cosign", "escrow"])
def test_invalid_input_returns_miner_error_without_raw_validation_input(
    operator, tmp_path, monkeypatch, invoke, command
):
    op = operator
    requests = _service(op, monkeypatch)
    marker = "private-validation-input"
    if command == "prepare":
        args = _prepare_args(op, tmp_path / "join.json")
        args[args.index("--request-id") + 1] = marker
    elif command == "cosign":
        artifact = _prepared(op, tmp_path)
        body = json.loads(artifact.read_bytes())
        body["hardware_hint"]["device_count"] = marker
        artifact.write_bytes(canonicalize(body))
        args = _cosign_args(op, artifact)
    else:
        args = _lock_args(op, [marker])
    code, out, diagnostic = invoke(args)
    assert code == 2
    assert "MinerError" in diagnostic
    assert marker not in out + diagnostic
    assert not any(r.method == "POST" for r in requests)


def _bond_args(op):
    return [
        "escrow-bond",
        "--api",
        API,
        "--run-id",
        op.run_id,
        "--coldkey",
        str(op.coldfile),
        "--hotkey-ss58",
        op.hot.ss58,
    ]


def test_escrow_bond_signs_a_bond_request_with_the_coldkey_and_prints_the_terms(
    operator, monkeypatch, invoke
):
    op = operator
    requests = _service(op, monkeypatch)

    code, out, _ = invoke(_bond_args(op))

    assert code == 0 and json.loads(out) == BOND_TERMS
    (post,) = [r for r in requests if r.method == "POST"]
    assert post.url.path == f"/v2/runs/{op.run_id}/join/{ADMISSION_ID}/bond"
    assert verify_envelope(post.content)
    envelope = parse_envelope(post.content)
    assert (envelope.type, envelope.run_id, envelope.signer, envelope.exp_drand) == (
        "BondRequest",
        op.run_id,
        op.cold.ss58,  # the coldkey owns the lock, not the hotkey
        NOW + REQUEST_HORIZON,
    )
    assert BondRequest.model_validate(envelope.body).admission_id == ADMISSION_ID


def test_escrow_bond_refuses_another_admission_owner_without_posting(operator, monkeypatch, invoke):
    requests = _service(operator, monkeypatch, owner=operator.other.ss58)

    code, _, _ = invoke(_bond_args(operator))

    assert code == 2
    assert not any(r.method == "POST" for r in requests)
