"""L2: architecture dispatch in trainer.model and the warm_start branch of Miner.start_state."""

from __future__ import annotations

import copy
import importlib.util
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest
import torch

import hypertrain.trainer.model as tm
from hypertrain.miner.core import Miner
from hypertrain.protocol.example import example_manifest
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import forward, init_params, param_shapes, stage_of


def od_body() -> dict[str, Any]:
    spec = importlib.util.spec_from_file_location(
        "_od_fields", Path(__file__).parents[1] / "protocol" / "test_od_fields.py"
    )
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.od_body()  # type: ignore[no-any-return]


class Stub:
    calls: list[tuple[str, tuple[Any, ...], dict[str, Any]]]

    def __init__(self) -> None:
        self.calls = []

    def _rec(self, name: str, *a: Any, **k: Any) -> str:
        self.calls.append((name, a, k))
        return name

    def param_shapes(self, cfg: Any) -> Any:
        return self._rec("param_shapes", cfg)

    def stage_of(self, name: str, cfg: Any, n: int) -> Any:
        return self._rec("stage_of", name, cfg, n)

    def init_params(self, cfg: Any) -> Any:
        return self._rec("init_params", cfg)

    def forward(self, cfg: Any, p: Any, tokens: Any, moe: Any = None) -> Any:
        return self._rec("forward", cfg, p, tokens, moe=moe)


@pytest.fixture
def od_cfg() -> TrainConfig:
    return TrainConfig.from_manifest(od_body())


def test_config_fills_od_fields(od_cfg: TrainConfig) -> None:
    m = od_cfg.model
    assert (m.arch, m.profile) == ("od-encoder", "od-bf16-det-eager-v1")
    assert m.od is not None and m.od.preset == "od-tiny"
    hash(m)  # frozen ODSpec keeps ModelConfig hashable for the adapter cache


def test_decoder_defaults_unchanged() -> None:
    m = TrainConfig.from_manifest(example_manifest().body()).model
    assert (m.arch, m.od, m.profile) == ("decoder", None, "")


def test_dispatch_routes_to_arch_impl(od_cfg: TrainConfig, monkeypatch: pytest.MonkeyPatch) -> None:
    stub = Stub()
    monkeypatch.setattr(tm, "_arch_impl", lambda cfg: stub)
    cfg = od_cfg.model
    assert param_shapes(cfg) == "param_shapes"
    assert stage_of("tok.weight", cfg, 1) == "stage_of"
    assert init_params(cfg) == "init_params"
    sentinel = object()
    assert forward(cfg, {}, torch.zeros(1, 2, dtype=torch.long), moe=sentinel) == "forward"  # type: ignore[arg-type]
    assert [c[0] for c in stub.calls] == ["param_shapes", "stage_of", "init_params", "forward"]
    assert stub.calls[-1][2]["moe"] is sentinel


def test_decoder_never_touches_arch_impl(monkeypatch: pytest.MonkeyPatch) -> None:
    def boom(cfg: Any) -> Any:
        raise AssertionError("decoder must not dispatch")

    monkeypatch.setattr(tm, "_arch_impl", boom)
    cfg = TrainConfig.from_manifest(example_manifest().body()).model
    assert "emb.weight" in param_shapes(cfg)
    assert stage_of("emb.weight", cfg, 1) == 0


def _miner(cfg: TrainConfig, fetched: list[str]) -> Any:
    m = SimpleNamespace(train_cfg=cfg)
    m._fetch_state = lambda h: (fetched.append(h), b"")[1]
    return m


@pytest.mark.parametrize("warm", [False, True])
def test_start_state_warm_start_branch(warm: bool, monkeypatch: pytest.MonkeyPatch) -> None:
    b = copy.deepcopy(od_body())
    b["model"]["od"]["warm_start"] = warm
    cfg = TrainConfig.from_manifest(b)
    inits: list[Any] = []
    fetched: list[str] = []
    theta = {"x": torch.zeros(1)}
    import hypertrain.miner.core as core

    monkeypatch.setattr(core, "init_params", lambda c: (inits.append(c), theta)[1])
    monkeypatch.setattr(core, "unpack_state", lambda blob: (theta, None))
    monkeypatch.setattr(core, "state_hash", lambda t: "h")
    ro = SimpleNamespace(theta_hash="h")
    out = Miner.start_state(_miner(cfg, fetched), 0, ro)  # type: ignore[arg-type]
    assert out is theta
    assert bool(inits) is (not warm)
    assert fetched == (["h"] if warm else [])
