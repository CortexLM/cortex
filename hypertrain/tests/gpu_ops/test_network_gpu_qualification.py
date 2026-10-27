"""Lightweight boundary/custody tests; no CUDA or training qualification."""

from __future__ import annotations

import functools
import importlib.util
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest
from pydantic import ValidationError

from hypertrain.gpu_ops import host_attest
from hypertrain.gpu_ops import network_qualification as driver
from hypertrain.gpu_ops.journal import fsha
from hypertrain.gpu_ops.launcher import Reject
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import IslandJobV1, RunManifestV2
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import param_shapes

TREE = Path(__file__).parents[2]
# Compute capabilities (major, minor) of the measured device classes.
RTX_5090, B300 = (12, 0), (10, 3)


def results(tmp_path: Path, ranks: int = 2, H: int = 30, J: int = 5) -> list[Path]:  # noqa: N803
    """Two synthetic CUDA-labelled JSON sets exercise parsing, never execute GPUs."""
    paths = []
    for i in range(2):
        directory = tmp_path / f"host-{i}"
        hashes = []
        for index in range(2):
            root = directory / f"round-{index}/published"
            files = {}
            for rel in driver.round_files(ranks, H, J):
                path = root / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(bytes([index]))
                files[rel] = fsha(path)
            hashes.append(files)
        result = driver.Result(
            status="CUDA_ROUNDS_CAPTURED_NOT_WORKLOAD_PASS",
            config_sha256="11" * 32,
            profile_sha256="22" * 32,
            sources={"src/x.py": "33" * 32},
            environment=driver.Environment(
                backend="cuda",
                torch="2.14.0+cu130",
                cuda="13.0",
                python="/opt/hypertrain/venv/bin/python",
                torch_path="/venv/torch/__init__.py",
                drivers=["595.84"] * ranks,
                sm_counts=[170] * ranks,
                capabilities=[RTX_5090] * ranks,
                image_digest="sha256:" + "44" * 32,
                gpu_uuids=host_gpus(i)[:ranks],
            ),
            instance_id=i + 1,
            machine_id=i + 10,
            role=f"h{i}",
            rounds=hashes,
            elapsed_seconds=1,
            workload_allowed=False,
        )
        path = directory / "result.json"
        path.write_text(result.model_dump_json())
        attest(path)
        paths.append(path)
    return paths


def host_gpus(host: int, count: int = 8) -> list[str]:
    """Every GPU of one box, in nvidia-smi form; two boxes never share one."""
    return ["GPU-" + str(uuid.UUID(int=(host + 1) * 1000 + d)) for d in range(count)]


def boot_id(host: int) -> str:
    return str(uuid.UUID(int=0xB007_0000 + host))


@functools.cache
def host_key(index: int) -> tuple[bytes, str]:
    """A throw-away SSH host key: private file bytes and the `ssh-ed25519 <base64>` form."""
    with tempfile.TemporaryDirectory(prefix="ht-hostkey-") as scratch:
        private = Path(scratch, "key")
        subprocess.run(
            ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(private)],
            check=True,
            capture_output=True,
        )
        public = " ".join(private.with_suffix(".pub").read_text().split()[:2])
        return private.read_bytes(), public


def key_file(path: Path, index: int) -> Path:
    """Host `index`'s private key beside the result directories, and its known_hosts file."""
    private = path.parent.parent / "keys" / f"host-{index}"
    if not private.exists():
        private.parent.mkdir(exist_ok=True)
        material, public = host_key(index)
        private.write_bytes(material)
        private.chmod(0o600)
        private.with_suffix(".known_hosts").write_text(f"host-{index} {public}\n")
    return private


def attest(path: Path, signer: int | None = None, **override: Any) -> None:
    """Sign, as an SSH host key, what the host would say about the result.json now on disk."""
    index = host_index(path)
    data = json.loads(path.read_bytes())  # Raw: a host also signs results the engine refuses.
    statement = host_attest.HostStatement.model_validate(
        {
            "version": 1,
            "config_sha256": data["config_sha256"],
            "result_sha256": fsha(path),
            "image_digest": data["environment"]["image_digest"],
            "gpu_uuids": sorted(host_gpus(index)),
            "drivers": sorted(set(data["environment"]["drivers"])),
            "boot_id": boot_id(index),
        }
        | override
    )
    raw = host_attest.statement_bytes(statement)
    key = key_file(path, index if signer is None else signer)
    signature = subprocess.run(
        ["ssh-keygen", "-Y", "sign", "-f", str(key), "-n", host_attest.NAMESPACE],
        input=raw,
        check=True,
        capture_output=True,
    ).stdout
    (path.parent / host_attest.ATTESTATION).write_bytes(raw)
    (path.parent / host_attest.SIGNATURE).write_bytes(signature)


def host_index(path: Path) -> int:
    return int(path.parent.name.removeprefix("host-"))


def known_hosts_of(path: Path) -> Path:
    return key_file(path, host_index(path)).with_suffix(".known_hosts")


def pins(paths: list[Path]) -> list[str]:
    """The host keys the operator pinned: one `ssh-ed25519` key per host's known_hosts file."""
    return [host_attest.pinned_key(known_hosts_of(p).read_bytes()) for p in paths]


def typed_fingerprint(known_hosts: Path) -> str:
    """What a provider console prints for the host key: ssh-keygen's own fingerprint, not ours."""
    listing = subprocess.run(
        ["ssh-keygen", "-l", "-f", str(known_hosts)], check=True, capture_output=True, text=True
    ).stdout
    return listing.split()[1]


def typed_pins(paths: list[Path]) -> list[driver.HostPin]:
    """The pins the owner types: each host's known_hosts file and its console fingerprint."""
    return [driver.HostPin(known_hosts_of(p), typed_fingerprint(known_hosts_of(p))) for p in paths]


