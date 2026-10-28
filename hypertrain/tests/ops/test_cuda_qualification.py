"""TEST-only CUDA backend authority: the record the store reads, and when the tool refuses."""

from __future__ import annotations

import json
import shutil
import sqlite3
from pathlib import Path
from typing import Any

import pytest
from scripts.ops.s100_test import cuda_qualification as qualification

from hypertrain.beacon import parse_round
from hypertrain.challenge.store import ChallengeStore
from hypertrain.ledger import Params
from hypertrain.ops.manifest_v2 import main as manifest_main
from hypertrain.protocol.env_pins import class_pin
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import QUICKNET_GENESIS
from hypertrain.protocol.messages_v2 import RunManifestV2

COORD = Keypair(bytes(range(32)))
OWNER = Keypair(b"\x77" * 32)


def build_stage(root: Path) -> tuple[Path, Path]:
    """A real `manifest_v2 build` output directory, as build_test_manifest.sh leaves it."""
    inputs = root / "inputs"
    assert manifest_main(["example", "--out", str(inputs)]) == 0
    training = json.loads((inputs / "training.json").read_text())
    training["coord_pubkey"] = COORD.ss58
    training["reference_spec"]["env"]["pinned"] = class_pin("avx2", "sm_120")  # a CUDA run
    (inputs / "training.json").write_text(json.dumps(training))
    policies = json.loads((inputs / "policies.json").read_text())
    policies["economics"]["reward_authority"] = COORD.ss58
    (inputs / "policies.json").write_text(json.dumps(policies))
    seed = root / "owner.seed"
    seed.write_bytes(b"\x77" * 32)
    seed.chmod(0o600)
    built = root / "stage"
    code = manifest_main(
        [
            *["build", "--training", str(inputs / "training.json")],
            *["--policies", str(inputs / "policies.json")],
            *["--relays", str(inputs / "relays.json"), "--owner-seed", str(seed)],
            *["--exp-round", "1000000", "--state-dir", str(root / "build-state")],
            *["--out", str(built)],
        ]
    )
    assert code == 0
    return built, seed


def challenge(tmp_path: Path, name: str = "data") -> tuple[ChallengeStore, Path]:
    data = tmp_path / name
    params = Params("hypertrain", QUICKNET_GENESIS, 300, 1, 10)
    from hypertrain.data.store import LocalFSStore

    store = ChallengeStore(
        data, params, COORD, OWNER.ss58, parse_round, LocalFSStore(data / "objects")
    )
    return store, data


@pytest.fixture
def stage(tmp_path: Path) -> tuple[Path, Path, RunManifestV2]:
    built, seed = build_stage(tmp_path)
    manifest = RunManifestV2.model_validate_json((built / "manifest.json").read_bytes())
    return built, seed, manifest


def test_installed_record_is_what_makes_the_store_choose_cuda(
    tmp_path: Path, stage: tuple[Path, Path, RunManifestV2]
) -> None:
    built, seed, manifest = stage
    economics = (built / "policy-economics.json").read_bytes()
    bare, _ = challenge(tmp_path, "bare")  # same run, no record: the test ledger freezes cpu
    bare.objects.put(economics)
    assert bare._backend_v2(manifest) == "cpu"

    store, data = challenge(tmp_path)
    store.objects.put(economics)
    built_record = qualification.build(built, seed)
    done = qualification.install(built, data, OWNER.ss58)
    assert done["authority_hash"] == built_record["authority_hash"] and done["test_only"] is True
    assert store._backend_v2(manifest) == "cuda"
    frozen = store._record_v2(manifest.run_id(), "execution-backend", "run")
    assert frozen == {"backend": "cuda", "authority_hash": built_record["authority_hash"]}
    record = store._record_v2(
        manifest.run_id(), "qualification", manifest.training.reference_spec.image_digest
    )
    assert record["harness"] == qualification.MARKER and "receipt" in record


def test_build_refuses_a_production_ledger(
    stage: tuple[Path, Path, RunManifestV2], monkeypatch: pytest.MonkeyPatch
) -> None:
    built, seed, manifest = stage

    class Production:
        ledger_mode = "production"

        @staticmethod
        def digest() -> str:
            return manifest.network.economics_policy_hash

        @classmethod
        def model_validate_json(cls, raw: bytes) -> type[Production]:
            return cls

    monkeypatch.setattr(qualification, "EconomicsPolicyV2", Production)
    with pytest.raises(SystemExit, match="not a TEST run: ledger_mode is production"):
        qualification.build(built, seed)
    assert not (built / qualification.FILE).exists()


