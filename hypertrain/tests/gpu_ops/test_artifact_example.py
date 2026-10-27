"""The operator example for the executor's R2 settings loads and states the code's defaults."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from hypertrain.gpu_ops.artifact_transfer import ArtifactSettings

EXAMPLE = Path(__file__).parents[2] / "docs/examples/cuda-executor-artifacts.example.json"


def _filled(tmp_path: Path) -> dict[str, Any]:
    raw = json.loads(EXAMPLE.read_text())
    raw["credentials_file"] = str(tmp_path / "r2-exec.env")
    return raw


def test_example_loads_with_its_one_placeholder_filled(tmp_path: Path) -> None:
    settings = ArtifactSettings.model_validate(_filled(tmp_path))
    assert settings.operations == ("audit", "referee") and settings.fallback == "classic"


def test_example_states_the_defaults_of_every_optional_field(tmp_path: Path) -> None:
    raw = _filled(tmp_path)
    optional = {
        name: field.default
        for name, field in ArtifactSettings.model_fields.items()
        if not field.is_required()
    }
    assert set(optional) <= set(raw), "the example must show every optional field"
    assert {name: raw[name] for name in optional} == {
        name: list(value) if isinstance(value, tuple) else value for name, value in optional.items()
    }
