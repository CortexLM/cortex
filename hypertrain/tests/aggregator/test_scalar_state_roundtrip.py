"""Rank0 OD temperatures are not length-one vectors; no runtime training needed."""

from pathlib import Path

import numpy as np
import pytest
import torch
from safetensors.numpy import load as st_load
from safetensors.numpy import save as st_save

from hypertrain.aggregator.checkpoint import MODEL, write_checkpoint, write_network_checkpoint
from hypertrain.aggregator.core import MalformedDelta, OuterState, load_delta
from hypertrain.auditor.replay import pack_state, unpack_state
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.jcs import canonicalize
from hypertrain.trainer.compress import compress
from hypertrain.trainer.config import CompressConfig
from hypertrain.trainer.optim import OptState


def arrays() -> dict[str, np.ndarray]:
    return {
        "log_temp.choice": np.array(0.25, dtype=np.float32),
        "log_temp.noul": np.array(-0.5, dtype=np.float32),
        "log_temp.score": np.array(1.5, dtype=np.float32),
        "vector": np.array([0.25], dtype=np.float32),
        "matrix": np.arange(12, dtype=np.float32).reshape(3, 4)[:, ::2],
    }


def equal(expected: dict[str, np.ndarray], actual: dict[str, np.ndarray]) -> None:
    assert expected.keys() == actual.keys()
    for name, value in expected.items():
        assert actual[name].shape == value.shape
        assert actual[name].dtype == value.dtype
        np.testing.assert_array_equal(actual[name], value)


def test_outer_state_roundtrip_preserves_all_parts_scalar_and_vector() -> None:
    # Given: real OD names, scalar temperatures, true [1] vector, noncontiguous matrix.
    values = arrays()
    state = OuterState(
        values,
        {n: np.array(x * 2, dtype=np.float32) for n, x in values.items()},
        {n: np.array(x * 3, dtype=np.float32) for n, x in values.items()},
    )
    # When
    raw = state.to_bytes()
    restored = OuterState.from_bytes(raw)
    # Then: theta, momentum and CClip center all preserve rank/dtype/value exactly.
    for part in ("theta", "u", "center"):
        equal(getattr(state, part), getattr(restored, part))
    assert restored.to_bytes() == raw
    assert restored.hashes() == state.hashes()


def test_nonscalar_serialization_bytes_match_preexisting_encoding() -> None:
    # Given: old encoding is the compatibility oracle for non-scalars only.
    values = {n: a for n, a in arrays().items() if a.shape}
    state = OuterState.init(values)
    legacy = {
        f"{part}/{name}": np.ascontiguousarray(value, dtype="<f4")
        for part in ("theta", "u", "center")
        for name, value in sorted(getattr(state, part).items())
    }
    # When / Then
    assert state.to_bytes() == st_save(legacy)


def test_real_delta_validator_keeps_scalar_and_vector_distinct(tmp_path: Path) -> None:
    store = LocalFSStore(tmp_path)
    theta = {"log_temp.choice": np.array(0.25, dtype=np.float32)}
    payload, _ = compress(
        CompressConfig("dense-int8", ef_beta=0),
        {n: torch.from_numpy(a) for n, a in theta.items()},
        {n: torch.zeros((), dtype=torch.float32) for n in theta},
    )
    key = store.put(payload)
    restored = OuterState.from_bytes(OuterState.init(theta).to_bytes())
    assert load_delta(store, key, restored.theta)["log_temp.choice"].shape == ()
    with pytest.raises(MalformedDelta, match="shape"):
        load_delta(store, key, {"log_temp.choice": np.array([0.25], dtype=np.float32)})


def test_full_training_state_and_ef_roundtrip_preserves_temperatures() -> None:
    # Existing pack/unpack path already carries theta/m/v/step and separate full EF.
    theta = {n: torch.from_numpy(x.copy()) for n, x in arrays().items()}
    optimizer = OptState(
        {n: x.clone() * 2 for n, x in theta.items()},
        {n: x.clone().square() for n, x in theta.items()},
        7,
    )
    ef = {n: x.clone() * 0.01 for n, x in theta.items()}
    loaded, state = unpack_state(pack_state(theta, optimizer))
    residual, absent = unpack_state(pack_state(ef))
    assert state is not None and state.step == 7 and absent is None
    for expected, actual in (
        (theta, loaded),
        (optimizer.m, state.m),
        (optimizer.v, state.v),
        (ef, residual),
    ):
        equal(
            {n: x.numpy() for n, x in expected.items()}, {n: x.numpy() for n, x in actual.items()}
        )


