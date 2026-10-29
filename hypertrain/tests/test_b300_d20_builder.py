"""scripts/b300/d20/build_test_manifest.sh: argument checks, a failed build, and --clean.

The builder runs its Python stages as `uv run --no-sync python -I ...`. A uv shim on PATH runs them
with this venv's python (no uv, no network) and can fail on the Nth call, so a failure after the
first stage is injected without touching the builder; it also keeps the training body that the
manifest stage is given. HT_D20_GEOMETRY=tiny-cpu keeps a full build to a few seconds; the relay key
is a throwaway public address made here. The dense-1b geometry is built only up to its first stage:
its manifest stage would initialise 889M parameters. The dense-0.1b geometry (100,676,352 params)
is built in full and read back the way the master's run creation reads it, and so are the
D94 run profile dense-0.03b (31,462,656 params) and its larger option dense-0.05b (44,045,568
params, ADR 0016).
"""

from __future__ import annotations

import json
import math
import os
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from hypertrain.data.store import LocalFSStore
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.env_pins import EnvPin
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    AuditPolicyV2,
    EconomicsPolicyV2,
    RunManifestV2,
    reaudit_rounds_for,
)
from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.model import param_shapes

ROOT = Path(__file__).resolve().parents[1]
BUILDER = ROOT / "scripts/b300/d20/build_test_manifest.sh"
DIGEST = "sha256:" + "cd" * 32

# The training.json model fields the measured dense builds take from their registry row.
ROW_FIELDS = (
    "arch",
    "n_layers",
    "d_model",
    "n_heads",
    "n_kv_heads",
    "d_ff",
    "n_experts",
    "top_k_experts",
    "vocab",
    "seq_len",
    "compute_dtype",
    "param_count",
)

# `uv run --no-sync python ARGS` is the only call the builder makes; the Nth can be made to fail.
FAKE_UV = r"""#!/usr/bin/env bash
state=$FAKE_STATE
n=$(($(cat "$state/uv.calls" 2>/dev/null || echo 0) + 1)); echo "$n" >"$state/uv.calls"
if [ "$1 $2 $3" != "run --no-sync python" ]; then
  echo "uv $*" >>"$state/tripwire"; echo "unexpected uv call: $*" >&2; exit 125
fi
prev=
for arg in "$@"; do
  # A failed build removes its inputs: keep the training body of the manifest stage to read.
  if [ "$prev" = --training ]; then cp -- "$arg" "$state/training.json"; fi
  prev=$arg
done
if [ "$n" = "${FAKE_UV_FAIL_ON:-0}" ]; then echo "injected uv failure" >&2; exit 9; fi
if [ "$n" = "${FAKE_UV_HANG_ON:-0}" ]; then echo $$ >"$state/uv.pid"; exec sleep 60; fi
shift 3
exec "$FAKE_PYTHON" "$@"
"""

# The builder is offline: anything that would leave the host is a failure of the test.
TRIPWIRE = r"""#!/usr/bin/env bash
echo "$(basename "$0") $*" >>"$FAKE_STATE/tripwire"
echo "unexpected call: $(basename "$0") $*" >&2
exit 127
"""


@pytest.fixture
def host(tmp_path: Path) -> Iterator[dict[str, str]]:
    """The builder's environment: shims first on PATH, tiny-cpu geometry, a throwaway relay key."""
    bin_dir, state = tmp_path / "bin", tmp_path / "state"
    bin_dir.mkdir()
    state.mkdir()
    shims = {"uv": FAKE_UV}
    shims.update(dict.fromkeys(("curl", "wget", "ssh", "scp", "rsync", "sudo"), TRIPWIRE))
    for name, body in shims.items():
        shim = bin_dir / name
        shim.write_text(body)
        shim.chmod(0o755)
    yield {
        **{k: v for k, v in os.environ.items() if not k.startswith("HT_D20_")},
        "PATH": f"{bin_dir}:{os.environ['PATH']}",
        "FAKE_STATE": str(state),
        "FAKE_PYTHON": sys.executable,
        "HT_D20_GEOMETRY": "tiny-cpu",
        "HT_D20_COLDKEYS": "2",
        "HT_D20_RELAY_PUBKEY": Keypair(secrets.token_bytes(32)).ss58,
    }
    tripwire = state / "tripwire"
    assert not tripwire.exists(), tripwire.read_text()


def uv_calls(host: dict[str, str]) -> int:
    calls = Path(host["FAKE_STATE"]) / "uv.calls"
    return int(calls.read_text()) if calls.exists() else 0


def build(
    host: dict[str, str],
    out: Path,
    *,
    image: str = DIGEST,
    driver: str = "cpu-test",
    sm_count: str = "170",
    exp_round: str | None = None,
    env: dict[str, str] | None = None,
    builder: Path = BUILDER,
) -> subprocess.CompletedProcess[str]:
    args = [image, driver, sm_count, str(out), *([exp_round] if exp_round is not None else [])]
    return subprocess.run(
        [str(builder), *args],
        env={**host, **(env or {})},
        capture_output=True,
        text=True,
        timeout=300,
    )


