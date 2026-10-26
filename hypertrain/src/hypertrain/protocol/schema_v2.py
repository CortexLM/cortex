"""V2-only schema export; caller supplies destination, frozen v1 schemas stay untouched."""

from __future__ import annotations

import json
from pathlib import Path

from pydantic import BaseModel

from hypertrain.protocol.envelope_v2 import EnvelopeV2
from hypertrain.protocol.messages_v2 import MESSAGE_TYPES_V2, TrainingManifestV2
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import RELAY_MESSAGE_TYPES

SCHEMA_MODELS_V2: dict[str, type[BaseModel]] = {
    **MESSAGE_TYPES_V2,
    **RELAY_MESSAGE_TYPES,
    "TrainingManifestV2": TrainingManifestV2,
    "EnvelopeV2": EnvelopeV2,
    "RelayEnvelope": RelayEnvelope,
}


def render() -> dict[str, bytes]:
    return {
        f"{name}.json": (
            json.dumps(model.model_json_schema(), sort_keys=True, indent=2) + "\n"
        ).encode()
        for name, model in SCHEMA_MODELS_V2.items()
    }


def write(out: Path) -> list[Path]:
    out.mkdir(parents=True, exist_ok=True)
    paths = []
    for name, data in render().items():
        path = out / name
        path.write_bytes(data)
        paths.append(path)
    return paths