def test_build_refuses_an_economics_policy_that_is_not_the_manifests(
    stage: tuple[Path, Path, RunManifestV2],
) -> None:
    built, seed, _ = stage
    policy = json.loads((built / "policy-economics.json").read_bytes())
    policy["R_collectible_units"] = policy["R_collectible_units"] + 1
    (built / "policy-economics.json").write_text(json.dumps(policy))
    with pytest.raises(SystemExit, match="not the manifest's economics policy"):
        qualification.build(built, seed)


def test_build_refuses_a_seed_that_did_not_sign_the_manifest(
    tmp_path: Path, stage: tuple[Path, Path, RunManifestV2]
) -> None:
    built, _, _ = stage
    other = tmp_path / "other.seed"
    other.write_bytes(b"\x55" * 32)
    other.chmod(0o600)
    with pytest.raises(SystemExit, match="not signed by this owner seed"):
        qualification.build(built, other)


def test_install_refuses_a_challenge_with_another_owner(
    tmp_path: Path, stage: tuple[Path, Path, RunManifestV2]
) -> None:
    built, seed, _ = stage
    qualification.build(built, seed)
    _, data = challenge(tmp_path)
    with pytest.raises(SystemExit, match="allow"):
        qualification.install(built, data, Keypair(b"\x66" * 32).ss58)
    assert not has_record(data)


def has_record(data: Path) -> bool:
    db = sqlite3.connect(data / "challenge.db")
    try:
        return (
            db.execute("SELECT 1 FROM records_v2 WHERE kind='qualification'").fetchone() is not None
        )
    finally:
        db.close()


@pytest.mark.parametrize("field", ["backend", "layout_hash", "reference_hash", "run_id", "harness"])
def test_install_refuses_an_altered_file(
    tmp_path: Path, stage: tuple[Path, Path, RunManifestV2], field: str
) -> None:
    built, seed, _ = stage
    qualification.build(built, seed)
    path = built / qualification.FILE
    record: dict[str, Any] = json.loads(path.read_bytes())
    record["evidence"][field] = "cpu" if field == "backend" else "x" * 64
    path.write_text(json.dumps(record))
    _, data = challenge(tmp_path)
    with pytest.raises(SystemExit, match="allow"):
        qualification.install(built, data, OWNER.ss58)
    assert not has_record(data)


def test_install_refuses_once_the_backend_is_frozen_or_the_run_exists(
    tmp_path: Path, stage: tuple[Path, Path, RunManifestV2]
) -> None:
    built, seed, manifest = stage
    qualification.build(built, seed)
    store, data = challenge(tmp_path)
    store.objects.put((built / "policy-economics.json").read_bytes())
    assert store._backend_v2(manifest) == "cpu"  # a run created first: frozen for good
    with pytest.raises(SystemExit, match="already frozen"):
        qualification.install(built, data, OWNER.ss58)
    assert not has_record(data)
    child = tmp_path / "other" / "runs" / manifest.run_id()
    child.mkdir(parents=True)
    challenge(tmp_path, "other")
    with pytest.raises(SystemExit, match="already exists"):
        qualification.install(built, tmp_path / "other", OWNER.ss58)
    shutil.rmtree(child)


def test_install_is_repeatable_and_never_replaces_a_record(
    tmp_path: Path, stage: tuple[Path, Path, RunManifestV2]
) -> None:
    built, seed, _ = stage
    qualification.build(built, seed)
    _, data = challenge(tmp_path)
    first = qualification.install(built, data, OWNER.ss58)
    assert qualification.install(built, data, OWNER.ss58) == first
    first_file = (built / qualification.FILE).read_bytes()
    qualification.build(built, seed)  # sr25519 signatures differ: another file, same evidence
    assert (built / qualification.FILE).read_bytes() != first_file
    with pytest.raises(SystemExit, match="different qualification record"):
        qualification.install(built, data, OWNER.ss58)