def clean(
    host: dict[str, str], *args: str, cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    # No relay key and no uv call are needed to clean.
    env = {k: v for k, v in host.items() if k != "HT_D20_RELAY_PUBKEY"}
    return subprocess.run(
        [str(BUILDER), "--clean", *args],
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
        cwd=cwd,
    )


def tree(root: Path) -> list[str]:
    """Every path under root (symlinks listed, never followed)."""
    found = []
    for directory, names, files in os.walk(root, followlinks=False):
        found += [os.path.relpath(os.path.join(directory, n), root) for n in (*names, *files)]
    return sorted(found)


def seed_files(secrets_dir: Path) -> list[Path]:
    return sorted(secrets_dir.glob("*.seed"))


@pytest.mark.parametrize(
    ("changes", "named"),
    [
        ({"image": "sha256:xyz"}, "IMAGE_DIGEST"),
        ({"image": "sha256:" + "AB" * 32}, "IMAGE_DIGEST"),
        ({"driver": "595.91.07; touch /nope"}, "DRIVER"),
        ({"sm_count": "148;touch /nope"}, "SM_COUNT"),
        ({"exp_round": "12x"}, "EXP_ROUND"),
    ],
    ids=["digest_not_hex", "digest_upper_case", "driver", "sm_count", "exp_round"],
)
def test_builder_refuses_a_malformed_argument_before_it_creates_or_runs_anything(
    host, tmp_path, changes, named
):
    out = tmp_path / "out"
    run = build(host, out, **changes)
    assert run.returncode == 2 and named in run.stderr
    assert not out.exists() and uv_calls(host) == 0


@pytest.mark.parametrize("fail_on", ["2", "3"], ids=["manifest_build_fails", "dry_run_fails"])
def test_a_failure_after_the_first_stage_removes_everything_the_run_made(host, tmp_path, fail_on):
    out = tmp_path / "campaign" / "o"  # neither exists: the builder makes both
    # uv call 1 writes the seeds and the inputs, call 2 the manifest (out/ and state/), call 3 is
    # the dry run: after either failure nothing of the run is left.
    run = build(host, out, env={"FAKE_UV_FAIL_ON": fail_on})
    assert uv_calls(host) == int(fail_on) and run.returncode == 9
    assert "removed what this run created" in run.stderr
    assert not (tmp_path / "campaign").exists()


def test_a_failure_keeps_the_seeds_and_the_directories_that_were_there_before(host, tmp_path):
    out = tmp_path / "out"
    secrets_dir = out / "secrets"
    secrets_dir.mkdir(parents=True, mode=0o700)
    staged = secrets.token_hex(32)
    (secrets_dir / "owner.seed").write_text(staged)
    (secrets_dir / "owner.seed").chmod(0o600)
    (out / "notes.txt").write_text("operator notes")

    run = build(host, out, env={"FAKE_UV_FAIL_ON": "2"})

    assert uv_calls(host) == 2 and run.returncode == 9
    assert tree(out) == ["notes.txt", "secrets", "secrets/owner.seed"]
    assert (secrets_dir / "owner.seed").read_text() == staged
    assert stat.S_IMODE((secrets_dir / "owner.seed").stat().st_mode) == 0o600


@pytest.mark.parametrize(
    ("existing", "left"),
    [(".", []), ("secrets", ["secrets"])],
    ids=["empty_out_dir", "empty_secrets_dir"],
)
def test_a_failure_leaves_an_empty_directory_that_was_already_there(host, tmp_path, existing, left):
    out = tmp_path / "out"
    (out / existing).mkdir(parents=True, exist_ok=True)

    run = build(host, out, env={"FAKE_UV_FAIL_ON": "2"})

    assert uv_calls(host) == 2 and run.returncode == 9
    assert out.is_dir() and tree(out) == left


def test_a_dangling_symlink_where_the_builder_writes_is_refused_and_left_alone(host, tmp_path):
    out = tmp_path / "out"
    out.mkdir()
    (out / "out").symlink_to(tmp_path / "nowhere")

    run = build(host, out)

    assert run.returncode == 2 and "already exists" in run.stderr
    assert uv_calls(host) == 0 and tree(out) == ["out"] and (out / "out").is_symlink()


def test_a_build_stopped_by_sigterm_while_it_runs_removes_what_it_made(host, tmp_path):
    # bash runs its EXIT trap on SIGTERM (and on an ssh drop, SIGHUP) with $? still 0.
    out = tmp_path / "campaign" / "o"
    hung = Path(host["FAKE_STATE"]) / "uv.pid"
    proc = subprocess.Popen(
        [str(BUILDER), DIGEST, "cpu-test", "170", str(out)],
        env={**host, "FAKE_UV_HANG_ON": "2"},
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.monotonic() + 120
        while not hung.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        assert hung.exists() and seed_files(out / "secrets")  # stage one is done, the build hangs
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=30)
        assert not (tmp_path / "campaign").exists()
    finally:
        # The shim's sleep is a child of the builder, not killed with it.
        if hung.exists():
            try:
                os.kill(int(hung.read_text()), signal.SIGKILL)
            except ProcessLookupError:  # already gone
                pass
        proc.kill()
        proc.wait(timeout=10)


def test_a_good_build_keeps_private_seeds_and_the_outputs_and_prints_no_seed(host, tmp_path):
    out = tmp_path / "out"

    run = build(host, out)

    assert run.returncode == 0, run.stderr
    assert uv_calls(host) == 3
    assert stat.S_IMODE((out / "secrets").stat().st_mode) == 0o700
    seeds = seed_files(out / "secrets")
    assert sorted(p.name for p in seeds) == sorted(
        f"{role}.seed"
        for role in ("owner", "coord", "auditor-0", "auditor-1", "referee", "cold-0", "cold-1")
    )
    assert {stat.S_IMODE(p.stat().st_mode) for p in seeds} == {0o600}
    assert (out / "inputs/training.json").is_file() and (
        out / "out/manifest-envelope.json"
    ).is_file()
    assert (out / "state").is_dir()
    policies = json.loads((out / "inputs/policies.json").read_text())
    assert (
        policies["admission"]["work_screen_epoch_rounds"] == 6000
    )  # only the dense TEST models widen it
    assert not {"reaudit_rounds", "gap_cap_rounds"} & policies["audit"].keys()  # nor sets these
    written = [
        p.read_bytes()
        for d in ("inputs", "out", "state")
        for p in (out / d).rglob("*")
        if p.is_file()
    ]
    for seed in seeds:
        value = seed.read_text()
        assert value not in run.stdout + run.stderr
        assert not any(value.encode() in content for content in written)


def stage_one_training(
    host: dict[str, str], out: Path, sm_count: str, env: dict[str, str] | None = None
) -> dict[str, Any]:
    """The training body the first stage of a dense-1b build hands to the manifest stage."""
    run = build(
        host,
        out,
        driver="595.91.07",
        sm_count=sm_count,
        env={"HT_D20_GEOMETRY": "dense-1b", "FAKE_UV_FAIL_ON": "2", **(env or {})},
    )
    assert uv_calls(host) == 2 and run.returncode == 9, run.stderr
    body = json.loads((Path(host["FAKE_STATE"]) / "training.json").read_bytes())
    assert isinstance(body, dict)
    return body


@pytest.mark.parametrize(
    ("sm_count", "attention", "gpu"),
    [
        ("148", None, "sm_103"),
        ("170", None, "sm_120"),
        ("148", "sdpa-flash", "sm_103"),
    ],
    ids=["sm_103", "sm_120", "sm_103_flash"],
)
def test_a_gpu_manifest_pins_the_class_measured_on_its_sm_count(
    host, tmp_path, sm_count, attention, gpu
):
    env = {} if attention is None else {"HT_D20_ATTN_BACKEND": attention}
    spec = stage_one_training(host, tmp_path / "o", sm_count, env)["reference_spec"]

    pin = EnvPin.from_body(spec["env"]["pinned"])  # a valid pin: exactly the table's rows
    assert spec["sm_count"] == int(sm_count)
    assert (pin.cpu, pin.gpu) == ("avx2", gpu)
    assert dict(pin.present)["HT_ATTN_BACKEND"] == (attention or "math")


@pytest.mark.parametrize(
    ("env", "sm_count", "named"),
    [
        ({"HT_D20_GEOMETRY": "dense-1b"}, "132", "SM_COUNT 132: no GPU class is measured on it"),
        ({"HT_D20_GEOMETRY": "dense-1b"}, "0", "SM_COUNT 0: no GPU class is measured on it"),
        ({"HT_D20_ATTN_BACKEND": "flash-attn"}, "148", "HT_D20_ATTN_BACKEND=flash-attn"),
        # A CPU run states "no GPU": the table has no flash backend for it.
        ({"HT_D20_ATTN_BACKEND": "sdpa-flash"}, "170", "HT_D20_ATTN_BACKEND=sdpa-flash"),
    ],
    ids=["unknown_sm_count", "zero_sm_count", "unmeasured_backend", "cpu_run_with_flash"],
)
def test_a_pin_no_gpu_class_supports_stops_the_first_stage_and_leaves_nothing(
    host, tmp_path, env, sm_count, named
):
    out = tmp_path / "campaign" / "o"

    run = build(host, out, sm_count=sm_count, env=env)

    assert run.returncode == 1 and named in run.stderr
    assert "removed what this run created" in run.stderr
    assert uv_calls(host) == 1 and not (tmp_path / "campaign").exists()


def test_a_cpu_build_pins_gpu_none_and_the_dry_run_reports_the_pin(host, tmp_path):
    out = tmp_path / "out"

    run = build(host, out)  # tiny-cpu, with the SM count of the RTX 5090 carried unmeasured

    assert run.returncode == 0, run.stderr
    pinned = json.loads((out / "inputs/training.json").read_bytes())["reference_spec"]["env"][
        "pinned"
    ]
    assert (pinned["cpu"], pinned["gpu"]) == ("avx2", "none")
    report = json.loads(run.stdout[run.stdout.rindex("\n{\n") + 1 :])
    assert report["pin"] == {"cpu": "avx2", "gpu": "none", "attention": "math"}
    checks = report["checks"]
    assert [
        checks[k] for k in ("pin_cpu_class_avx2", "pin_gpu_class_none", "pin_attention_backend")
    ] == [
        True,
        True,
        True,
    ]


def accepted_manifest(out: Path) -> tuple[RunManifestV2, AuditPolicyV2]:
    """The built run as `POST /v2/admin/runs` takes it (store._create_run_v2 minus the database):
    the owner-signed envelope through the intake, the run id it names, and the audit policy object
    it pins, validated against the training."""
    owner = json.loads((out / "inputs/public-keys.json").read_bytes())["owner"]
    raw = (out / "out/manifest-envelope.json").read_bytes()
    env = envelope_v2.parse_envelope(raw)
    manifest = envelope_v2.Intake(env.run_id, {"RunManifestV2": owner.__eq__}).accept(raw, 1)
    assert isinstance(manifest, RunManifestV2)
    assert (
        manifest.run_id()
        == env.run_id
        == json.loads((out / "out/index.json").read_bytes())["run_id"]
    )
    objects = LocalFSStore(out / "state/objects")
    economics = EconomicsPolicyV2.model_validate_json(
        objects.get(manifest.network.economics_policy_hash)
    )
    audit = AuditPolicyV2.model_validate_json(objects.get(manifest.network.audit_policy_hash))
    assert audit.digest() == manifest.network.audit_policy_hash
    audit.validate_training(manifest.training, ledger_mode=economics.ledger_mode)
    return manifest, audit


def production_build(out: Path, scratch: Path) -> subprocess.CompletedProcess[str]:
    """The build's own inputs given to its manifest stage (`ops/manifest_v2.py build`) again, with
    the economics policy turned into a production one (which pins no genesis allocation)."""
    policies = json.loads((out / "inputs/policies.json").read_bytes())
    policies["economics"].update(ledger_mode="production", genesis_allocation_hash=None)
    (scratch / "policies.json").write_text(json.dumps(policies))
    inputs = out / "inputs"
    return subprocess.run(
        [
            *(sys.executable, "-I", "-m", "hypertrain.ops.manifest_v2", "build"),
            *("--training", str(inputs / "training.json")),
            *("--policies", str(scratch / "policies.json")),
            *("--relays", str(inputs / "relays.json")),
            *("--owner-seed", str(out / "secrets/owner.seed")),
            *("--exp-round", (inputs / "exp-round.txt").read_text().strip()),
            *("--out", str(scratch / "out")),
        ],
        capture_output=True,
        text=True,
        timeout=300,
    )


@pytest.mark.parametrize(
    ("replay", "reaudit", "gap_needed", "cap"),
    [(144, 100, 175, None), (256, 171, 276, 276)],
    ids=["replay_144s_fits_the_default_cap", "replay_256s_raises_the_cap"],
)
def test_a_dense_0_1b_build_writes_the_registry_row_and_a_wide_screen_window(
    host, tmp_path, replay, reaudit, gap_needed, cap
):
    out = tmp_path / "out"

    run = build(
        host,
        out,
        driver="595.91.07",
        sm_count="148",
        env={"HT_D20_GEOMETRY": "dense-0.1b", "HT_D20_REPLAY_SECONDS": str(replay)},
    )

    assert run.returncode == 0, run.stderr
    assert uv_calls(host) == 3
    training = json.loads((out / "inputs/training.json").read_text())
    assert {k: v for k, v in training["model"].items() if k in ROW_FIELDS} == {
        "arch": "decoder",
        "n_layers": 8,
        "d_model": 768,
        "n_heads": 12,
        "n_kv_heads": 4,
        "d_ff": 2048,
        "n_experts": 1,
        "top_k_experts": 1,
        "vocab": 32768,
        "seq_len": 2048,
        "compute_dtype": "bf16",
        "param_count": 100_676_352,
    }
    inner = training["inner"]
    assert {k: inner[k] for k in ("H", "J", "micro_batch", "grad_accum", "opt")} == {
        "H": 4,
        "J": 4,
        "micro_batch": 2,
        "grad_accum": 1,
        "opt": "adamw",
    }
    assert (inner["state_policy"], inner["rewarmup_steps"]) == ("reset", 0)  # the 8 P job inputs
    assert inner["lr_schedule"] == {
        "type": "wsd",
        "peak_lr": f32hex(3e-3),
        "warmup": 2,
        "stable": 100000,
        "decay": 0,
    }
    outer = training["outer"]  # dense-int8 at full density: the 1.016 P delta of the budget
    assert (outer["opt"], outer["bits"], outer["topk_frac"], outer["ef_beta"]) == (
        "nesterov",
        8,
        f32hex(1.0),
        f32hex(0.0),
    )
    dataset = training["dataset"]
    samples = (out / "inputs/samples.u32").read_bytes()
    rows = np.frombuffer(samples, dtype="<u4").reshape(4096, 2049)
    assert dataset["n_samples"] == 4096 and dataset["depth"] == 12
    assert rows.max() < 32768  # every id is inside the vocab
    assert dataset["merkle_root"] == MerkleTree([r.tobytes() for r in rows]).root.hex()
    assert (
        dataset["shard_sha256_root"] == MerkleTree([bytes.fromhex(sha256_hex(samples))]).root.hex()
    )
    assert training["reference_spec"]["layout"]["n_gpus"] == 1
    assert training["reference_spec"]["driver_allowlist"] == ["595.91.07"]
    policies = json.loads((out / "inputs/policies.json").read_text())
    assert policies["admission"]["work_screen_epoch_rounds"] == 20000
    assert (
        policies["audit"]["lease_rounds"] == 100 and policies["audit"]["max_steps_per_attempt"] == 8
    )
    assert '"param_count": 100676352' in run.stdout
    # ADR 0011 Part 9 decision 12 (D84, D88): twice the measured replay at 3 s a round, floored at
    # the lease; the cap is raised to the gap that window needs only when 200 rounds cannot hold it.
    manifest, audit = accepted_manifest(out)
    assert (audit.lease_rounds, audit.reaudit_rounds, audit.gap_cap_rounds) == (100, reaudit, cap)
    pin = manifest.training.reference_spec.env.pinned
    assert pin is not None and (pin.cpu, pin.gpu) == ("avx2", "sm_103")  # 148 SM: the B300 class
    report = json.loads(run.stdout[run.stdout.rindex("\n{\n") + 1 :])
    assert report["audit"] == {
        "lease_rounds": 100,
        "reaudit_rounds": reaudit,
        "gap_needed": gap_needed,
        "gap_cap": cap or 200,
        "replay_seconds": replay,
        "warmup_replay_seconds": 300,
        "stop_above_seconds": 298,
    }
    assert report["warmup_run_id"] is None
    assert all(report["checks"].values()) and "gap_cap_holds_the_reaudit_window" in report["checks"]
    # D88: the same inputs over a production ledger publish only while the cap stays at 200.
    scratch = tmp_path / "production"
    scratch.mkdir()
    production = production_build(out, scratch)
    if cap is None:
        assert production.returncode == 0, production.stderr
    else:
        assert production.returncode == 2 and not (scratch / "out").exists()
        assert "gap_cap_rounds above 200 is for a TEST manifest only" in production.stderr


@pytest.mark.parametrize(
    ("geometry", "vocab", "count", "replay", "reaudit", "gap_needed", "cap"),
    [
        ("dense-0.05b", 16384, 44_045_568, 120, 100, 175, None),
        ("dense-0.05b", 16384, 44_045_568, 200, 134, 226, 226),
        ("dense-0.03b", 8192, 31_462_656, 144, 100, 175, None),
    ],
    ids=["50m_replay_120s_fits_the_default_cap", "50m_replay_200s_raises_the_cap", "run_profile"],
)
def test_a_d94_build_writes_its_own_row_with_the_dense_0_1b_loop_and_windows(
    host, tmp_path, geometry, vocab, count, replay, reaudit, gap_needed, cap
):
    out = tmp_path / "out"

    run = build(
        host,
        out,
        driver="595.91.07",
        sm_count="148",
        env={"HT_D20_GEOMETRY": geometry, "HT_D20_REPLAY_SECONDS": str(replay)},
    )

    assert run.returncode == 0, run.stderr
    training = json.loads((out / "inputs/training.json").read_text())
    assert {k: v for k, v in training["model"].items() if k in ROW_FIELDS} == {
        "arch": "decoder",
        "n_layers": 3,
        "d_model": 768,
        "n_heads": 12,
        "n_kv_heads": 4,
        "d_ff": 2048,
        "n_experts": 1,
        "top_k_experts": 1,
        "vocab": vocab,
        "seq_len": 2048,
        "compute_dtype": "bf16",
        "param_count": count,
    }
    inner = training["inner"]
    assert {k: inner[k] for k in ("H", "J", "micro_batch", "grad_accum", "opt")} == {
        "H": 4,
        "J": 4,
        "micro_batch": 2,
        "grad_accum": 1,
        "opt": "adamw",
    }
    assert (inner["state_policy"], inner["rewarmup_steps"]) == ("reset", 0)
    samples = (out / "inputs/samples.u32").read_bytes()
    rows = np.frombuffer(samples, dtype="<u4").reshape(4096, 2049)
    assert training["dataset"]["n_samples"] == 4096 and rows.max() == vocab - 1
    assert training["dataset"]["merkle_root"] == MerkleTree([r.tobytes() for r in rows]).root.hex()
    assert training["reference_spec"]["layout"]["n_gpus"] == 1
    spec = json.loads((out / "inputs/reference-spec.json").read_text())
    assert spec["qualification_profile"] == f"{geometry}-bf16"
    assert spec["geometry_source"] == f"scripts/mfu_profiles.json mfu-{geometry}; ADR 0016"
    policies = json.loads((out / "inputs/policies.json").read_text())
    assert policies["admission"]["work_screen_epoch_rounds"] == 20000
    assert f'"param_count": {count}' in run.stdout
    manifest, audit = accepted_manifest(out)
    assert (audit.lease_rounds, audit.reaudit_rounds, audit.gap_cap_rounds) == (100, reaudit, cap)
    pin = manifest.training.reference_spec.env.pinned
    assert pin is not None and (pin.cpu, pin.gpu) == ("avx2", "sm_103")
    report = json.loads(run.stdout[run.stdout.rindex("\n{\n") + 1 :])
    assert report["audit"] == {
        "lease_rounds": 100,
        "reaudit_rounds": reaudit,
        "gap_needed": gap_needed,
        "gap_cap": cap or 200,
        "replay_seconds": replay,
        "warmup_replay_seconds": 300,
        "stop_above_seconds": 298,
    }
    assert all(report["checks"].values()) and "gap_cap_holds_the_reaudit_window" in report["checks"]


def test_the_d94_runs_differ_from_the_101m_run_built_from_the_same_seeds_only_by_their_training(
    host, tmp_path
):
    # Same seeds, image, expiry and replay: the policies, the relays and so the network block are
    # those of dense-0.1b; the model, its init state and its fixture make another run id.
    args = {"driver": "595.91.07", "sm_count": "148", "exp_round": "34310319"}
    geometries = ("dense-0.1b", "dense-0.05b", "dense-0.03b")
    outs = [tmp_path / geometry for geometry in geometries]
    for i, geometry in enumerate(geometries):
        if i:
            (outs[i] / "secrets").mkdir(parents=True, mode=0o700)
            for seed in seed_files(outs[0] / "secrets"):
                shutil.copy(seed, outs[i] / "secrets" / seed.name)
        env = {"HT_D20_GEOMETRY": geometry, "HT_D20_REPLAY_SECONDS": "144"}
        run = build(host, outs[i], env=env, **args)
        assert run.returncode == 0, run.stderr

    assert len({(out / "inputs/policies.json").read_bytes() for out in outs}) == 1
    manifests = [json.loads((out / "out/manifest.json").read_bytes()) for out in outs]
    assert manifests[0]["network"] == manifests[1]["network"] == manifests[2]["network"]
    trainings = [m["training"] for m in manifests]
    assert [t["model"]["param_count"] for t in trainings] == [100_676_352, 44_045_568, 31_462_656]
    assert [t["init_state_hash"] for t in trainings] == [
        "be9b7bd63f79e4d7a3e640f51e9cfcc03e445a9de9ab249bcf77d938b8ee28f8",
        "9cbe3ee93068dbf7cf08791f02bd3bd07e5181fb3bc13efa4396b4ade5eaf897",
        "e372a68edb6485fce5f3df4efa9d72705fb04bbf7fd130dc3805d84075bf0aa6",
    ]
    assert len({accepted_manifest(out)[0].run_id() for out in outs}) == 3


def two_builds(
    host: dict[str, str], tmp_path: Path, replay: str, geometry: str = "dense-0.1b"
) -> tuple[Path, Path, subprocess.CompletedProcess[str], subprocess.CompletedProcess[str]]:
    """The runbook's two builds over the same seeds, image and expiry: the throw-away warm-up run at
    the 300 s ceiling, then the real run from the measured `replay`, told the warm-up's OUT_DIR."""
    dense = {"HT_D20_GEOMETRY": geometry}
    warmup, real = tmp_path / "warmup", tmp_path / "real"
    args = {"driver": "595.91.07", "sm_count": "148", "exp_round": "34310319"}
    first = build(host, warmup, env={**dense, "HT_D20_REPLAY_SECONDS": "300"}, **args)
    assert first.returncode == 0, first.stderr
    (real / "secrets").mkdir(parents=True, mode=0o700)
    for seed in seed_files(warmup / "secrets"):
        shutil.copy(seed, real / "secrets" / seed.name)
    env = {**dense, "HT_D20_REPLAY_SECONDS": replay, "HT_D20_WARMUP_OUT": str(warmup)}
    return warmup, real, first, build(host, real, env=env, **args)


def test_the_rebuild_with_the_measured_replay_changes_only_the_audit_window(host, tmp_path):
    # 298 s, the last replay whose window (199 rounds) is not the warm-up's (200). Only the audit
    # policy, and so the run id, may differ, or the warm-up would not have measured the run that is
    # published.
    warmup, real, first, second = two_builds(host, tmp_path, "298")
    assert second.returncode == 0, second.stderr

    def read(out: Path, name: str) -> dict[str, Any]:
        body = json.loads((out / name).read_bytes())
        assert isinstance(body, dict)
        return body

    assert (warmup / "inputs/training.json").read_bytes() == (
        real / "inputs/training.json"
    ).read_bytes()
    policies = [read(out, "inputs/policies.json") for out in (warmup, real)]
    windows = [
        {k: p["audit"].pop(k, None) for k in ("reaudit_rounds", "gap_cap_rounds")} for p in policies
    ]
    assert windows == [
        {"reaudit_rounds": 200, "gap_cap_rounds": 305},
        {"reaudit_rounds": 199, "gap_cap_rounds": 304},
    ]
    assert policies[0] == policies[1]
    manifests = [read(out, "out/manifest.json") for out in (warmup, real)]
    assert manifests[0]["training"] == manifests[1]["training"]  # image, model and init state
    networks = [
        {k: v for k, v in m["network"].items() if k != "audit_policy_hash"} for m in manifests
    ]
    assert networks[0] == networks[1]
    assert (
        manifests[0]["network"]["audit_policy_hash"] != manifests[1]["network"]["audit_policy_hash"]
    )
    reports = [json.loads(r.stdout[r.stdout.rindex("\n{\n") + 1 :]) for r in (first, second)]
    assert reports[1]["warmup_run_id"] == reports[0]["run_id"] != reports[1]["run_id"]
    assert reports[1]["checks"]["warmup_same_training"]
    assert reports[1]["checks"]["warmup_differs_only_by_the_audit_policy"]


def test_the_50m_warm_up_guard_lets_298_s_through_and_refuses_the_warm_up_window(host, tmp_path):
    warmup, real, first, second = two_builds(host, tmp_path / "a", "298", "dense-0.05b")
    assert second.returncode == 0, second.stderr
    reports = [json.loads(r.stdout[r.stdout.rindex("\n{\n") + 1 :]) for r in (first, second)]
    assert reports[1]["warmup_run_id"] == reports[0]["run_id"] != reports[1]["run_id"]
    assert reports[1]["checks"]["warmup_differs_only_by_the_audit_policy"]

    _, real, _, refused = two_builds(host, tmp_path / "b", "299", "dense-0.05b")

    assert refused.returncode == 2 and "the manifest is the warm-up build's" in refused.stderr
    assert not any((real / d).exists() for d in ("inputs", "out", "state"))


@pytest.mark.parametrize("replay", ["299", "300"])
def test_a_real_build_that_would_be_the_warm_up_is_refused(host, tmp_path, replay):
    # 299 and 300 s ask the warm-up's own window: the real manifest would be the warm-up's, and so
    # would its run id. The build stops (exit 2) and removes what it made; the staged seeds stay.
    _, real, _, second = two_builds(host, tmp_path, replay)

    assert second.returncode == 2
    assert "the manifest is the warm-up build's" in second.stderr
    assert not any((real / d).exists() for d in ("inputs", "out", "state"))
    assert len(seed_files(real / "secrets")) == 7


def test_every_replay_up_to_298_s_asks_another_window_than_the_warm_up():
    # The audit policy is the only input the replay moves, through reaudit_rounds_for: a window
    # other than the warm-up's is another policy digest, and so another run id.
    warmup = reaudit_rounds_for(300, 100)
    assert all(reaudit_rounds_for(r, 100) < warmup for r in range(1, 299))
    assert reaudit_rounds_for(299, 100) == warmup


def test_a_replay_above_the_300_s_ceiling_is_refused_by_the_manifest_stage(host, tmp_path):
    # 301 s asks a re-audit window of 201 rounds, past two leases: validate_training refuses it in
    # the manifest stage and the build removes what it made (the runbook's STOP).
    out = tmp_path / "out"

    run = build(
        host,
        out,
        driver="595.91.07",
        sm_count="148",
        env={"HT_D20_GEOMETRY": "dense-0.1b", "HT_D20_REPLAY_SECONDS": "301"},
    )

    assert run.returncode != 0 and uv_calls(host) == 2
    assert "re-audit window must be between one and two leases" in run.stderr
    assert not out.exists()


DENSE_144 = {"HT_D20_GEOMETRY": "dense-0.1b", "HT_D20_REPLAY_SECONDS": "144"}


@pytest.mark.parametrize(
    ("env", "named"),
    [
        ({"HT_D20_GEOMETRY": "dense-0.1b"}, "HT_D20_REPLAY_SECONDS: dense-0.1b has no default"),
        ({"HT_D20_GEOMETRY": "dense-0.05b"}, "HT_D20_REPLAY_SECONDS: dense-0.05b has no default"),
        ({"HT_D20_GEOMETRY": "dense-0.03b"}, "HT_D20_REPLAY_SECONDS: dense-0.03b has no default"),
        (
            {"HT_D20_GEOMETRY": "dense-0.1b", "HT_D20_REPLAY_SECONDS": "0"},
            "HT_D20_REPLAY_SECONDS: dense-0.1b has no default",
        ),
        (
            {"HT_D20_GEOMETRY": "dense-0.1b", "HT_D20_REPLAY_SECONDS": "144.5"},
            "HT_D20_REPLAY_SECONDS: dense-0.1b has no default",
        ),
        (
            {"HT_D20_GEOMETRY": "dense-1b", "HT_D20_REPLAY_SECONDS": "144"},
            "HT_D20_REPLAY_SECONDS is read by dense-0.1b, dense-0.05b and dense-0.03b only",
        ),
        (
            {"HT_D20_GEOMETRY": "dense-1b", "HT_D20_WARMUP_OUT": "/nonexistent"},
            "HT_D20_WARMUP_OUT is read by dense-0.1b, dense-0.05b and dense-0.03b only",
        ),
        (
            {**DENSE_144, "HT_D20_WARMUP_OUT": "warmup"},
            "HT_D20_WARMUP_OUT must be an absolute path",
        ),
        (
            {**DENSE_144, "HT_D20_WARMUP_OUT": "/nonexistent/warmup"},
            "HT_D20_WARMUP_OUT: no out/manifest.json under /nonexistent/warmup",
        ),
        (
            {
                "HT_D20_GEOMETRY": "dense-0.05b",
                "HT_D20_REPLAY_SECONDS": "144",
                "HT_D20_WARMUP_OUT": "/nonexistent/warmup",
            },
            "HT_D20_WARMUP_OUT: no out/manifest.json under /nonexistent/warmup",
        ),
    ],
    ids=[
        "missing",
        "missing_for_dense_0_05b",
        "missing_for_dense_0_03b",
        "zero",
        "not_whole_seconds",
        "another_geometry",
        "warmup_for_another_geometry",
        "warmup_relative",
        "warmup_without_a_manifest",
        "dense_0_05b_warmup_without_a_manifest",
    ],
)
def test_a_dense_0_1b_build_without_a_measured_replay_is_refused_before_anything_runs(
    host, tmp_path, env, named
):
    # The replay enters the run id through reaudit_rounds: no default is ever signed in its place.
    out = tmp_path / "out"

    run = build(host, out, driver="595.91.07", sm_count="148", env=env)

    assert run.returncode == 2 and named in run.stderr
    assert not out.exists() and uv_calls(host) == 0


def row_with_count(count: int) -> dict[str, object]:
    """A one-layer decoder row, two wide, whose shape-only parameter count is exactly `count`.

    It has 4 * vocab + 22 + 6 * d_ff parameters, so d_ff 1..4 settles the residue of the vocab."""
    for d_ff in range(1, 5):
        rest = count - 22 - 6 * d_ff
        if rest % 4 == 0:
            row: dict[str, object] = {
                "n_layers": 1,
                "d_model": 2,
                "n_heads": 1,
                "n_kv_heads": 1,
                "d_ff": d_ff,
                "n_experts": 1,
                "top_k_experts": 1,
                "seq_len": 2,
                "vocab": rest // 4,
                "compute_dtype": "bf16",
            }
            shapes = param_shapes(ModelConfig(**row))  # type: ignore[arg-type]
            assert sum(math.prod(shape) for shape in shapes.values()) == count
            return row
    raise AssertionError(f"no row with {count} parameters")


@pytest.mark.parametrize(
    ("count", "accepted"),
    [
        (99_999_998, False),
        (100_000_000, True),
        (300_000_000, True),
        (300_000_002, False),
    ],
    ids=["just_below", "lower_bound", "upper_bound", "just_above"],
)
def test_the_dense_0_1b_row_must_stay_inside_the_inclusive_100_to_300_million_band(
    host, tmp_path, count, accepted
):
    # The row is read from the checkout the builder sits in, so the builder runs from a copy
    # whose scripts/mfu_profiles.json holds the row under test. Call 2 of uv (the manifest build,
    # which would initialise the model) is made to fail: reaching it means the band let the row in.
    root = tmp_path / "checkout"
    builder = root / "scripts/b300/d20/build_test_manifest.sh"
    builder.parent.mkdir(parents=True)
    shutil.copy(BUILDER, builder)
    row = row_with_count(count)
    (root / "scripts/mfu_profiles.json").write_text(
        json.dumps({"profiles": {"mfu-dense-0.1b": {"model": row}}})
    )
    out = tmp_path / "out"

    env = {"HT_D20_GEOMETRY": "dense-0.1b", "HT_D20_REPLAY_SECONDS": "144", "FAKE_UV_FAIL_ON": "2"}
    run = build(host, out, env=env, builder=builder)

    if accepted:
        assert run.returncode == 9 and uv_calls(host) == 2, run.stderr
    else:
        assert run.returncode not in (0, 9) and uv_calls(host) == 1
        assert f"has {count} parameters, outside the band 100000000..300000000" in run.stderr
        assert not out.exists()


@pytest.mark.parametrize(
    ("geometry", "low", "high"),
    [("dense-0.05b", 40_000_000, 70_000_000), ("dense-0.03b", 25_000_000, 40_000_000)],
    ids=["50m", "run_profile"],
)
@pytest.mark.parametrize("edge", ["just_below", "lower_bound", "upper_bound", "just_above"])
def test_a_d94_row_must_stay_inside_its_own_inclusive_band(
    host, tmp_path, geometry, low, high, edge
):
    # As for dense-0.1b: the builder runs from a copy whose profiles hold the row under test, and
    # reaching the manifest stage (call 2 of uv, made to fail) means the band let the row in.
    count, accepted = {
        "just_below": (low - 2, False),
        "lower_bound": (low, True),
        "upper_bound": (high, True),
        "just_above": (high + 2, False),
    }[edge]
    root = tmp_path / "checkout"
    builder = root / "scripts/b300/d20/build_test_manifest.sh"
    builder.parent.mkdir(parents=True)
    shutil.copy(BUILDER, builder)
    (root / "scripts/mfu_profiles.json").write_text(
        json.dumps({"profiles": {f"mfu-{geometry}": {"model": row_with_count(count)}}})
    )
    out = tmp_path / "out"

    env = {"HT_D20_GEOMETRY": geometry, "HT_D20_REPLAY_SECONDS": "144", "FAKE_UV_FAIL_ON": "2"}
    run = build(host, out, env=env, builder=builder)

    if accepted:
        assert run.returncode == 9 and uv_calls(host) == 2, run.stderr
    else:
        assert run.returncode not in (0, 9) and uv_calls(host) == 1
        assert f"mfu-{geometry} has {count} parameters, outside the band {low}..{high}" in (
            run.stderr
        )
        assert not out.exists()


@pytest.mark.parametrize("rounds", [None, "200"], ids=["default_100", "opt_in_200"])
def test_the_trial_window_is_100_rounds_unless_the_operator_asks_for_another(
    host, tmp_path, rounds
):
    # ADR 0016: HT_D20_CHALLENGE_ROUNDS opts in to a wider admission trial window; without it the
    # policy, and so the run id, is the one every build so far signed.
    out = tmp_path / "out"
    env = {} if rounds is None else {"HT_D20_CHALLENGE_ROUNDS": rounds}

    run = build(host, out, env=env)

    assert run.returncode == 0, run.stderr
    written = json.loads((out / "inputs/policies.json").read_bytes())["admission"]
    expected = 100 if rounds is None else int(rounds)
    assert written["challenge_rounds"] == expected
    manifest, _ = accepted_manifest(out)
    admission = AdmissionPolicyV2.model_validate_json(
        LocalFSStore(out / "state/objects").get(manifest.network.admission_policy_hash)
    )
    assert admission.challenge_rounds == expected
    report = json.loads(run.stdout[run.stdout.rindex("\n{\n") + 1 :])
    assert report["challenge_rounds"] == expected and report["checks"]["challenge_rounds"]


@pytest.mark.parametrize("rounds", ["99", "401", "0", "2e2", "200 ", "+200", "0200"])
def test_a_trial_window_outside_100_to_400_rounds_is_refused_before_anything_runs(
    host, tmp_path, rounds
):
    out = tmp_path / "out"

    run = build(host, out, env={"HT_D20_CHALLENGE_ROUNDS": rounds})

    assert run.returncode == 2 and "HT_D20_CHALLENGE_ROUNDS" in run.stderr
    assert not out.exists() and uv_calls(host) == 0


def test_an_unknown_geometry_is_refused_and_the_known_ones_are_named(host, tmp_path):
    out = tmp_path / "out"

    run = build(host, out, env={"HT_D20_GEOMETRY": "dense-0.2b"})

    assert run.returncode == 2
    assert "dense-1b, dense-0.1b, dense-0.05b, dense-0.03b or tiny-cpu" in run.stderr
    assert not out.exists() and uv_calls(host) == 0


def fake_output(out: Path, layout: str = "full") -> list[str]:
    """What a build leaves under out: all four directories, or only secrets, or all but secrets.

    Returns the directory names created, in the order --clean reports them.
    """
    made = []
    if layout in ("full", "secrets_only"):
        (out / "secrets").mkdir(parents=True, mode=0o700)
        (out / "secrets/owner.seed").write_text("ab" * 32)
        made.append("secrets")
    if layout in ("full", "inputs_only"):
        (out / "inputs").mkdir(parents=True)
        (out / "inputs/training.json").write_text("{}")
        (out / "inputs/public-keys.json").write_text("{}")
        (out / "out").mkdir()
        (out / "out/manifest-envelope.json").write_text("{}")
        (out / "state/objects").mkdir(parents=True)
        (out / "state/objects/x").write_text("x")
        made += ["inputs", "out", "state"]
    return made


@pytest.mark.parametrize("layout", ["full", "secrets_only", "inputs_only"])
@pytest.mark.parametrize("extra", [False, True], ids=["empty_after", "operator_file_stays"])
def test_clean_removes_the_build_directories_and_the_out_dir_only_when_it_is_empty(
    host, tmp_path, layout, extra
):
    out = tmp_path / "out"
    made = fake_output(out, layout)
    if extra:
        (out / "notes.txt").write_text("operator notes")

    run = clean(host, str(out))

    assert run.returncode == 0, run.stderr
    assert f"removed under {out}: {' '.join(made)}" in run.stdout
    assert "ab" * 32 not in run.stdout + run.stderr  # names only, never seed content
    if extra:
        assert tree(out) == ["notes.txt"] and f"kept {out}" in run.stdout
    else:
        assert not out.exists()


def test_clean_refuses_a_relative_path_and_removes_nothing(host, tmp_path):
    fake_output(tmp_path / "rel")
    before = tree(tmp_path)

    run = clean(host, "rel", cwd=tmp_path)

    assert run.returncode == 2 and "absolute" in run.stderr
    assert tree(tmp_path) == before


@pytest.mark.parametrize("link", ["out_dir", "out_dir_slash", "secrets", "inputs", "out", "state"])
def test_clean_never_follows_a_symlink_and_removes_nothing(host, tmp_path, link):
    real = tmp_path / "real"
    fake_output(real)
    (real / "canary").write_text("keep")
    if link.startswith("out_dir"):
        target = tmp_path / "link"
        target.symlink_to(real)
    else:
        target = real / link
        moved = tmp_path / "elsewhere"
        target.rename(moved)
        target.symlink_to(moved)
    before = tree(tmp_path)

    # A trailing slash resolves the link, so `[ -L ]` alone would let it through.
    given = {"out_dir": f"{tmp_path / 'link'}", "out_dir_slash": f"{tmp_path / 'link'}/"}
    run = clean(host, given.get(link, str(real)))

    assert run.returncode == 2 and "symlink" in run.stderr
    assert tree(tmp_path) == before


def test_clean_refuses_a_directory_that_is_not_a_builder_output(host, tmp_path):
    other = tmp_path / "project"
    (other / "out").mkdir(parents=True)
    (other / "out/report.txt").write_text("not ours")
    (other / "state").mkdir()
    before = tree(tmp_path)

    run = clean(host, str(other))

    assert run.returncode == 2 and "not a builder output" in run.stderr
    assert tree(tmp_path) == before


def test_clean_refuses_the_root_even_with_slashes(host, tmp_path):
    run = clean(host, "//")
    assert run.returncode == 2 and "must not be /" in run.stderr


NOT_OURS = {
    # the TEST stack's own root: its secrets/ and state/ are not a builder's
    "test_stack_root": [
        "secrets/coord.key",
        "state/challenge.db",
        "config/metagraph.json",
    ],
    "a_key_among_the_seeds": [
        "inputs/training.json",
        "inputs/public-keys.json",
        "secrets/coord.key",
    ],
    "a_database_among_the_objects": [
        "inputs/training.json",
        "inputs/public-keys.json",
        "state/challenge.db",
    ],
    "training_json_alone": ["inputs/training.json", "out/report.txt"],
}


@pytest.mark.parametrize("files", NOT_OURS.values(), ids=NOT_OURS)
def test_clean_refuses_a_directory_it_cannot_prove_is_a_builder_output(host, tmp_path, files):
    other = tmp_path / "stack"
    for name in files:
        (other / name).parent.mkdir(parents=True, exist_ok=True)
        (other / name).write_text("keep")
    before = tree(tmp_path)

    run = clean(host, str(other))

    assert run.returncode == 2 and "not a builder output" in run.stderr
    assert tree(tmp_path) == before


def test_clean_quotes_an_out_dir_with_a_space_and_a_glob_character(host, tmp_path):
    # Unquoted, `rm -rf $OUT/inputs` would split at the space and remove tmp_path/my.
    (tmp_path / "my").mkdir()
    (tmp_path / "my/canary").write_text("keep")
    out = tmp_path / "my out*" / "o"
    fake_output(out)

    run = clean(host, str(out))

    assert run.returncode == 0, run.stderr
    assert (tmp_path / "my/canary").read_text() == "keep"
    assert tree(tmp_path / "my out*") == []


def test_a_failed_build_cleans_up_in_an_out_dir_with_a_space_and_a_glob_character(host, tmp_path):
    (tmp_path / "my").mkdir()
    (tmp_path / "my/canary").write_text("keep")
    out = tmp_path / "my out*" / "o"

    run = build(host, out, env={"FAKE_UV_FAIL_ON": "2"})

    assert run.returncode == 9 and uv_calls(host) == 2
    assert (tmp_path / "my/canary").read_text() == "keep"
    assert not (tmp_path / "my out*").exists()


def test_a_failure_leaves_an_empty_parent_directory_that_was_already_there(host, tmp_path):
    campaign = tmp_path / "campaign"
    campaign.mkdir()

    run = build(host, campaign / "o", env={"FAKE_UV_FAIL_ON": "2"})

    assert run.returncode == 9 and campaign.is_dir() and tree(campaign) == []


@pytest.mark.parametrize("link", ["out_dir", "out_dir_slash", "secrets"])
def test_a_build_into_a_symlink_is_refused_and_touches_nothing(host, tmp_path, link):
    real = tmp_path / "real"
    (real / "secrets").mkdir(parents=True)
    (real / "secrets/owner.seed").write_text("ab" * 32)
    if link == "secrets":
        out = tmp_path / "out"
        out.mkdir()
        (out / "secrets").symlink_to(real / "secrets")
        given = str(out)
    else:
        (tmp_path / "link").symlink_to(real)
        given = str(tmp_path / "link") + ("/" if link == "out_dir_slash" else "")
    before = tree(tmp_path)

    run = build(host, given)

    assert run.returncode == 2 and "symlink" in run.stderr and uv_calls(host) == 0
    assert tree(tmp_path) == before
