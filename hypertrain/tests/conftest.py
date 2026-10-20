from __future__ import annotations

import json
import os
import subprocess
import uuid
from pathlib import Path

import numpy as np
import pytest

from systemd_probe import classify_systemd_probe

SEQ_LEN = 1024
SAMPLES_PER_SHARD = 128


def _shard(rng: np.random.Generator) -> bytes:
    rows = np.empty((SAMPLES_PER_SHARD, SEQ_LEN + 1), dtype=np.uint32)
    rows[:, 0] = rng.integers(0, 259, SAMPLES_PER_SHARD)
    noise = rng.random((SAMPLES_PER_SHARD, SEQ_LEN)) < 0.1
    rand = rng.integers(0, 259, (SAMPLES_PER_SHARD, SEQ_LEN))
    for t in range(SEQ_LEN):
        step = (rows[:, t] * 7 + 3) % 259
        rows[:, t + 1] = np.where(noise[:, t], rand[:, t], step)
    return rows.astype("<u4").tobytes()


def build_corpus(root: Path, n_train: int = 2, n_holdout: int = 1) -> Path:
    for name, n, seed in (("train", n_train, 1), ("holdout", n_holdout, 2)):
        d = root / name
        d.mkdir(parents=True, exist_ok=True)
        rng = np.random.default_rng(seed)
        for i in range(n):
            (d / f"shard-{i:05d}.u32").write_bytes(_shard(rng))
        (d / "manifest.json").write_text(
            json.dumps(
                {
                    "seq_len": SEQ_LEN,
                    "samples_per_shard": SAMPLES_PER_SHARD,
                    "n_shards": n,
                    "n_samples": n * SAMPLES_PER_SHARD,
                    "sample_format": "u32[seq_len+1] token ids",
                },
                sort_keys=True,
            )
        )
    return root


# `fast` marker: the CPU mining-path subset scripts/check-fast.sh runs in under 2 minutes
# (protocol, beacon, data, ledger, aggregator, trainer core, auditor, service contract/rounds,
# miner client). Chosen from --durations profiles on 2026-10-18; a file is listed, not a test,
# so new tests in these files are fast by default. FAST_SLOW_TESTS are the few multi-second
# scale/perf cases inside those files that add nothing to the mining path.
FAST_DIRS = (
    "tests/protocol",
    "tests/beacon",
    "tests/data",
    "tests/datasets",
    "tests/ledger",
    "tests/aggregator",
    "tests/ops",
)
FAST_FILES = (
    "tests/test_smoke.py",
    "tests/trainer/test_trainer_core.py",
    "tests/auditor/test_auditor.py",
    "tests/challenge/test_contract.py",
    "tests/challenge/test_rounds.py",
    "tests/miner/test_miner.py",
    "tests/miner/test_segments_e2e.py",
    "tests/miner/test_selfcheck.py",
    "tests/miner/test_server_keys.py",
    "tests/miner/test_cli_v3.py",
    "tests/miner/test_update.py",
    "tests/miner/test_genesis_cpu_binding.py",
    "tests/miner/test_hardware_class.py",
    "tests/miner/test_client_network_v2.py",
    "tests/miner/test_multigpu_cli.py",
    "tests/miner/test_coldkey_ops.py",
    "tests/challenge/test_trial_export.py",
)
FAST_SLOW_TESTS = (
    "tests/data/test_parts.py::test_oversize_input_never_publishes_index",
    "tests/aggregator/test_weighted_v2.py::"
    "test_scale_roster_when_unique_owner_caps_saturate_in_many_stages[1024]",
    "tests/ledger/test_ledger.py::test_cap_65536_largest_ties_by_hotkey_rest_carried",
    "tests/challenge/test_contract.py::test_get_weights_p99_under_2s_with_65536_hotkeys",
)


def pytest_collection_modifyitems(config: pytest.Config, items: list[pytest.Item]) -> None:
    root = Path(str(config.rootpath))
    for item in items:
        rel = Path(item.fspath).relative_to(root).as_posix()
        nodeid = f"{rel}::{item.name}"
        if (
            rel in FAST_FILES or any(rel.startswith(d + "/") for d in FAST_DIRS)
        ) and nodeid not in FAST_SLOW_TESTS:
            item.add_marker(pytest.mark.fast)


@pytest.fixture(scope="session", autouse=True)
def corpus_dir(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Hermetic corpus: tests and their subprocesses read HYPERTRAIN_DATA_DIR, never <repo>/data.

    data/ is unpublished (.publishignore), so a fresh clone has none. Set HYPERTRAIN_DATA_DIR to
    run against the real corpora locally.
    """
    preset = os.environ.get("HYPERTRAIN_DATA_DIR")
    if preset:
        return Path(preset)
    d = build_corpus(tmp_path_factory.mktemp("corpus"))
    os.environ["HYPERTRAIN_DATA_DIR"] = str(d)
    return d


@pytest.fixture(scope="session")
def systemd_manager() -> None:
    """Skip unless this user may start transient units on the system manager, as the product does.

    Non-root skips only when the manager is absent or authorization is denied; any other probe
    failure (EAGAIN, failed transient unit, timeout) is a broken host and fails. Root that cannot
    is a broken host, not an unprivileged one: fail, so the heavy `native` job (isolated root
    environment) can never turn green by skipping.
    """
    try:
        probe = subprocess.run(
            [
                "systemd-run",
                "--unit=ht-probe-" + uuid.uuid4().hex,
                "--wait",
                "--collect",
                "--quiet",
                "--no-ask-password",
                "--property=MemoryMax=64M",
                "--property=CPUQuota=50%",
                "/bin/true",
            ],
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        verdict = classify_systemd_probe(probe.returncode, probe.stderr)
        reason = f"rc={probe.returncode} {probe.stderr.strip()[-500:]}"
    except FileNotFoundError as exc:
        verdict, reason = "unavailable", repr(exc)
    except (OSError, subprocess.TimeoutExpired) as exc:
        verdict, reason = "error", repr(exc)
    if verdict == "ok":
        return
    if os.geteuid() == 0:
        pytest.fail(f"systemd-run as root failed: {reason}")
    if verdict == "error":
        pytest.fail(f"systemd-run probe failed: {reason}")
    pytest.skip(f"system manager transient units unavailable: {reason}")
