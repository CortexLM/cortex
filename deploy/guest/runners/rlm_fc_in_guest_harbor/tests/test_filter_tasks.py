#!/usr/bin/env python3
"""filter_tasks.py: drop tasks whose duration metadata is ≥ 1h."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import filter_tasks  # noqa: E402


def _task(root: Path, name: str, timeout_sec: int | None) -> Path:
    d = root / name
    d.mkdir()
    if timeout_sec is None:
        (d / "instruction.md").write_text("# task\n", encoding="utf-8")
        return d
    (d / "task.toml").write_text(
        f"[agent]\ntimeout_sec = {timeout_sec}\n",
        encoding="utf-8",
    )
    return d


class FilterTasksTests(unittest.TestCase):
    def test_drops_hour_or_longer(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "quick", 600)
            _task(tasks, "hour", 3600)
            _task(tasks, "slow", 7200)
            dest = pack / "out"
            summary = filter_tasks.filter_tasks(
                tasks,
                dest,
                pack_dir=pack,
                max_s=3600,
                filter_rel=None,
                drop_unknown=False,
            )
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], "quick")
            self.assertTrue((dest / "quick").is_dir())
            self.assertFalse((dest / "slow").exists())
            self.assertFalse((dest / "hour").exists())

    def test_allow_list(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "keep-me", 100)
            _task(tasks, "skip-me", 100)
            (pack / "filter.json").write_text(
                json.dumps({"allow": ["keep-me"]}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = filter_tasks.filter_tasks(
                tasks,
                dest,
                pack_dir=pack,
                max_s=3600,
                filter_rel=None,
                drop_unknown=False,
            )
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], "keep-me")

    def test_empty_after_filter_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "slow", 10_000)
            dest = pack / "out"
            with self.assertRaises(SystemExit):
                filter_tasks.filter_tasks(
                    tasks,
                    dest,
                    pack_dir=pack,
                    max_s=3600,
                    filter_rel=None,
                    drop_unknown=False,
                )

    def test_pack_max_cannot_raise_the_ceiling(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "mid", 2000)
            (pack / "filter.json").write_text(
                json.dumps({"max_duration_s": 10_000}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = filter_tasks.filter_tasks(
                tasks,
                dest,
                pack_dir=pack,
                max_s=3600,
                filter_rel=None,
                drop_unknown=False,
            )
            self.assertEqual(summary["max_duration_s"], 3600)
            self.assertEqual(summary["n_kept"], 1)


if __name__ == "__main__":
    unittest.main()