def evidence_for(
    manifest: RunManifestV2,
    configs: tuple[Path, ...],
    results: tuple[Path, ...],
    *,
    label_only: bool = False,
) -> dict[str, Any]:
    """The owner-signed qualification evidence, written out independently of the store's builder
    so that drift between the two is a failing digest, not a shared mistake."""
    reference = manifest.training.reference_spec
    evidence: dict[str, Any] = {
        "run_id": manifest.run_id(),
        "backend": "cuda",
        "reference_hash": sha256_hex(canonicalize(reference.model_dump(mode="json"))),
        "layout_hash": reference.layout.model_dump_json(),
        "configs": [fsha(p) for p in configs],
        "results": [fsha(p) for p in results],
        "summaries": [
            fsha(path.parent / f"round-{round_index}/published/rank-{rank}/summary.json")
            for path in results
            for round_index in range(2)
            for rank in range(reference.layout.n_gpus)
        ],
    }
    if label_only:
        evidence["qualification_mode"] = "label-only"
    else:
        evidence["host_attestation"] = {
            "host_keys": [typed_fingerprint(known_hosts_of(p)) for p in results],
            "statements": [fsha(p.parent / host_attest.ATTESTATION) for p in results],
            "signatures": [fsha(p.parent / host_attest.SIGNATURE) for p in results],
        }
    return evidence


def manifest(ranks: int = 2, H: int = 30, J: int = 5) -> RunManifestV2:  # noqa: N803
    """Caller-validated authority for compare(); results never supply their own shape."""
    body = example_manifest().body()
    shapes = param_shapes(TrainConfig.from_manifest(example_manifest()).model)
    body["model"]["param_count"] = sum(math.prod(s) for s in shapes.values())
    body["inner"].update(H=H, J=J)
    body["reference_spec"].update(image_digest="sha256:" + "44" * 32)
    body["reference_spec"]["layout"].update(n_gpus=ranks, dp_size=ranks, ep_size=1)
    network = {
        **{
            k: "11" * 32
            for k in (
                "admission_policy_hash",
                "economics_policy_hash",
                "aggregation_policy_hash",
                "dispute_policy_hash",
                "audit_policy_hash",
                "relay_registry_hash",
            )
        },
        "audit_mode": "anchored-full",
        "full_anchor_version": 1,
        "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
    }
    return RunManifestV2.model_validate(
        {"manifest_version": 2, "training": body, "network": network}
    )


