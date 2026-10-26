"""complete_audit_v2 sizes dispute spans with one checkpoint load per sized leaf."""

from __future__ import annotations

import importlib.util
import sys
from collections import Counter
from pathlib import Path
from types import ModuleType
from typing import Any

from hypertrain.auditor import island_bisect
from hypertrain.challenge.disputes_v2 import Contest
from hypertrain.challenge.store import ChallengeStore
from hypertrain.protocol.messages_v2 import IslandJobV1


def load(name: str, path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


here = Path(__file__).parent
# The real tiny audit (publication, execute, tampered/faulty completes, success, replay).
scenario = load("complete_cost_scenario", here / "test_audit_deadline_v2.py")
oracle = load("complete_cost_oracle", here.parent / "auditor/test_island_bisect_memo.py")
network = scenario.network


def test_complete_audit_v2_loads_each_sized_checkpoint_once(network, monkeypatch) -> None:
    # Given: checkpoint loads by IslandParty counted per complete_audit_v2 call
    calls: list[Counter[bytes]] = []
    active: list[Counter[bytes]] = []
    unpack, complete = island_bisect.unpack_state, ChallengeStore.complete_audit_v2

    def counting(blob: bytes) -> Any:
        if active:
            active[-1][blob] += 1
        return unpack(blob)

    def counted(self: ChallengeStore, run_id: str, job_id: str, raw: bytes) -> dict[str, str]:
        active.append(Counter())
        try:
            return complete(self, run_id, job_id, raw)
        finally:
            calls.append(active.pop())

    monkeypatch.setattr(island_bisect, "unpack_state", counting)
    monkeypatch.setattr(ChallengeStore, "complete_audit_v2", counted)
    # When
    scenario.test_real_audit_publication_completes_with_exact_lease_descriptor(network, monkeypatch)
    # Then: only the first accepted complete sizes spans, loading window 1's two checkpoints once
    store, run = network.store, network.manifest.run_id()
    (row,) = store._db.execute("SELECT id FROM records_v2 WHERE kind='contest'").fetchall()
    contest = Contest.model_validate(store._record_v2(run, "contest", row[0]))
    geometry = store._record_v2(run, "trace-geometry", contest.verdict_hash)
    job, directory = IslandJobV1.model_validate(geometry["job"]), Path(geometry["directory"])
    checkpoints = directory / "rank-0/checkpoints"
    window = {
        (checkpoints / f"{t}.safetensors").read_bytes(): 1
        for t in (0, job.manifest.training.inner.J)
    }
    assert len(calls) == 7
    assert [c for c in calls if c] == [Counter(window)]
    # Stored spans equal the unmemoized sizing of the same publication.
    expected = oracle.store_sizing(oracle.BaseParty.published(contest.miner, job, directory))
    assert (contest.step_span, contest.layer_span, contest.op_span) == expected
