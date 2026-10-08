import subprocess
import sys
from pathlib import Path

import pytest
from pydantic import ValidationError

from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import (
    QUICKNET_GENESIS,
    SCHEMA_MODELS,
    Bisect,
    LeafPreimage,
    RoundOpen,
    RunManifest,
    StageState,
    Timeouts,
    VerifySpec,
    default_verify,
    drand_round_at,
    f32hex,
    vesting_rounds,
)
from hypertrain.protocol.schema import render

ROOT = Path(__file__).resolve().parents[2]
SCHEMAS = ROOT / "docs" / "schemas"
T = Timeouts(
    assign_after_open=20,
    audit_after_commit=40,
    upload_after_commit=300,
    serve_deadline=900,
    dispute_per_level=600,
)


def test_run_id_is_sha256_of_jcs_body() -> None:
    m = example_manifest()
    assert m.run_id() == sha256_hex(canonicalize(m.model_dump(mode="json")))
    reparsed = RunManifest.model_validate_json(m.model_dump_json())
    assert reparsed.run_id() == m.run_id()


def test_run_id_changes_with_any_field() -> None:
    m = example_manifest()
    body = m.body()
    body["outer"] = {**body["outer"], "lr": f32hex(0.6)}  # type: ignore[dict-item]
    assert RunManifest.model_validate(body).run_id() != m.run_id()


def test_operator_budget_present_and_required() -> None:
    body = example_manifest().body()
    assert set(body["operator_budget"]) == {"relay", "auditor", "honeypot"}  # type: ignore[arg-type]
    del body["operator_budget"]
    with pytest.raises(ValidationError):
        RunManifest.model_validate(body)


def test_manifest_rejects_unknown_fields_and_raw_floats() -> None:
    body = example_manifest().body()
    with pytest.raises(ValidationError):
        RunManifest.model_validate({**body, "surprise": 1})
    body["outer"] = {**body["outer"], "lr": 0.4}  # type: ignore[dict-item]
    with pytest.raises(ValidationError):
        RunManifest.model_validate(body)


def test_drand_round_mapping() -> None:
    g = QUICKNET_GENESIS
    assert [drand_round_at(g + d, g) for d in (0, 2, 3, 5, 6)] == [1, 1, 2, 2, 3]
    assert example_manifest().drand_round_at(g + 3000) == 1001
    with pytest.raises(ValueError):
        drand_round_at(g - 1, g)


def test_verify_defaults_e_probation_smin() -> None:
    v = default_verify(0.1, "00" * 32, T)
    assert (v.E_vest_rounds, v.probation_rounds, v.s_min_reward_multiple) == (10, 10, 10)
    assert v.s_min_units(250_000) == 2_500_000
    assert vesting_rounds(0.05) == 20 and vesting_rounds(1.0) == 1
    with pytest.raises(ValidationError):
        default_verify(0.1, "00" * 32, T, E_vest_rounds=9)


def test_audit_skew_margin_enforced() -> None:
    with pytest.raises(ValidationError):
        Timeouts(
            assign_after_open=20,
            audit_after_commit=39,
            upload_after_commit=300,
            serve_deadline=900,
            dispute_per_level=600,
        )
    assert isinstance(default_verify(0.1, "00" * 32, T), VerifySpec)


def test_bisect_needs_n_plus_one_hashes() -> None:
    ok = {
        "dispute_id": "aa" * 32,
        "level": "step",
        "interval": [0, 30],
        "N": 2,
        "party": example_manifest().coord_pubkey,
    }
    Bisect.model_validate({**ok, "hashes": ["bb" * 32] * 3})
    with pytest.raises(ValidationError):
        Bisect.model_validate({**ok, "hashes": ["bb" * 32] * 2})


def test_leaf_digest_fixed_width_and_sensitive() -> None:
    leaf = LeafPreimage(
        run_id="ab" * 32,
        w=1,
        t=5,
        stages=[StageState(theta="01" * 32, m="02" * 32, v="03" * 32)],
        batch_ids_sha256="04" * 32,
        rng_ctr=5,
        loss_f32=f32hex(2.5),
        norm_f32=f32hex(0.125),
    )
    d = leaf.digest()
    assert d == leaf.model_copy().digest()
    assert leaf.model_copy(update={"t": 10}).digest() != d
    assert leaf.model_copy(update={"loss_f32": f32hex(2.25)}).digest() != d


def test_schema_files_match_and_regenerate_byte_identically(tmp_path: Path) -> None:
    on_disk = {p.name: p.read_bytes() for p in SCHEMAS.glob("*.json")}
    assert on_disk == render()
    assert set(f"{n}.json" for n in SCHEMA_MODELS) <= set(on_disk)
    out = tmp_path / "schemas"
    subprocess.run(
        [sys.executable, "-m", "hypertrain.protocol.schema", str(out)],
        check=True,
        capture_output=True,
    )
    assert {p.name: p.read_bytes() for p in out.glob("*.json")} == on_disk


ROUND = {
    "w": 1,
    "prev_final_hash": "01" * 32,
    "theta_hash": "02" * 32,
    "outer_state_hash": "03" * 32,
    "center_hash": "04" * 32,
    "roster": [],
    "roster_hash": "05" * 32,
    "honeypot_commit": "06" * 32,
    "d_open": 1,
    "d_assign": 21,
    "d_commit": 100,
    "d_audit": 140,
    "d_upload": 400,
    "d_final": 500,
}


def test_round_open_valid_order() -> None:
    RoundOpen.model_validate(ROUND)
    RoundOpen.model_validate({**ROUND, "d_upload": 500})


@pytest.mark.parametrize(
    "bad",
    [
        {"d_upload": 5},
        {"d_assign": 120},
        {"d_audit": 50},
        {"d_open": 21},
        {"d_upload": 501},
        {"d_final": 140},
        {"d_upload": 100},
        {"w": True},
    ],
)
def test_round_open_rejects_bad_order(bad: dict[str, object]) -> None:
    with pytest.raises(ValidationError):
        RoundOpen.model_validate({**ROUND, **bad})