def test_ordinary_checkpoint_preserves_scalar_model(tmp_path: Path) -> None:
    from agg_helpers import COORD

    tape = {"body": {"kind": "global", "run_id": "11" * 32, "w": 0, "inputs": []}}
    write_checkpoint(
        tmp_path / "scalar", COORD, arrays(), [tape], license="Apache-2.0", dataset={"name": "tiny"}
    )
    equal(arrays(), st_load((tmp_path / "scalar" / MODEL).read_bytes()))
    non_scalar = {n: a for n, a in arrays().items() if a.shape}
    write_checkpoint(
        tmp_path / "non-scalar",
        COORD,
        non_scalar,
        [tape],
        license="Apache-2.0",
        dataset={"name": "tiny"},
    )
    assert (tmp_path / "non-scalar" / MODEL).read_bytes() == st_save(
        {n: np.ascontiguousarray(a, dtype="<f4") for n, a in non_scalar.items()}
    )


def test_network_checkpoint_scalar_model_seam(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Only serialization seam isolated; real replay arithmetic has its own tiny controls."""
    from types import SimpleNamespace

    from test_weighted_v2 import KEY, economics, fixture, policy

    import hypertrain.aggregator.checkpoint as checkpoint
    from hypertrain.protocol.messages_v2 import PolicyHashes

    store = LocalFSStore(tmp_path / "objects")
    manifest, works, previous = fixture(store)
    for raw in (canonicalize(policy().body()), canonicalize(economics().body())):
        store.put(raw)
    state = OuterState.init(arrays())
    output = store.put(state.to_bytes())
    tape = SimpleNamespace(
        body=SimpleNamespace(
            w=0, out_state=output, out_hashes=state.hashes(), run_id=manifest.run_id()
        )
    )
    final = seal(
        KEY,
        "Finalize",
        manifest.run_id(),
        {
            "w": 0,
            "final_theta_hash_w1": state.hashes()["theta_hash"],
            "included": [],
            "entitlements_root": "11" * 32,
        },
        100,
    )
    opening = seal(
        KEY,
        "RoundOpenV2",
        manifest.run_id(),
        {
            "w": 0,
            "roster": [work.roster.body() for work in works],
            "theta_hash": state.hashes()["theta_hash"],
            "prev_final_hash": "0" * 64,
            "outer_state_hash": "0" * 64,
            "center_hash": "0" * 64,
            "roster_hash": "0" * 64,
            "honeypot_commit": "0" * 64,
            "d_open": 1,
            "d_assign": 2,
            "d_commit": 3,
            "d_audit": 4,
            "d_upload": 5,
            "d_final": 6,
            "contract_version": 2,
            "registry_epoch": 0,
            "policy_hashes": {n: getattr(manifest.network, n) for n in PolicyHashes.model_fields},
            "start_state_index_hash": "0" * 64,
            "audit_mode": "anchored-full",
        },
        100,
    )
    item = {
        "tape_hash": output,
        "prev_state": previous,
        "predecessor_tape_hash": "0" * 64,
        "inputs": [],
        "round_open": opening,
        "finalize": final,
    }
    monkeypatch.setattr(checkpoint, "_network_tape", lambda *args: tape)
    monkeypatch.setattr(checkpoint, "_network_subjects", lambda *args: None)
    monkeypatch.setattr(checkpoint, "require_network_roster", lambda *args: None)
    monkeypatch.setattr(checkpoint, "_network_replay", lambda *args: (state, None))
    write_network_checkpoint(tmp_path / "network", KEY, store, manifest, [item])
    equal(arrays(), st_load((tmp_path / "network" / MODEL).read_bytes()))
