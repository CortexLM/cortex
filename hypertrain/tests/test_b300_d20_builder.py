"""scripts/b300/d20/build_test_manifest.sh: argument checks, a failed build, and --clean.

The builder runs its Python stages as `uv run --no-sync python -I ...`. A uv shim on PATH runs them
with this venv's python (no uv, no network) and can fail on the Nth call, so a failure after the
first stage is injected without touching the builder. HT_D20_GEOMETRY=tiny-cpu keeps a full build to
a few seconds; the relay key is a throwaway public address made here.
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

import numpy as np
import pytest

from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.model import param_shapes

ROOT = Path(__file__).resolve().parents[1]
BUILDER = ROOT / "scripts/b300/d20/build_test_manifest.sh"
DIGEST = "sha256:" + "cd" * 32

# The training.json model fields the dense-0.1b build takes from its registry row.
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
    admission = json.loads((out / "inputs/policies.json").read_text())["admission"]
    assert admission["work_screen_epoch_rounds"] == 6000  # only dense-0.1b widens the window
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


def test_a_dense_0_1b_build_writes_the_registry_row_and_a_wide_screen_window(host, tmp_path):
    out = tmp_path / "out"

    run = build(
        host, out, driver="595.91.07", sm_count="148", env={"HT_D20_GEOMETRY": "dense-0.1b"}
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

    run = build(
        host, out, env={"HT_D20_GEOMETRY": "dense-0.1b", "FAKE_UV_FAIL_ON": "2"}, builder=builder
    )

    if accepted:
        assert run.returncode == 9 and uv_calls(host) == 2, run.stderr
    else:
        assert run.returncode not in (0, 9) and uv_calls(host) == 1
        assert f"has {count} parameters, outside the band 100000000..300000000" in run.stderr
        assert not out.exists()


def test_an_unknown_geometry_is_refused_and_the_known_ones_are_named(host, tmp_path):
    out = tmp_path / "out"

    run = build(host, out, env={"HT_D20_GEOMETRY": "dense-0.2b"})

    assert run.returncode == 2 and "dense-1b, dense-0.1b or tiny-cpu" in run.stderr
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
