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

    def test_n15_measured_walls_drop_without_timeout(self) -> None:
        """Retained n15 x0017 dirs drop even when task.toml has no timeout."""
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "quick", 600)
            for name in (
                "biped",
                "biped-contact-dynamics",
                "formal-crypto",
                "cad",
                "cad-model",
                "data-anon",
                "data-anonymization",
            ):
                _task(tasks, name, None)
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
            self.assertGreaterEqual(summary["n_dropped"], 7)
            for name in (
                "biped",
                "biped-contact-dynamics",
                "formal-crypto",
                "cad",
                "cad-model",
                "data-anon",
                "data-anonymization",
            ):
                self.assertFalse((dest / name).exists(), name)

    def test_hint_beats_short_declared_timeout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "quick", 600)
            _task(tasks, "biped-contact-dynamics", 120)
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
            self.assertFalse((dest / "biped-contact-dynamics").exists())

    def test_pack_deny_alias_matches_harbor_id(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "quick", 100)
            _task(tasks, "biped-contact-dynamics", 100)
            (pack / "filter.json").write_text(
                json.dumps({"deny": ["biped"]}),
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
            self.assertEqual(summary["dropped"][0]["reason"], "deny-list")

    def test_unknown_unhinted_task_kept(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "mystery", None)
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
            self.assertEqual(summary["kept"][0]["name"], "mystery")

    def test_expert_time_estimate_hours_drops(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "quick", 600)
            d = tasks / "harbor-long"
            d.mkdir()
            (d / "task.toml").write_text(
                "[metadata]\nexpert_time_estimate_hours = 5.0\n",
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
            self.assertFalse((dest / "harbor-long").exists())

    def test_x0017_canonical_exclude_matches_dev_list(self) -> None:
        walls, exclude = filter_tasks.load_adaptor_spec()
        self.assertEqual(tuple(sorted(exclude)), tuple(sorted(filter_tasks.X0017_EXCLUDE)))
        self.assertEqual(
            set(filter_tasks.X0017_EXCLUDE),
            {
                "biped-contact-dynamics",
                "formal-crypto",
                "cad-model",
                "data-anonymization",
            },
        )
        for name in filter_tasks.X0017_EXCLUDE:
            self.assertGreaterEqual(walls.get(name, 0), 3600, name)

    def test_x0017_exclude_is_deny_even_without_timeout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "quick", 600)
            for name in filter_tasks.X0017_EXCLUDE:
                _task(tasks, name, None)
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
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            for name in filter_tasks.X0017_EXCLUDE:
                self.assertEqual(dropped[name], "deny-list", name)
                self.assertFalse((dest / name).exists(), name)

    def test_pack_allow_cannot_reinclude_x0017(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "quick", 100)
            _task(tasks, "cad-model", 100)
            (pack / "filter.json").write_text(
                json.dumps({"allow": ["quick", "cad-model"]}),
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
            self.assertFalse((dest / "cad-model").exists())
            self.assertEqual(summary["dropped"][0]["reason"], "deny-list")

    def test_alias_match_does_not_eat_unrelated_prefix(self) -> None:
        self.assertFalse(filter_tasks.alias_match("cadillac", "cad"))
        self.assertTrue(filter_tasks.alias_match("cad-model", "cad"))
        self.assertTrue(filter_tasks.alias_match("biped-contact-dynamics", "biped"))
        self.assertTrue(filter_tasks.alias_match("biped", "biped-contact-dynamics"))


if __name__ == "__main__":
    unittest.main()