def test_bitwise_comparison_preserves_workload_gate(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    # When
    result = driver.compare(paths, manifest(), host_keys=pins(paths))
    # Then
    assert result == {"two_host_bitwise_match": True, "workload_allowed": False}


def test_tampered_artifact_rejects_claimed_matching_hashes(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    (paths[1].parent / "round-1/published/rank-0/state.safetensors").write_bytes(b"tampered")
    # When / Then
    with pytest.raises(Reject, match="custody_changed"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


def test_same_machine_rejects_two_host_claim(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    data = json.loads(paths[1].read_bytes())
    data["machine_id"] = 10
    paths[1].write_text(json.dumps(data))
    # When / Then
    with pytest.raises(Reject, match="binding_or_bitwise"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


def relabel_host_zero_as_host_one(paths: list[Path]) -> None:
    """The Lens attack: host 1 is a byte copy of host 0 with its three labels rewritten."""
    one, two = paths
    shutil.rmtree(two.parent)
    shutil.copytree(one.parent, two.parent)
    data = json.loads(two.read_bytes())
    data.update(machine_id=999, instance_id=998, role="relabelled")
    two.write_text(json.dumps(data))


@pytest.mark.parametrize(
    "attack,code",
    [
        ("copy", "qualification_attestation_signature"),
        ("copy_resigned_with_a_foreign_key", "qualification_attestation_signature"),
        ("same_pin_twice", "qualification_hosts_not_distinct:host_key"),
    ],
)
def test_relabelled_copy_of_one_host_is_refused(tmp_path: Path, attack: str, code: str) -> None:
    # Given: two attested hosts, then one host passed off as both.
    paths = results(tmp_path)
    keys = pins(paths)
    if attack == "same_pin_twice":
        keys = [keys[0], keys[0]]
    else:
        relabel_host_zero_as_host_one(paths)
        if attack == "copy_resigned_with_a_foreign_key":
            attest(paths[1], signer=7)
    # When / Then
    with pytest.raises(Reject, match=f"^{code}$"):
        driver.compare(paths, manifest(), host_keys=keys)


CHANGES = {
    "drivers": lambda d: d["environment"].update(drivers=["999.99.99"] * 2),
    "torch_path": lambda d: d["environment"].update(torch_path="/somewhere/else/torch/x.py"),
    "config_sha256": lambda d: d.update(config_sha256="ab" * 32),
    "elapsed_seconds": lambda d: d.update(elapsed_seconds=0.001),
}


@pytest.mark.parametrize("field", list(CHANGES))
def test_result_fields_changed_after_signing_are_refused(tmp_path: Path, field: str) -> None:
    # Given: host 1's result edited after its host signed it.
    paths = results(tmp_path)
    data = json.loads(paths[1].read_bytes())
    CHANGES[field](data)
    paths[1].write_text(json.dumps(data))
    # When / Then
    with pytest.raises(Reject, match="^qualification_attestation_unbound$"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


@pytest.mark.parametrize("recorded", ["other_gpus", "none"])
def test_result_from_other_gpus_is_unbound(tmp_path: Path, recorded: str) -> None:
    # Given: host 1 honestly re-signs a result that names host 0's GPUs, or names none.
    paths = results(tmp_path)
    data = json.loads(paths[1].read_bytes())
    if recorded == "other_gpus":
        data["environment"]["gpu_uuids"] = host_gpus(0)[:2]
    else:
        del data["environment"]["gpu_uuids"]
    paths[1].write_text(json.dumps(data))
    attest(paths[1])
    # When / Then
    with pytest.raises(Reject, match="^qualification_attestation_unbound$"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


@pytest.mark.parametrize(
    "override",
    [
        {"image_digest": "sha256:" + "55" * 32},
        {"config_sha256": "ab" * 32},
        {"drivers": ["595.91"]},
    ],
    ids=["image", "config", "driver"],
)
def test_attestation_for_another_image_config_or_driver_is_unbound(
    tmp_path: Path, override: dict[str, Any]
) -> None:
    # Given: host 1 signed a statement about another image, another config or another driver.
    paths = results(tmp_path)
    attest(paths[1], **override)
    # When / Then
    with pytest.raises(Reject, match="^qualification_attestation_unbound$"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


@pytest.mark.parametrize("fault", ["missing", "not_canonical"])
def test_missing_or_non_canonical_attestation_is_refused(tmp_path: Path, fault: str) -> None:
    # Given
    paths = results(tmp_path)
    statement = paths[1].parent / host_attest.ATTESTATION
    if fault == "missing":
        statement.unlink()
    else:
        statement.write_text(json.dumps(json.loads(statement.read_bytes()), indent=1))
    # When / Then
    with pytest.raises(Reject, match=f"^qualification_attestation_{fault}$"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


@pytest.mark.parametrize("shared", ["gpu_uuid", "boot_id"])
def test_hosts_sharing_a_gpu_or_a_boot_id_are_refused(tmp_path: Path, shared: str) -> None:
    # Given: two honestly signed statements that still describe one box.
    paths = results(tmp_path)
    if shared == "gpu_uuid":  # One of host 1's eight GPUs is one of host 0's.
        attest(paths[1], gpu_uuids=sorted([*host_gpus(1)[:7], host_gpus(0)[3]]))
    else:
        attest(paths[1], boot_id=boot_id(0))
    # When / Then
    with pytest.raises(Reject, match=f"^qualification_hosts_not_distinct:{shared}$"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


def test_label_only_accepts_legacy_results_without_attestation(tmp_path: Path) -> None:
    # Given: results written before attestations and device UUIDs existed (the Vast lifecycle).
    paths = results(tmp_path)
    for path in paths:
        data = json.loads(path.read_bytes())
        del data["environment"]["gpu_uuids"]
        path.write_text(json.dumps(data))
        (path.parent / host_attest.ATTESTATION).unlink()
        (path.parent / host_attest.SIGNATURE).unlink()
    # When
    result = driver.compare(paths, manifest(), host_keys=None)
    # Then
    assert result == {"two_host_bitwise_match": True, "workload_allowed": False}


def test_compare_has_no_default_trust_mode(tmp_path: Path) -> None:
    # Given / When / Then: the bare call that used to pass the relabelled copy is no longer a call.
    with pytest.raises(TypeError, match="host_keys"):
        driver.compare(results(tmp_path), manifest())  # type: ignore[call-arg]


def test_compare_refuses_one_key_for_two_hosts(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    # When / Then
    with pytest.raises(Reject, match="^qualification_requires_two_host_results$"):
        driver.compare(paths, manifest(), host_keys=pins(paths)[:1])


def committed_pins(tmp_path: Path, paths: list[Path]) -> tuple[list[Path], list[driver.HostPin]]:
    """Per host: a config committing to its known_hosts file, and the pin the owner types."""
    configs = []
    for path in paths:
        index = host_index(path)
        known = known_hosts_of(path)
        directory = tmp_path / f"cfg-{index}"
        directory.mkdir()
        cfg = seed_config(directory, TREE / "experiments/gpu_network_v2/profile.json")
        committed = cfg.model_copy(update={"known_hosts_sha256": fsha(known)})
        config = directory / "config.json"
        config.write_bytes(committed.model_dump_json().encode())
        configs.append(config)
    return configs, typed_pins(paths)


def test_pinned_keys_accepts_the_committed_known_hosts_and_typed_fingerprint(
    tmp_path: Path,
) -> None:
    # Given
    paths = results(tmp_path)
    configs, host_pins = committed_pins(tmp_path, paths)
    # When
    keys = driver.pinned_keys([c.read_bytes() for c in configs], host_pins)
    # Then
    assert keys == pins(paths)


@pytest.mark.parametrize(
    "fault,code",
    [
        ("known_hosts_edited", "qualification_known_hosts_changed"),
        ("fingerprint_of_the_other_host", "qualification_host_fingerprint_mismatch"),
        ("one_pin", "qualification_requires_two_host_results"),
    ],
)
def test_pinned_keys_refuses_what_the_owner_did_not_commit_to(
    tmp_path: Path, fault: str, code: str
) -> None:
    # Given
    paths = results(tmp_path)
    configs, host_pins = committed_pins(tmp_path, paths)
    if fault == "known_hosts_edited":
        host_pins[0].known_hosts.write_text(host_pins[0].known_hosts.read_text() + "# edited\n")
    elif fault == "fingerprint_of_the_other_host":
        host_pins[0] = host_pins[0]._replace(fingerprint=host_pins[1].fingerprint)
    else:
        host_pins = host_pins[:1]
    # When / Then
    with pytest.raises(Reject, match=f"^{code}$"):
        driver.pinned_keys([c.read_bytes() for c in configs], host_pins)


@pytest.mark.parametrize(
    "mode",
    [
        [],
        ["--known-hosts", "a", "b"],
        ["--host-fingerprint", "x", "y"],
        ["--label-only", "--known-hosts", "a", "b", "--host-fingerprint", "x", "y"],
    ],
    ids=["bare", "no_fingerprints", "no_known_hosts", "both_modes"],
)
def test_compare_cli_requires_an_explicit_trust_mode(
    monkeypatch: pytest.MonkeyPatch, mode: list[str]
) -> None:
    # Given: arguments that name no files; the mode is refused before any is read.
    argv = ["network_qualification", "compare", "c0", "c1", "r0", "r1", *mode]
    monkeypatch.setattr(sys, "argv", argv)
    # When / Then
    with pytest.raises(SystemExit, match="--label-only"):
        driver.main()


def test_compare_cli_runs_in_either_trust_mode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # Given: attested results whose configs commit to the known_hosts files.
    paths = results(tmp_path)
    configs, host_pins = committed_pins(tmp_path, paths)
    for path, config in zip(paths, configs, strict=True):
        data = json.loads(path.read_bytes())
        data["config_sha256"] = fsha(config)
        path.write_text(json.dumps(data))
        attest(path)
    monkeypatch.setattr(driver, "check_inputs", lambda cfg: SimpleNamespace(manifest=manifest()))
    base = ["network_qualification", "compare", *map(str, configs), *map(str, paths)]
    strict = [
        "--known-hosts",
        *(str(p.known_hosts) for p in host_pins),
        "--host-fingerprint",
        *(p.fingerprint for p in host_pins),
    ]
    expected = {"two_host_bitwise_match": True, "workload_allowed": False}
    for mode in (["--label-only"], strict):
        monkeypatch.setattr(sys, "argv", [*base, *mode])
        # When
        driver.main()
        # Then
        assert json.loads(capsys.readouterr().out) == expected
    # And a fingerprint typed for the other host is refused.
    swapped = [*strict[:4], strict[5], strict[4]]
    monkeypatch.setattr(sys, "argv", [*base, *swapped])
    with pytest.raises(Reject, match="^qualification_host_fingerprint_mismatch$"):
        driver.main()


def hash_only(paths: list[Path]) -> None:
    """What a hash-only rescue leaves locally: custody.json + JSON, no round tensors."""
    for path in paths:
        root = path.parent
        files = {str(p.relative_to(root)): fsha(p) for p in root.rglob("*") if p.is_file()}
        (root / "custody.json").write_text(json.dumps(files, sort_keys=True))
        for p in root.glob("round-*/published/**/*"):
            if p.is_file():
                p.unlink()
        # The rescue carries per-rank summaries and job bindings (HASH_RESCUE_NAMES).
        for rank in root.glob("round-*/published/rank-*"):
            (rank / "summary.json").write_text('{"backend": "cuda"}')
        for published in root.glob("round-*/published"):
            (published / "job.json").write_text("{}")


def test_hash_only_rescue_compares_from_bound_host_inventories(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    hash_only(paths)
    # When
    result = driver.compare(paths, manifest(), host_keys=pins(paths))
    # Then
    assert result == {"two_host_bitwise_match": True, "workload_allowed": False}


def test_full_rescue_missing_tensor_rejects_despite_inventory(tmp_path: Path) -> None:
    # Given: a full-rescue tree with one tensor deleted and a custody.json that still lists it.
    paths = results(tmp_path)
    root = paths[1].parent
    files = {str(p.relative_to(root)): fsha(p) for p in root.rglob("*") if p.is_file()}
    (root / "custody.json").write_text(json.dumps(files, sort_keys=True))
    (root / "round-1/published/rank-0/state.safetensors").unlink()
    # When / Then
    with pytest.raises(Reject, match="custody_changed"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


@pytest.mark.parametrize(
    "fault,match",
    [
        ("mismatch", "custody_changed"),
        ("missing_rank", "custody_changed"),
        ("unbound", "inventory_unbound"),
        ("malformed", "inventory_malformed"),
        ("not_hex", "inventory_malformed"),
        ("oversized", "inventory_oversized"),
        ("absent", "inventory_missing"),
    ],
)
def test_hash_only_inventory_faults_reject(tmp_path: Path, fault: str, match: str) -> None:
    # Given
    paths = results(tmp_path)
    hash_only(paths)
    custody = paths[1].parent / "custody.json"
    data = json.loads(custody.read_bytes())
    if fault == "mismatch":  # Host B's inventory disagrees with what both results claim.
        data["round-1/published/rank-1/state.safetensors"] = "ab" * 32
    elif fault == "missing_rank":
        data = {k: v for k, v in data.items() if "/rank-1/" not in k}
    elif fault == "unbound":
        data["result.json"] = "00" * 32
    elif fault == "not_hex":
        data["round-0/published/rank-0/delta.bin"] = "ZZ" * 32
    custody.write_text(json.dumps(data))
    if fault == "malformed":
        custody.write_bytes(b"[not json")
    elif fault == "oversized":
        custody.write_bytes(b" " * (driver.INVENTORY_BYTES + 1))
    elif fault == "absent":
        custody.unlink()
    # When / Then
    with pytest.raises(Reject, match=match):
        driver.compare(paths, manifest(), host_keys=pins(paths))


def test_cross_host_hash_mismatch_rejects_with_hash_only_rescue(tmp_path: Path) -> None:
    # Given: host B honestly reports a different round hash for one rank.
    paths = results(tmp_path)
    data = json.loads(paths[1].read_bytes())
    data["rounds"][1]["rank-0/delta.bin"] = "cd" * 32
    paths[1].write_text(json.dumps(data))
    hash_only(paths)
    # When / Then
    with pytest.raises(Reject, match="binding_or_bitwise"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


@pytest.mark.parametrize(
    "field,value", [("backend", "cpu"), ("torch", "2.14.0+cpu"), ("sm_counts", [170])]
)
def test_cpu_or_incomplete_environment_rejects(
    tmp_path: Path, field: str, value: str | list[int]
) -> None:
    # Given
    paths = results(tmp_path)
    data = json.loads(paths[0].read_bytes())["environment"]
    data[field] = value
    # When / Then
    with pytest.raises(ValidationError):
        driver.Environment.model_validate(data)


def test_source_manifest_detects_profile_change(tmp_path: Path) -> None:
    # Given
    for rel in [
        "src/hypertrain/x.py",
        "experiments/gpu_network_v2/run.py",
        "experiments/gpu_network_v2/orchestrate.py",
        "experiments/gpu_network_v2/profile.json",
        "scripts/network_gpu_qualification.py",
        "docker/Dockerfile.gpu",
        "pyproject.toml",
        "uv.lock",
    ]:
        path = tmp_path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"initial")
    initial = driver.sources(tmp_path)
    (tmp_path / "experiments/gpu_network_v2/profile.json").write_bytes(b"changed")
    # When
    changed = driver.sources(tmp_path)
    # Then
    assert initial != changed


def test_missing_runtime_proof_rejects_before_launch(tmp_path: Path) -> None:
    # Given
    config = tmp_path / "config.json"
    config.write_text("{}")
    # When / Then
    with pytest.raises(ValidationError):
        driver.run(config)


def test_partial_round_coverage_rejects_even_identical_results(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path)
    for path in paths:
        data = json.loads(path.read_bytes())
        del data["rounds"][0]["rank-0/checkpoints/30.safetensors"]
        path.write_text(json.dumps(data))
    # When / Then
    with pytest.raises(Reject, match="coverage_missing"):
        driver.compare(paths, manifest(), host_keys=pins(paths))


def test_eight_gpu_h4_j2_results_compare_with_full_rank_coverage(tmp_path: Path) -> None:
    # Given
    paths = results(tmp_path, ranks=8, H=4, J=2)
    # When
    result = driver.compare(paths, manifest(8, 4, 2), host_keys=pins(paths))
    # Then: 8 ranks x 5 finals + rank-0 checkpoints 0,2,4 (only rank 0 writes them).
    rounds = json.loads(paths[0].read_bytes())["rounds"]
    assert result == {"two_host_bitwise_match": True, "workload_allowed": False}
    assert [len(r) for r in rounds] == [43, 43] and "rank-0/checkpoints/4.safetensors" in rounds[1]
    assert not any(k.startswith("rank-7/checkpoints/") for k in rounds[1])


@pytest.mark.parametrize("attack", ["missing_rank", "same_host", "tamper", "wrong_image"])
def test_eight_gpu_results_reject_rank_host_custody_and_shape_attacks(
    tmp_path: Path, attack: str
) -> None:
    # Given
    paths = results(tmp_path, ranks=8, H=4, J=2)
    datas = [json.loads(p.read_bytes()) for p in paths]
    if attack == "missing_rank":  # Both hosts agree on a 7-rank capture.
        for data in datas:
            data["rounds"] = [
                {k: v for k, v in r.items() if not k.startswith("rank-7/")} for r in data["rounds"]
            ]
    elif attack == "same_host":
        datas[1]["machine_id"] = datas[0]["machine_id"]
    elif attack == "wrong_image":  # Both hosts agree on an image the manifest never named.
        for data in datas:
            data["environment"]["image_digest"] = "sha256:" + "55" * 32
    for path, data in zip(paths, datas, strict=True):
        path.write_text(json.dumps(data))
    if attack == "tamper":
        (paths[1].parent / "round-1/published/rank-0/checkpoints/2.safetensors").write_bytes(b"x")
    match = {"missing_rank": "coverage_missing", "tamper": "custody_changed"}
    # When / Then
    with pytest.raises(Reject, match=match.get(attack, "binding_or_bitwise")):
        driver.compare(paths, manifest(8, 4, 2), host_keys=pins(paths))


@pytest.mark.parametrize(
    "attack", ["two_ranks_j4", "eight_ranks_j4", "legacy_j15", "no_checkpoints"]
)
def test_coordinated_self_consistent_results_cannot_redefine_layout(
    tmp_path: Path, attack: str
) -> None:
    # Given: both hosts agree bitwise on a shape the 8-GPU H4/J2 seed manifest never named.
    shape = {"two_ranks_j4": (2, 4, 4), "eight_ranks_j4": (8, 4, 4), "legacy_j15": (8, 30, 15)}
    paths = results(tmp_path, *shape.get(attack, (8, 4, 2)))
    if attack == "no_checkpoints":  # What a J=-1 capture listed: finals only.
        for path in paths:
            data = json.loads(path.read_bytes())
            data["rounds"] = [
                {k: v for k, v in r.items() if "/checkpoints/" not in k} for r in data["rounds"]
            ]
            path.write_text(json.dumps(data))
    # When / Then: the manifest, not the results, defines ranks and checkpoints.
    with pytest.raises(Reject, match="binding_or_bitwise|coverage_missing"):
        driver.compare(paths, manifest(8, 4, 2), host_keys=pins(paths))


@pytest.mark.parametrize("shape", [(8, 30, 0), (8, 30, -1), (8, 30, 7), (0, 4, 2)])
def test_round_files_refuses_non_positive_or_non_dividing_shape(
    shape: tuple[int, int, int],
) -> None:
    # When / Then
    with pytest.raises(Reject, match="layout_invalid"):
        driver.round_files(*shape)


def test_environment_rejects_driver_count_differing_from_devices(tmp_path: Path) -> None:
    # Given
    data = json.loads(results(tmp_path, ranks=8, H=4, J=2)[0].read_bytes())["environment"]
    data["drivers"] = data["drivers"][:7]
    # When / Then
    with pytest.raises(ValidationError, match="driver/device count"):
        driver.Environment.model_validate(data)


@pytest.mark.parametrize(
    "capability,sm_count", [(RTX_5090, 170), (B300, 148)], ids=["rtx5090", "b300"]
)
def test_result_admits_each_qualified_capability_sm_pair(
    tmp_path: Path, capability: tuple[int, int], sm_count: int
) -> None:
    # Given: a result whose devices are one row of the per-arch table.
    data = json.loads(results(tmp_path)[0].read_bytes())
    data["environment"].update(capabilities=[capability] * 2, sm_counts=[sm_count] * 2)
    # When
    result = driver.Result.model_validate_json(json.dumps(data))
    # Then
    assert result.environment.capabilities == [capability] * 2
    assert result.environment.sm_counts == [sm_count] * 2


@pytest.mark.parametrize(
    "capabilities,sm_counts,match",
    [
        ([RTX_5090] * 2, [148] * 2, r"capability 12\.0 qualifies \[170\] SMs, not 148"),
        ([(9, 0)] * 2, [132] * 2, r"capability 9\.0 is not a qualified device class"),
        ([RTX_5090], [170, 132], "capability/device count mismatch"),
    ],
    ids=["b300_count_on_rtx5090", "unknown_pair", "unlisted_device"],
)
def test_result_refuses_pair_outside_the_table(
    tmp_path: Path, capabilities: list[tuple[int, int]], sm_counts: list[int], match: str
) -> None:
    # Given
    data = json.loads(results(tmp_path)[0].read_bytes())
    data["environment"].update(capabilities=capabilities, sm_counts=sm_counts)
    # When / Then
    with pytest.raises(ValidationError, match=match):
        driver.Result.model_validate_json(json.dumps(data))


def test_result_without_capabilities_is_an_rtx_5090_record_only(tmp_path: Path) -> None:
    # Given: result.json as written before capabilities were recorded (Literal[170] era).
    data = json.loads(results(tmp_path)[0].read_bytes())
    del data["environment"]["capabilities"]
    # When
    legacy = driver.Result.model_validate_json(json.dumps(data))
    # Then: it still qualifies 170-SM devices and serializes without the field...
    assert legacy.environment.capabilities is None
    assert "capabilities" not in json.loads(legacy.model_dump_json())["environment"]
    # ...but cannot stand for any other device class.
    data["environment"]["sm_counts"] = [148, 148]
    with pytest.raises(ValidationError, match="without capabilities qualifies 170-SM"):
        driver.Result.model_validate_json(json.dumps(data))


def torch_uuid(index: int) -> str:
    """What torch prints for a device UUID: no `GPU-` prefix."""
    return str(uuid.UUID(int=index + 1))


def fake_cuda_host(
    monkeypatch: pytest.MonkeyPatch,
    devices: list[tuple[tuple[int, int], int]],
    uuids: list[str] | None = None,
) -> None:
    """Fake the image's torch/nvidia-smi probes; no GPU or subprocess is touched."""
    import torch  # After hypertrain.trainer configured determinism.

    printed = uuids or [torch_uuid(i) for i in range(len(devices))]

    monkeypatch.setattr(torch.cuda, "is_available", lambda: True)
    monkeypatch.setattr(torch.cuda, "device_count", lambda: len(devices))
    monkeypatch.setattr(
        torch.cuda,
        "get_device_properties",
        lambda i: SimpleNamespace(
            major=devices[i][0][0],
            minor=devices[i][0][1],
            multi_processor_count=devices[i][1],
            uuid=printed[i],
        ),
    )
    monkeypatch.setattr(torch, "__file__", "/venv/main/torch/__init__.py")
    monkeypatch.setattr(torch, "__version__", "2.14.0+cu130")
    monkeypatch.setattr(torch.version, "cuda", "13.0")
    monkeypatch.setattr(driver.sys, "executable", "/opt/hypertrain/venv/bin/python")
    monkeypatch.setattr(driver.shutil, "which", lambda name: "/usr/bin/nvidia-smi")
    smi = SimpleNamespace(stdout="595.84\n" * len(devices))
    monkeypatch.setattr(driver.subprocess, "run", lambda *a, **k: smi)


@pytest.mark.parametrize(
    "capability,sm_count", [(RTX_5090, 170), (B300, 148)], ids=["rtx5090", "b300"]
)
def test_inspect_environment_records_capability_with_manifest_sm_count(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capability: tuple[int, int], sm_count: int
) -> None:
    # Given: a manifest naming the host's SM count, on a qualified device class.
    cfg = seed_config(tmp_path, TREE / "experiments/gpu_network_v2/profile.json")
    fake_cuda_host(monkeypatch, [(capability, sm_count)] * 2)
    # When
    environment = driver.inspect_environment(cfg, 2, sm_count)
    # Then
    assert environment.capabilities == [capability] * 2
    assert environment.sm_counts == [sm_count] * 2


def test_inspect_environment_records_one_distinct_uuid_per_device(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: two devices, torch printing their UUIDs without the nvidia-smi prefix.
    cfg = seed_config(tmp_path, TREE / "experiments/gpu_network_v2/profile.json")
    fake_cuda_host(monkeypatch, [(B300, 148)] * 2)
    # When
    environment = driver.inspect_environment(cfg, 2, 148)
    # Then: the nvidia-smi form, in device order.
    assert environment.gpu_uuids == [
        "GPU-00000000-0000-0000-0000-000000000001",
        "GPU-00000000-0000-0000-0000-000000000002",
    ]


@pytest.mark.parametrize("printed", ["not-a-uuid", "0000000g-0000-0000-0000-000000000001"])
def test_inspect_environment_refuses_a_malformed_device_uuid(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, printed: str
) -> None:
    # Given: a device whose UUID is not 36 hex digits in the 8-4-4-4-12 layout.
    cfg = seed_config(tmp_path, TREE / "experiments/gpu_network_v2/profile.json")
    fake_cuda_host(monkeypatch, [(B300, 148)] * 2, uuids=[torch_uuid(0), printed])
    # When / Then
    with pytest.raises(Reject, match="^qualification_gpu_uuid_malformed$"):
        driver.inspect_environment(cfg, 2, 148)


@pytest.mark.parametrize(
    "gpu_uuids,match",
    [
        (["GPU-" + torch_uuid(0)] * 2, "must be distinct"),
        (["GPU-" + torch_uuid(0)], "one GPU UUID per device"),
        (["GPU-" + torch_uuid(i) for i in range(3)], "one GPU UUID per device"),
        (
            ["GPU-0A1B2C3D-0000-4000-8000-000000000001", "GPU-" + torch_uuid(1)],
            "lowercase GPU-<uuid>",
        ),
        ([torch_uuid(0), torch_uuid(1)], "lowercase GPU-<uuid>"),
    ],
    ids=["duplicate", "fewer_than_devices", "more_than_devices", "uppercase", "no_prefix"],
)
def test_environment_refuses_duplicate_missing_or_malformed_uuids(
    tmp_path: Path, gpu_uuids: list[str], match: str
) -> None:
    # Given: a two-device environment whose gpu_uuids are wrong.
    data = json.loads(results(tmp_path)[0].read_bytes())["environment"]
    data["gpu_uuids"] = gpu_uuids
    # When / Then
    with pytest.raises(ValidationError, match=match):
        driver.Environment.model_validate(data)


def test_legacy_result_bytes_are_unchanged(tmp_path: Path) -> None:
    # Mutant guard, passes before this change: dropping exclude_if on gpu_uuids would add
    # "gpu_uuids":null to every result written before the field existed and change its bytes.
    # Given: result.json as written before the field existed.
    data = json.loads(results(tmp_path)[0].read_bytes())
    data["environment"].pop("gpu_uuids", None)
    raw = json.dumps(data, separators=(",", ":")).encode()
    # When / Then
    assert driver.Result.model_validate_json(raw).model_dump_json().encode() == raw


def test_inspect_environment_refuses_device_off_manifest_sm_count(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: an RTX 5090 (170-SM) manifest and a host with one B300 device.
    cfg = seed_config(tmp_path, TREE / "experiments/gpu_network_v2/profile.json")
    fake_cuda_host(monkeypatch, [(RTX_5090, 170), (B300, 148)])
    # When / Then
    with pytest.raises(Reject, match="qualification_sm_profile_mismatch"):
        driver.inspect_environment(cfg, 2, 170)


def test_inspect_environment_refuses_manifest_count_on_unqualified_device(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: a 148-SM manifest and 148-SM devices that report the RTX 5090 capability.
    cfg = seed_config(tmp_path, TREE / "experiments/gpu_network_v2/profile.json")
    fake_cuda_host(monkeypatch, [(RTX_5090, 148)] * 2)
    # When / Then
    with pytest.raises(Reject, match=r"qualification_sm_profile_unqualified:device 0"):
        driver.inspect_environment(cfg, 2, 148)


def test_eight_gpu_seed_manifest_drives_rank_and_checkpoint_coverage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: exact profile with an 8-GPU dp4 x ep2 layout and H4/J2 inner loop.
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["layout"].update(n_gpus=8, dp_size=4, ep_size=2)
    profile["inner"].update(H=4, J=2)
    profile_path = tmp_path / "profile-8.json"
    profile_path.write_text(json.dumps(profile))
    job = driver.check_inputs(seed_config(tmp_path, profile_path))
    published = tmp_path / "published"
    for rel in driver.round_files(8, 4, 2):
        (published / rel).parent.mkdir(parents=True, exist_ok=True)
        (published / rel).write_bytes(rel.encode())
    for rank in range(8):
        (published / f"rank-{rank}/summary.json").write_text('{"backend": "cuda"}')
    monkeypatch.setattr(driver, "validate_artifacts", lambda job, directory: None)
    # When
    captured = driver.capture(published, job)
    # Then
    assert len(job.sample_ids) == 32 and set(captured) == set(driver.round_files(8, 4, 2))
    (published / "rank-0/checkpoints/4.safetensors").unlink()
    with pytest.raises(FileNotFoundError):
        driver.capture(published, job)


def test_exact_profile_accepts_wire_defaults_without_training(tmp_path: Path) -> None:
    # Given
    cfg = seed_config(tmp_path, TREE / "experiments/gpu_network_v2/profile.json")
    # When
    accepted = driver.check_inputs(cfg)
    # Then
    assert accepted.run_id == json.loads(cfg.seed_job.read_bytes())["run_id"]


def seed_config(tmp_path: Path, profile_path: Path) -> driver.Qualification:
    profile = json.loads(profile_path.read_bytes())
    body = example_manifest().body()
    body["model"].update(profile["model"])
    body["inner"].update({k: v for k, v in profile["inner"].items() if k != "lr_schedule"})
    body["inner"]["lr_schedule"].update(profile["inner"]["lr_schedule"])
    body["outer"].update(profile["outer"])
    registry = tmp_path / "registry.json"
    registry.write_bytes(b'{"schemaVersion":2,"layers":[{"size":42}]}')
    image = "sha256:" + fsha(registry)
    body["reference_spec"].update(image_digest=image, driver_allowlist=["595.84"], sm_count=170)
    body["reference_spec"]["layout"].update(profile["layout"])
    wrapper = RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": body,
            "network": {
                **{
                    k: "11" * 32
                    for k in (
                        "admission_policy_hash",
                        "economics_policy_hash",
                        "aggregation_policy_hash",
                        "dispute_policy_hash",
                        "audit_policy_hash",
                        "relay_registry_hash",
                    )
                },
                "audit_mode": "anchored-full",
                "full_anchor_version": 1,
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )
    job = IslandJobV1(
        job_version=1,
        run_id=wrapper.run_id(),
        w=0,
        manifest=wrapper,
        sample_ids=list(range(wrapper.training.batch_samples())),
        global_step0=0,
        start_state_sha256="22" * 32,
        ef_in_sha256="33" * 32,
        v0_sha256="44" * 32,
        object_paths={
            "start_state": "start",
            "ef_in": "ef",
            "v0": "v0",
            "samples": "samples",
            "sample_proofs": "proofs",
        },
        deadline=5000,
    )
    job_path = tmp_path / "job.json"
    job_path.write_text(job.model_dump_json())
    receipts = tmp_path / "receipts.json"
    receipts.write_text(
        json.dumps(
            {
                "instance_id": 1,
                "machine_id": 2,
                "role": "h0",
                "admitted_unix": 100,
                "image_digest": image,
                "known_hosts_sha256": "55" * 32,
                "lifecycle_contract_sha256": "66" * 32,
                "supervisor_pid": 123,
                "deadline_unix": 3700,
                "cuda_observed": True,
            }
        )
    )
    return driver.Qualification(
        tree=TREE,
        profile=profile_path,
        profile_sha256=fsha(profile_path),
        sources=driver.sources(TREE),
        registry_manifest=registry,
        image_digest=image,
        driver_allowlist=["595.84"],
        seed_job=job_path,
        hotkey=body["coord_pubkey"],
        output=tmp_path / "out",
        admitted_unix=100,
        instance_id=1,
        machine_id=2,
        role="h0",
        lifecycle_contract_sha256="66" * 32,
        known_hosts_sha256="55" * 32,
        lifecycle_receipts=receipts,
        lifecycle_receipts_sha256=fsha(receipts),
    )


def test_script_reexports_exact_packaged_engine() -> None:
    # Given: existing source-host tools still load the script by filename.
    spec = importlib.util.spec_from_file_location(
        "qualification_wrapper_test", TREE / "scripts/network_gpu_qualification.py"
    )
    assert spec and spec.loader
    wrapper = importlib.util.module_from_spec(spec)
    # When
    spec.loader.exec_module(wrapper)
    # Then: there is only one strict schema/function implementation.
    for name in (
        "Boundary",
        "Qualification",
        "Environment",
        "Result",
        "sources",
        "bundle",
        "check_inputs",
        "inspect_environment",
        "capture",
        "run",
        "compare",
        "main",
    ):
        assert getattr(wrapper, name) is getattr(driver, name)


def test_package_artifact_import_without_repository_cwd(tmp_path: Path) -> None:
    # Given: final packaging gate supplies an actual built wheel; source tests use package root.
    artifact = Path(os.environ.get("HT_QUALIFICATION_WHEEL", Path(driver.__file__).parents[2]))
    code = """
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from pydantic import ValidationError
from hypertrain.gpu_ops import network_qualification as engine
from hypertrain.challenge.store import ChallengeStore
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages_v2 import RunManifestV2
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import param_shapes
assert str(Path(engine.__file__)).startswith(sys.argv[1])
assert not Path("scripts/network_gpu_qualification.py").exists()
assert engine.Qualification.__module__ == "hypertrain.gpu_ops.network_qualification"
payload = {"tree":"tree", "profile":"profile", "profile_sha256":"11"*32,
"sources":{}, "registry_manifest":"registry", "image_digest":"sha256:"+"22"*32,
"driver_allowlist":["driver"], "seed_job":"job", "hotkey":"key", "output":"out",
"admitted_unix":1, "instance_id":1, "machine_id":2, "role":"h0",
"lifecycle_contract_sha256":"33"*32, "known_hosts_sha256":"44"*32,
"lifecycle_receipts":"receipt", "lifecycle_receipts_sha256":"55"*32}
assert engine.Qualification.model_validate(payload).tree == Path("tree")
for bad in ({**payload,"instance_id":True}, {**payload,"unexpected":1}):
    try:
        engine.Qualification.model_validate(bad)
    except ValidationError:
        pass
    else:
        raise AssertionError("strict schema weakened")
body = example_manifest().body()
body["model"]["param_count"] = sum(__import__("math").prod(shape) for shape in
    param_shapes(TrainConfig.from_manifest(example_manifest()).model).values())
manifest = RunManifestV2.model_validate({"manifest_version":2,"training":body,"network":{
**{n:"11"*32 for n in ("admission_policy_hash","economics_policy_hash",
"aggregation_policy_hash","dispute_policy_hash","audit_policy_hash","relay_registry_hash")},
"audit_mode":"anchored-full","full_anchor_version":1,
"capabilities":["island-replay","all-level-disputes","transport-receipts"]}})
store = ChallengeStore.__new__(ChallengeStore)
try:
    store._bootstrap_backend_v2(manifest, (Path("missing-config"),)*2,
                                (Path("missing-result"),)*2, b"", pins=None)
except FileNotFoundError as error:
    assert error.filename == "missing-config", str(error)
else:
    raise AssertionError("expected real config read after packaged importer")
print("PACKAGED_QUALIFICATION_STRICT_SCHEMA_AND_STORE_IMPORT_PASS")
"""
    # When: isolated interpreter ignores checkout PYTHONPATH and runs outside repository.
    result = subprocess.run(
        [sys.executable, "-I", "-c", code, str(artifact.resolve())],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    # Then: real package/store seam imports without a sibling scripts tree.
    assert result.returncode == 0, result.stderr
    assert "PACKAGED_QUALIFICATION_STRICT_SCHEMA_AND_STORE_IMPORT_PASS" in result.stdout
