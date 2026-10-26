"""Export JSON schemas to docs/schemas/: `python -m hypertrain.protocol.schema [out_dir]`."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

from hypertrain.protocol.example import example_document
from hypertrain.protocol.messages import SCHEMA_MODELS, V1_ARCHS

DEFAULT_OUT = Path(__file__).resolve().parents[3] / "docs" / "schemas"


def _v1(schema: dict[str, Any]) -> dict[str, Any]:
    """The frozen v1 wire contract: a signed body always carries ``arch`` (the canonical dump
    writes it, intake compares bytes) and v1 names only V1_ARCHS; the model's default and its
    registry enum describe unsigned parsing and v2 (ADR 0006)."""
    spec = schema.get("$defs", {}).get("ModelSpec")
    if spec is not None:
        spec["properties"]["arch"] = {"enum": list(V1_ARCHS), "title": "Arch", "type": "string"}
        spec["required"] = ["arch", *spec["required"]]
    return schema


def render() -> dict[str, bytes]:
    files = {
        f"{name}.json": (
            json.dumps(_v1(model.model_json_schema()), indent=2, sort_keys=True) + "\n"
        )
        for name, model in SCHEMA_MODELS.items()
    }
    files["example-run-manifest.json"] = (
        json.dumps(example_document(), indent=2, sort_keys=True) + "\n"
    )
    return {k: v.encode() for k, v in files.items()}


def write(out: Path) -> list[Path]:
    out.mkdir(parents=True, exist_ok=True)
    paths = []
    for name, data in render().items():
        p = out / name
        p.write_bytes(data)
        paths.append(p)
    return paths


if __name__ == "__main__":
    for p in write(Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUT):
        print(p)
