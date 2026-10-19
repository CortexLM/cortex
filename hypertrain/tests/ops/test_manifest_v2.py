from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import httpx
import pytest
from fastapi.testclient import TestClient
from tests.challenge.conftest import ADMIN, Master, beacon_payload, bearer, verify_fixture

from hypertrain.challenge.app import Config, create_app
from hypertrain.ops.manifest_v2 import main
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import RunManifestV2
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params


@pytest.fixture
def build_args(tmp_path: Path) -> list[str]:
    inputs = tmp_path / "inputs"
    assert main(["example", "--out", str(inputs)]) == 0
    owner_seed = tmp_path / "owner.seed"
    owner_seed.write_bytes(b"\x77" * 32)
    owner_seed.chmod(0o600)
    return [
        "build",
        "--training",
        str(inputs / "training.json"),
        "--policies",
        str(inputs / "policies.json"),
        "--relays",
        str(inputs / "relays.json"),
        "--owner-seed",
        str(owner_seed),
        "--exp-round",
        "1000000",
        "--state-dir",
        str(tmp_path / "state"),
        "--out",
        str(tmp_path / "output"),
    ]


def test_build_then_service_accepts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    build_args: list[str],
    capsys: pytest.CaptureFixture[str],
) -> None:
    output = tmp_path / "output"
    state = tmp_path / "state"
    training_path = tmp_path / "inputs" / "training.json"
    training = json.loads(training_path.read_text())
    coord_seed = bytes(range(32))
    training["coord_pubkey"] = Keypair(coord_seed).ss58
    training_path.write_text(json.dumps(training))
    policies_path = tmp_path / "inputs" / "policies.json"
    policies = json.loads(policies_path.read_text())
    policies["economics"]["reward_authority"] = training["coord_pubkey"]
    policies_path.write_text(json.dumps(policies))
    assert main(build_args) == 0
    index = json.loads((output / "index.json").read_text())
    manifest = RunManifestV2.model_validate_json((output / "manifest.json").read_bytes())
    captured = capsys.readouterr()
    envelope = json.loads((output / "manifest-envelope.json").read_text())
    assert captured.err == ""
    assert envelope["sig"] not in captured.out
    assert (b"\x77" * 32).hex() not in captured.out
    assert (b"\x77" * 32).decode() not in captured.out
    assert manifest.run_id() == index["run_id"]
    assert manifest.training.init_state_hash == state_hash(
        init_params(TrainConfig.from_manifest_v2(manifest).model)
    )
    assert index["owner_ss58"] == Keypair(b"\x77" * 32).ss58
    assert set(index["objects"]) == {
        "admission",
        "economics",
        "aggregation",
        "audit",
        "dispute",
        "relay_registry",
    }
    expected_files = {
        "manifest.json",
        "manifest-envelope.json",
        "policy-admission.json",
        "policy-economics.json",
        "policy-aggregation.json",
        "policy-audit.json",
        "policy-dispute.json",
        "relay-registry.json",
    }
    assert set(index["files"]) == expected_files
    assert {p.name for p in output.iterdir()} == expected_files | {"index.json"}
    for name, digest in index["files"].items():
        assert sha256_hex((output / name).read_bytes()) == digest
    for name, digest in index["objects"].items():
        path = state / "objects" / digest[:2] / digest
        assert sha256_hex(path.read_bytes()) == digest
        field = "relay_registry_hash" if name == "relay_registry" else f"{name}_policy_hash"
        assert getattr(manifest.network, field) == digest
    admin_file = tmp_path / "admin.token"
    admin_file.write_text(ADMIN)
    coord_file = tmp_path / "coord.key"
    coord_file.write_bytes(coord_seed)
    for name, value in {
        "CHALLENGE_STATE_DIR": str(state),
        "CHALLENGE_ADMIN_TOKEN_FILE": str(admin_file),
        "HYPERTRAIN_COORD_KEY_FILE": str(coord_file),
        "HYPERTRAIN_OWNER_HOTKEY": index["owner_ss58"],
    }.items():
        monkeypatch.setenv(name, value)
    config = Config.from_env()
    assert config.owner_hotkey == index["owner_ss58"]
    app = create_app(
        config,
        clock=lambda: float(manifest.training.beacon.genesis_time),
        transport=httpx.MockTransport(Master().handler),
        verify_beacon=verify_fixture,
    )
    with TestClient(app) as client:
        pushed = client.post("/v1/admin/beacon", json=beacon_payload(1), headers=bearer(ADMIN))
        assert pushed.status_code == 200, pushed.text
        response = client.post(
            "/v2/admin/runs",
            content=(output / "manifest-envelope.json").read_bytes(),
            headers={**bearer(ADMIN), "content-type": "application/json"},
        )
        assert response.status_code == 201, response.text
        assert response.json()["run_id"] == index["run_id"]


