"""ADR 0011 part 3: a miner whose host cannot honor the manifest's environment pin is told at join
and at the trial, before any request: bond, admission and the trial deadline are not spent on a
round the island workers would refuse."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from test_island_launch_v2 import tiny_manifest

from env_pin_fixtures import CPU_FP32
from hypertrain.miner import core
from hypertrain.miner.core import MinerError, NetworkMiner
from hypertrain.protocol.env_pins import EnvPin, EnvPinUnhonorable


class NoRequests:
    def call(self, *args: Any, **kwargs: Any) -> None:
        raise AssertionError("a request was made before the pin was checked")


def miner(pinned: dict[str, Any] | None) -> NetworkMiner:
    instance = NetworkMiner.__new__(NetworkMiner)
    instance.manifest = tiny_manifest(2, pinned=pinned)
    instance.api = NoRequests()  # type: ignore[assignment]
    return instance


def refuse(pin: EnvPin) -> str:
    raise EnvPinUnhonorable("ATen CPU capability: manifest 'AVX2', live 'DEFAULT'")


@pytest.mark.parametrize("entry", ["join", "probe_trial"])
def test_join_and_probe_refuse_before_any_request(
    entry: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(core, "require_honorable", refuse)
    pinned = miner(CPU_FP32)
    entries = {
        "join": lambda: pinned.join(tmp_path / "coldkey", "request-1", 10),
        "probe_trial": pinned.probe_trial,
    }
    with pytest.raises(MinerError, match="cannot honor the manifest's environment pin: ATen CPU"):
        entries[entry]()


def test_preflight_probes_only_a_pinned_manifest_and_passes_a_honorable_pin(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def forbidden(pin: EnvPin) -> str:
        raise AssertionError("an unpinned manifest must never probe the host")

    monkeypatch.setattr(core, "require_honorable", forbidden)
    miner(None).preflight_env_pin()
    monkeypatch.setattr(core, "require_honorable", lambda pin: "AVX2")
    miner(CPU_FP32).preflight_env_pin()
