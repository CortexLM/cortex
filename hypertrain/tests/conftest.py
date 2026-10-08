from __future__ import annotations

import json
import os
from pathlib import Path

import numpy as np
import pytest

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
