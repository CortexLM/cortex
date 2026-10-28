"""The od-large builder pins the device class of the 8x RTX 5090 run it builds."""

from __future__ import annotations

from typing import Any

from scripts.ops.manifest import od_large_manifest as od

from hypertrain.protocol.env_pins import EnvPin
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import ReferenceSpec


def test_pin_names_the_device_of_the_sm_count_the_builder_writes() -> None:
    body: dict[str, Any] = example_manifest().body()["reference_spec"]
    body["sm_count"] = od.SM_COUNT
    body["env"]["pinned"] = od.ENV_PIN
    ReferenceSpec.model_validate(body)  # refuses an sm_count its gpu class was not measured at
    pin = EnvPin.from_body(od.ENV_PIN)
    assert (pin.cpu, pin.gpu) == ("avx2", "sm_120")
