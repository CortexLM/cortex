"""Follow-ups from verify-9-r2: journal head anchor (tail truncation) and single-writer guard."""

from __future__ import annotations

from pathlib import Path

import pytest
from agg_helpers import COORD, fresh, scenario
from test_aggregator import DATASET

from hypertrain.aggregator.checkpoint import (
    finalize_checkpoint,
    read_manifest,
    verify_checkpoint,
    write_checkpoint,
)
from hypertrain.aggregator.core import JournalError, load_state


def test_truncated_final_line_detected_by_checkpoint_anchor(tmp_path: Path) -> None:
    sc = scenario(tmp_path / "s")
    agg = sc["agg"]
    agg.finalize_round(0)
    s = load_state(agg.store, sc["t1"]["body"]["out_state"])
    d = tmp_path / "ckpt"
    write_checkpoint(d, COORD, s.theta, [sc["t0"], sc["t1"]], license="Apache-2.0", dataset=DATASET)
    finalize_checkpoint(d, COORD, (), journal_anchor=agg.journal.anchor)
    assert verify_checkpoint(d) == []
    anchor = read_manifest(d)["body"]["journal_anchor"]
    assert fresh(tmp_path / "s").journal.anchor == anchor  # intact journal accepted
    type(agg)(agg.store, agg.run_id, agg.params, COORD, tmp_path / "s" / "agg-state", anchor=anchor)
    path = agg.journal.path
    lines = path.read_bytes().splitlines(keepends=True)
    path.write_bytes(b"".join(lines[:-1]))  # drop the trailing 'final' event
    fresh(tmp_path / "s")  # a bare hash chain cannot see it ...
    with pytest.raises(JournalError, match="anchored entry"):  # ... the signed anchor does
        type(agg)(
            agg.store, agg.run_id, agg.params, COORD, tmp_path / "s" / "agg-state", anchor=anchor
        )
    with pytest.raises(JournalError, match="malformed"):
        agg.journal.check_anchor({"seq": True, "hash": anchor["hash"]})


def test_second_writer_refused(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    a = sc["agg"]
    b = fresh(tmp_path)
    b.finalize_round(0)
    before = a.journal.path.read_bytes()
    with pytest.raises(JournalError, match="another aggregator"):
        a.finalize_round(1)
    assert a.journal.path.read_bytes() == before
