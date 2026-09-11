#!/usr/bin/env python3
"""filter_tasks.py: default x0017 short-task allowlist; drop ≥1h and broken."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import filter_tasks  # noqa: E402

HINTS_NONE = HERE / "fixtures" / "hints_none.json"
KEEP = "cargo-flight-dispatch"


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


def _generic_filter(tasks: Path, dest: Path, pack: Path, **kwargs):
    kwargs.setdefault("hints_path", HINTS_NONE)
    kwargs.setdefault("filter_rel", None)
    kwargs.setdefault("drop_unknown", False)
    kwargs.setdefault("max_s", 3600)
    kwargs.setdefault("mode", filter_tasks.MODE_SHORTPACK)
    return filter_tasks.filter_tasks(tasks, dest, pack_dir=pack, **kwargs)


def _shortpack(tasks: Path, dest: Path, pack: Path, **kwargs):
    kwargs.setdefault("filter_rel", None)
    kwargs.setdefault("drop_unknown", False)
    kwargs.setdefault("max_s", 3600)
    kwargs.setdefault("mode", filter_tasks.MODE_SHORTPACK)
    return filter_tasks.filter_tasks(tasks, dest, pack_dir=pack, **kwargs)


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
            summary = _generic_filter(tasks, dest, pack)
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
            summary = _generic_filter(tasks, dest, pack)
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
                _generic_filter(tasks, dest, pack)

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
            summary = _generic_filter(tasks, dest, pack)
            self.assertEqual(summary["max_duration_s"], 3600)
            self.assertEqual(summary["n_kept"], 1)

    def test_n15_measured_walls_drop_without_timeout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 600)
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
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], KEEP)
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
            _task(tasks, KEEP, 600)
            _task(tasks, "biped-contact-dynamics", 120)
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertFalse((dest / "biped-contact-dynamics").exists())
            self.assertEqual(summary["kept"][0]["name"], KEEP)

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
            summary = _generic_filter(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["dropped"][0]["reason"], "deny-list")

    def test_unknown_unhinted_task_kept_without_default_allow(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "mystery", None)
            dest = pack / "out"
            summary = _generic_filter(tasks, dest, pack)
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
            summary = _generic_filter(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertFalse((dest / "harbor-long").exists())

    def test_x0017_default_allow_and_exclude_match_dev_list(self) -> None:
        walls, exclude, allow, infra = filter_tasks.load_adaptor_spec()
        self.assertEqual(tuple(sorted(allow)), tuple(sorted(filter_tasks.X0017_ALLOW)))
        self.assertEqual(tuple(sorted(exclude)), tuple(sorted(filter_tasks.X0017_EXCLUDE)))
        self.assertEqual(tuple(sorted(infra)), tuple(sorted(filter_tasks.X0017_EXCLUDE_BROKEN)))
        self.assertEqual(len(filter_tasks.X0017_ALLOW), 6)
        for name in filter_tasks.X0017_EXCLUDE_LONG:
            self.assertGreaterEqual(walls.get(name, 0), 3600, name)
        for name in filter_tasks.X0017_EXCLUDE_BROKEN:
            self.assertIn(name, exclude)
            self.assertIn(name, infra)
        for name in filter_tasks.X0017_EXCLUDE_LONG:
            self.assertNotIn(name, infra)

    def test_default_pack_keeps_only_allowlisted_short_tasks(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            for name in filter_tasks.X0017_ALLOW:
                _task(tasks, name, 600)
            for name in filter_tasks.X0017_EXCLUDE:
                _task(tasks, name, None)
            _task(tasks, "mystery", 100)
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            kept = {row["name"] for row in summary["kept"]}
            self.assertEqual(kept, set(filter_tasks.X0017_ALLOW))
            self.assertFalse((dest / "mystery").exists())
            for name in filter_tasks.X0017_EXCLUDE:
                self.assertFalse((dest / name).exists(), name)

    def test_default_allow_drops_unknown_names(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 600)
            _task(tasks, "mystery", None)
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], KEEP)
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            self.assertEqual(dropped["mystery"], "not on allow-list")

    def test_broken_until_fixed_are_denied(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 600)
            for name in filter_tasks.X0017_EXCLUDE_BROKEN:
                _task(tasks, name, 120)
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            for name in filter_tasks.X0017_EXCLUDE_BROKEN:
                self.assertEqual(dropped[name], "deny-list", name)

    def test_pack_allow_cannot_expand_default_allowlist(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 100)
            _task(tasks, "mystery", 100)
            (pack / "filter.json").write_text(
                json.dumps({"allow": [KEEP, "mystery"]}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertFalse((dest / "mystery").exists())

    def test_pack_allow_cannot_reinclude_x0017_exclude(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 100)
            _task(tasks, "cad-model", 100)
            (pack / "filter.json").write_text(
                json.dumps({"allow": [KEEP, "cad-model"]}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertFalse((dest / "cad-model").exists())
            self.assertEqual(summary["dropped"][0]["reason"], "deny-list")

    def test_pack_allow_can_further_restrict(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 100)
            _task(tasks, "bun-sourcemap-leak", 100)
            (pack / "filter.json").write_text(
                json.dumps({"allow": [KEEP]}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], KEEP)
            self.assertFalse((dest / "bun-sourcemap-leak").exists())

    def test_x0017_exclude_is_deny_even_without_timeout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 600)
            for name in filter_tasks.X0017_EXCLUDE_LONG:
                _task(tasks, name, None)
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            for name in filter_tasks.X0017_EXCLUDE_LONG:
                self.assertEqual(dropped[name], "deny-list", name)
                self.assertFalse((dest / name).exists(), name)

    def test_allowlisted_task_survives_pack_agent_timeout_false_floor(self) -> None:
        # n15 attempt1 (pin 4a04eeb1): every allowlisted task declares
        # agent_timeout = 28800, which emptied the pack at the 3600 default.
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            for name in filter_tasks.X0017_ALLOW:
                _task(tasks, name, 28_800)
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            kept = {row["name"] for row in summary["kept"]}
            self.assertEqual(kept, set(filter_tasks.X0017_ALLOW))
            for row in summary["kept"]:
                self.assertEqual(row["reason"], "allow-list")

    def test_deny_list_still_drops_long_tasks_at_a_raised_max(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 28_800)
            for name in filter_tasks.X0017_EXCLUDE:
                _task(tasks, name, 600)
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack, max_s=30_000)
            self.assertEqual(summary["n_kept"], 1)
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            for name in filter_tasks.X0017_EXCLUDE:
                self.assertEqual(dropped[name], "deny-list", name)
                self.assertFalse((dest / name).exists(), name)

    def test_allowlisted_task_dropped_by_measured_wall(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 100)
            _task(tasks, "fin-saccr-rwa", 100)
            hints = pack / "hints.json"
            hints.write_text(
                json.dumps(
                    {
                        "allow": [KEEP, "fin-saccr-rwa"],
                        "walls_sec": {"fin-saccr-rwa": 5400},
                    }
                ),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _generic_filter(tasks, dest, pack, hints_path=hints)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], KEEP)
            self.assertFalse((dest / "fin-saccr-rwa").exists())
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            self.assertEqual(dropped["fin-saccr-rwa"], "allow-list wall_s=5400 >= 3600")

    def test_allowlisted_task_kept_with_short_measured_wall(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 28_800)
            hints = pack / "hints.json"
            hints.write_text(
                json.dumps({"allow": [KEEP], "walls_sec": {KEEP: 900}}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _generic_filter(tasks, dest, pack, hints_path=hints)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["reason"], "allow-list wall_s=900")

    def test_pack_durations_do_not_drop_an_allowlisted_task(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, None)
            (pack / "task_durations.json").write_text(
                json.dumps({KEEP: 28_800}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], KEEP)

    def test_allow_list_alias_does_not_bypass_the_duration_gate(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 28_800)
            _task(tasks, f"{KEEP}-extra", 7200)
            (pack / "task_durations.json").write_text(
                json.dumps({f"{KEEP}-extra": 7200}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], KEEP)
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            self.assertEqual(dropped[f"{KEEP}-extra"], "duration_s=7200 >= 3600")

    def test_tightened_ceiling_regates_unmeasured_allowlisted_task(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 600)
            _task(tasks, "fin-saccr-rwa", 28_800)
            (pack / "filter.json").write_text(
                json.dumps({"max_duration_s": 900}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["max_duration_s"], 900)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["reason"], "duration_s=600")
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            self.assertEqual(dropped["fin-saccr-rwa"], "duration_s=28800 >= 900")

    def test_tightened_ceiling_honours_exclude_unknown_duration(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 100)
            _task(tasks, "fin-saccr-rwa", None)
            (pack / "filter.json").write_text(
                json.dumps({"max_duration_s": 300, "exclude_unknown_duration": True}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _shortpack(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], KEEP)
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            self.assertEqual(dropped["fin-saccr-rwa"], "unknown duration")

    def test_measured_wall_beats_declared_floor_under_a_tight_ceiling(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 28_800)
            hints = pack / "hints.json"
            hints.write_text(
                json.dumps({"allow": [KEEP], "walls_sec": {KEEP: 400}}),
                encoding="utf-8",
            )
            dest = pack / "out"
            summary = _generic_filter(tasks, dest, pack, max_s=900, hints_path=hints)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["reason"], "allow-list wall_s=400")

    def test_non_allowlisted_task_still_drops_on_declared_timeout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, "quick", 100)
            _task(tasks, "declared-long", 28_800)
            dest = pack / "out"
            summary = _generic_filter(tasks, dest, pack)
            self.assertEqual(summary["n_kept"], 1)
            self.assertEqual(summary["kept"][0]["name"], "quick")
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            self.assertEqual(dropped["declared-long"], "duration_s=28800 >= 3600")

    def test_first15_keeps_hour_plus_and_drops_infra_only(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            for name in filter_tasks.X0017_ALLOW:
                _task(tasks, name, 28_800)
            for name in filter_tasks.X0017_EXCLUDE_LONG:
                _task(tasks, name, None)
            for name in filter_tasks.X0017_EXCLUDE_BROKEN:
                _task(tasks, name, 120)
            _task(tasks, "mystery", 100)
            dest = pack / "out"
            summary = filter_tasks.filter_tasks(
                tasks,
                dest,
                pack_dir=pack,
                max_s=3600,
                filter_rel=None,
                drop_unknown=False,
                mode=filter_tasks.MODE_FIRST15,
            )
            self.assertEqual(summary["mode"], "first15")
            kept = {row["name"] for row in summary["kept"]}
            expected = set(filter_tasks.X0017_ALLOW) | set(
                filter_tasks.X0017_EXCLUDE_LONG
            ) | {"mystery"}
            self.assertEqual(kept, expected)
            self.assertEqual(summary["n_kept"], 11)
            dropped = {row["name"]: row["reason"] for row in summary["dropped"]}
            for name in filter_tasks.X0017_EXCLUDE_BROKEN:
                self.assertEqual(dropped[name], "deny-list", name)
                self.assertFalse((dest / name).exists(), name)
            for name in filter_tasks.X0017_EXCLUDE_LONG:
                self.assertTrue((dest / name).is_dir(), name)

    def test_first15_ignores_adaptor_and_pack_allow_lists(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 28_800)
            _task(tasks, "biped-contact-dynamics", None)
            _task(tasks, "batched-eval-parity", 100)
            (pack / "filter.json").write_text(
                json.dumps({"allow": [KEEP]}),
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
                mode="first15",
            )
            kept = {row["name"] for row in summary["kept"]}
            self.assertEqual(kept, {KEEP, "biped-contact-dynamics"})
            self.assertFalse((dest / "batched-eval-parity").exists())

    def test_default_mode_is_first15_not_shortpack(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 600)
            _task(tasks, "mystery", 100)
            dest = pack / "out"
            summary = filter_tasks.filter_tasks(
                tasks,
                dest,
                pack_dir=pack,
                max_s=3600,
                filter_rel=None,
                drop_unknown=False,
            )
            self.assertEqual(summary["mode"], "first15")
            kept = {row["name"] for row in summary["kept"]}
            self.assertEqual(kept, {KEEP, "mystery"})

    def test_unknown_mode_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp)
            tasks = pack / "tasks"
            tasks.mkdir()
            _task(tasks, KEEP, 100)
            dest = pack / "out"
            with self.assertRaises(SystemExit):
                filter_tasks.filter_tasks(
                    tasks,
                    dest,
                    pack_dir=pack,
                    max_s=3600,
                    filter_rel=None,
                    drop_unknown=False,
                    mode="allow6",
                )

    def test_alias_match_does_not_eat_unrelated_prefix(self) -> None:
        self.assertFalse(filter_tasks.alias_match("cadillac", "cad"))
        self.assertTrue(filter_tasks.alias_match("cad-model", "cad"))
        self.assertTrue(filter_tasks.alias_match("biped-contact-dynamics", "biped"))
        self.assertTrue(filter_tasks.alias_match("biped", "biped-contact-dynamics"))


if __name__ == "__main__":
    unittest.main()