@pytest.mark.parametrize(
    "fault,message",
    [
        ("audit", "audit policy budget must equal 2H"),
        ("auditors", "two distinct audit reference identities required"),
        ("derived", "v2 derived-state influence requires authenticated v0 integration"),
        ("permissions", "keyfile must be mode 0600 and owned by uid"),
        ("output", "--out must not exist or be empty"),
        ("init_hash", "init_state_hash does not match computed initial state"),
        ("policy_schema", "q_base"),
        ("registry_schema", "registry_version"),
        ("multiline_field", "extra_forbidden"),
        ("expiry", "--exp-round must be positive"),
        ("authority", "PINNED_AUTHORITIES"),
        ("reward_budget", "MANIFEST_REWARD_BUDGET"),
    ],
)
def test_build_refuses_invalid_inputs(
    tmp_path: Path,
    build_args: list[str],
    capsys: pytest.CaptureFixture[str],
    fault: str,
    message: str,
) -> None:
    inputs = tmp_path / "inputs"
    training_path = inputs / "training.json"
    training = json.loads(training_path.read_text())
    if fault in {"audit", "policy_schema", "authority", "reward_budget", "multiline_field"}:
        path = inputs / "policies.json"
        policies = json.loads(path.read_text())
        if fault == "audit":
            policies["audit"]["max_steps_per_attempt"] = 3
        elif fault == "authority":
            policies["economics"]["reward_authority"] = training["auditors"][0]
        elif fault == "reward_budget":
            policies["economics"]["round_reward_units"] = 999999
        elif fault == "multiline_field":
            policies["admission"]["unexpected\nfield"] = True
        else:
            policies["admission"]["q_base"] = "00000000"
        path.write_text(json.dumps(policies))
    elif fault == "registry_schema":
        path = inputs / "relays.json"
        registry = json.loads(path.read_text())
        registry["registry_version"] = 2
        path.write_text(json.dumps(registry))
    elif fault == "expiry":
        build_args[build_args.index("--exp-round") + 1] = "0"
    elif fault == "auditors":
        training["auditors"] = training["auditors"][:1]
    elif fault == "derived":
        training["inner"]["state_policy"] = "derived"
    elif fault == "permissions":
        (tmp_path / "owner.seed").chmod(0o644)
    elif fault == "output":
        output = tmp_path / "output"
        output.mkdir()
        (output / "keep").write_text("untouched")
    elif fault == "init_hash":
        training["init_state_hash"] = "ff" * 32
    training_path.write_text(json.dumps(training))
    capsys.readouterr()
    assert main(build_args) == 2
    captured = capsys.readouterr()
    assert captured.out == ""
    assert len(captured.err.splitlines()) == 1
    assert message in captured.err
    assert not (tmp_path / "state" / "objects").exists()
    assert not (tmp_path / "output" / "manifest-envelope.json").exists()
    if fault == "output":
        assert (tmp_path / "output" / "keep").read_text() == "untouched"


def test_build_without_store_accepts_hex_seed_and_matching_init_hash(
    tmp_path: Path, build_args: list[str]
) -> None:
    assert main(build_args) == 0
    first = json.loads((tmp_path / "output" / "manifest.json").read_text())
    training = tmp_path / "inputs" / "training.json"
    training.write_text(json.dumps(first["training"]))
    (tmp_path / "owner.seed").write_text((b"\x77" * 32).hex())
    output = tmp_path / "second-output"
    output.mkdir()
    no_store = build_args[:]
    index = no_store.index("--state-dir")
    del no_store[index : index + 2]
    no_store[no_store.index("--out") + 1] = str(output)
    assert main(no_store) == 0
    second = json.loads((output / "index.json").read_text())
    assert second["run_id"] == RunManifestV2.model_validate(first).run_id()
    assert json.loads((output / "manifest.json").read_text()) == first


def test_example_and_help_do_not_import_torch(tmp_path: Path) -> None:
    code = """
import sys
class NoTorch:
    def find_spec(self, fullname, path=None, target=None):
        if fullname == 'torch' or fullname.startswith('torch.'):
            raise RuntimeError('torch must remain optional')
sys.meta_path.insert(0, NoTorch())
from hypertrain.ops.manifest_v2 import main
raise SystemExit(main(sys.argv[1:]))
"""
    example = subprocess.run(
        [sys.executable, "-c", code, "example", "--out", str(tmp_path / "inputs")],
        capture_output=True,
        text=True,
        check=False,
    )
    assert example.returncode == 0, example.stderr
    training = json.loads((tmp_path / "inputs" / "training.json").read_text())
    assert "init_state_hash" not in training
    assert len(set(training["auditors"])) == 2
    help_result = subprocess.run(
        [sys.executable, "-c", code, "--help"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert help_result.returncode == 0, help_result.stderr
    assert "build" in help_result.stdout
    assert "example" in help_result.stdout


def test_build_refuses_corrupt_existing_object(
    tmp_path: Path, build_args: list[str], capsys: pytest.CaptureFixture[str]
) -> None:
    from hypertrain.protocol.messages_v2 import AdmissionPolicyV2

    policies = json.loads((tmp_path / "inputs" / "policies.json").read_text())
    digest = AdmissionPolicyV2.model_validate(policies["admission"]).digest()
    path = tmp_path / "state" / "objects" / digest[:2] / digest
    path.parent.mkdir(parents=True)
    path.write_bytes(b"corrupt")
    capsys.readouterr()
    assert main(build_args) == 2
    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == "error: object store contains corrupt content\n"
    assert path.read_bytes() == b"corrupt"
    assert not (tmp_path / "output" / "manifest-envelope.json").exists()
