"""ADR 0011 part 3: a miner answers a dispute about a CPU publication without replaying it in its
own daemon: every manifest pins the environment, the daemon's ambient need not honor the pin, and
a replay under it would fail REPLAY_COMMITMENT_MISMATCH against an honest, pinned publication."""

from __future__ import annotations

from pathlib import Path

import pytest
from test_island_launch_v2 import staged_job

from env_pin_fixtures import CPU_AVX2, HOSTILE, ambient, child, clean_env, needs_avx2
from hypertrain.miner.island_launch import launch_island

REPLAY = """
import sys
from pathlib import Path
import hypertrain.trainer
from hypertrain.miner.dispute_follow import replay_party
from hypertrain.protocol.messages_v2 import IslandJobV1

published = Path(sys.argv[1])
job = IslandJobV1.model_validate_json((published / "job.json").read_bytes())
print(type(replay_party("miner", job, published)).__name__)
"""


@needs_avx2
def test_publication_is_answered_under_any_process_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ambient(monkeypatch)
    job = staged_job(tmp_path, pinned=CPU_AVX2)
    published = launch_island(job, tmp_path, backend="cpu", trace=True).directory
    done = child(REPLAY, clean_env() | HOSTILE, str(published))
    assert done.returncode == 0, done.stderr
    assert done.stdout.strip() == "IslandParty"
