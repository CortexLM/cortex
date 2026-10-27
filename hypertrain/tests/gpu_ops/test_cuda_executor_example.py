"""The operator example for the generic CUDA executor loads and states the code's defaults."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from hypertrain.gpu_ops.cuda_executor import ExecutorConfig

EXAMPLE = Path(__file__).parents[2] / "docs/examples/cuda-executor.example.json"


def _filled(tmp_path: Path) -> dict[str, Any]:
    key = tmp_path / "id_ed25519"
    key.write_text("not a real key\n")
    key.chmod(0o600)
    known = tmp_path / "known_hosts"
    known.write_text("[gpu.example.net]:22 ssh-ed25519 AAAA\n")
    raw = json.loads(EXAMPLE.read_text())
    raw["run_id"] = "ab" * 32
    raw["state_dir"] = str(tmp_path / "state")
    for index, host in enumerate(raw["hosts"]):
        host.update(
            address=f"gpu{index}.example.net",
            identity_file=str(key),
            known_hosts_file=str(known),
        )
    return raw


def test_example_loads_and_every_placeholder_is_filled_by_the_test(tmp_path: Path) -> None:
    config = ExecutorConfig.model_validate(_filled(tmp_path))
    assert [h.label for h in config.hosts] == ["gpu-a", "gpu-b"]
    assert config.hosts[0].docker == ("sudo", "-n", "docker")


def test_example_states_the_defaults_of_every_optional_field(tmp_path: Path) -> None:
    raw = _filled(tmp_path)
    optional = {
        name: field.default
        for name, field in ExecutorConfig.model_fields.items()
        if not field.is_required()
    }
    assert set(optional) <= set(raw), "the example must show every optional field"
    assert {name: raw[name] for name in optional} == optional
