#!/usr/bin/env python3
"""filter_tasks.py: the scored set is a pure function of topic data.

No task name, slice, or mode is compiled in. Selection: ``tasks`` →
pack slice for ``task_slice`` → pack ``allow`` → every task; then
``task_exclude`` / pack ``deny``, an optional duration gate, ``n_tasks``.
A named task the pack does not hold fails closed; a ``task_slice`` the pack
does not define fails closed **whether or not the pack defines any slice**; an
empty set fails closed.
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import filter_tasks  # noqa: E402


def _task(root: Path, name: str, duration_s: int | None = None, timeout_sec: int | None = None) -> Path:
    d = root / name
    d.mkdir(parents=True, exist_ok=True)
    lines = ["[task]", f'name = "{name}"']
    if duration_s is not None:
        lines.append(f"estimated_duration_s = {duration_s}")
    if timeout_sec is not None:
        lines += ["", "[agent]", f"timeout_sec = {timeout_sec}"]
    (d / "task.toml").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return d


def _pack(tmp: str, names: list[str]) -> tuple[Path, Path]:
    pack = Path(tmp) / "pack"
    tasks = pack / "tasks"
    tasks.mkdir(parents=True)
    for n in names:
        _task(tasks, n)
    return pack, tasks


def _run(pack: Path, tasks_dir: Path, dest: Path, **kw) -> dict:
    args = {
        "tasks": [],
        "exclude": [],
        "n_tasks": None,
        "task_slice": None,
        "max_s": None,
        "drop_unknown": False,
        "filter_rel": None,
    }
    args.update(kw)
    return filter_tasks.filter_tasks(tasks_dir, dest, pack_dir=pack, **args)


def _kept(dest: Path) -> list[str]:
    return sorted(p.name for p in dest.iterdir() if p.is_dir())


def _fail_message(pack: Path, tasks_dir: Path, dest: Path, **kw) -> str:
    """The refusal text for a run that must fail.

    `_fail` writes to stderr and exits, so the message is not in the exception
    — it is what the operator actually reads. Asserting on it is the point:
    a fail-closed that does not say *why* is only half the fix.
    """
    captured = io.StringIO()
    with contextlib.redirect_stderr(captured):
        try:
            _run(pack, tasks_dir, dest, **kw)
        except SystemExit:
            return captured.getvalue()
    raise AssertionError("expected SystemExit, but the call returned")


class SelectionTests(unittest.TestCase):
    def test_silent_topic_scores_every_task(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-b", "task-a", "task-c"])
            dest = Path(tmp) / "dest"
            summary = _run(pack, tasks, dest)
            self.assertEqual(summary["source"], "tasks_dir")
            self.assertEqual(_kept(dest), ["task-a", "task-b", "task-c"])
            self.assertEqual(summary["n_kept"], 3)
            self.assertIsNone(summary["max_duration_s"], "no gate unless the topic sets one")
            recorded = json.loads((dest / ".proof-task-filter.json").read_text())
            self.assertEqual(recorded["source"], "tasks_dir")
            self.assertFalse(recorded["task_slice_resolved"])

    def test_explicit_tasks_win_keep_order_and_fail_on_unknown(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            dest = Path(tmp) / "dest"
            summary = _run(pack, tasks, dest, tasks=["task-c", "task-a"])
            self.assertEqual(summary["source"], "params.tasks")
            self.assertEqual([k["name"] for k in summary["kept"]], ["task-c", "task-a"])
            self.assertEqual(_kept(dest), ["task-a", "task-c"])
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest2", tasks=["task-a", "task-missing"])

    def test_single_task_smoke_is_a_topic_shape(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            one = _run(pack, tasks, Path(tmp) / "one", tasks=["task-b"])
            self.assertEqual(_kept(Path(tmp) / "one"), ["task-b"])
            self.assertEqual(one["n_kept"], 1)
            first = _run(pack, tasks, Path(tmp) / "first", n_tasks=1)
            self.assertEqual(_kept(Path(tmp) / "first"), ["task-a"], "first of the sorted set")
            self.assertEqual(first["dropped"][0]["reason"], "beyond n_tasks=1")
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "zero", n_tasks=0)

    def test_exclude_from_params_and_pack_deny(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c", "task-d"])
            (pack / "filter.json").write_text(json.dumps({"deny": ["task-d"]}), encoding="utf-8")
            dest = Path(tmp) / "dest"
            summary = _run(pack, tasks, dest, exclude=["task-b"])
            self.assertEqual(_kept(dest), ["task-a", "task-c"])
            self.assertEqual(sorted(summary["excluded"]), ["task-b", "task-d"])
            reasons = {d["name"]: d["reason"] for d in summary["dropped"]}
            self.assertEqual(reasons, {"task-b": "excluded", "task-d": "excluded"})
            # Exact names only: no alias / prefix matching.
            _task(tasks, "task-b-extra")
            summary = _run(pack, tasks, Path(tmp) / "dest2", exclude=["task-b"])
            self.assertIn("task-b-extra", _kept(Path(tmp) / "dest2"))
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest3", tasks=["task-d"])

    def test_slice_from_pack_file_or_filter_json(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            slices = pack / "slices"
            slices.mkdir()
            (slices / "quick.json").write_text(json.dumps(["task-c", "task-a"]), encoding="utf-8")
            (slices / "one.txt").write_text("# comment\ntask-b\n", encoding="utf-8")
            (pack / "filter.json").write_text(
                json.dumps({"slices": {"from-json": ["task-a"]}}), encoding="utf-8"
            )
            s = _run(pack, tasks, Path(tmp) / "d1", task_slice="quick")
            self.assertEqual(s["source"], "pack slice quick")
            self.assertTrue(s["task_slice_resolved"])
            self.assertEqual([k["name"] for k in s["kept"]], ["task-c", "task-a"])
            s = _run(pack, tasks, Path(tmp) / "d2", task_slice="one")
            self.assertEqual(_kept(Path(tmp) / "d2"), ["task-b"])
            s = _run(pack, tasks, Path(tmp) / "d3", task_slice="from-json")
            self.assertEqual(_kept(Path(tmp) / "d3"), ["task-a"])
            # The pack defines slices, so an unknown label is a typo: fail closed.
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "d4", task_slice="typo-slice")
            # Explicit tasks still win over a slice.
            s = _run(pack, tasks, Path(tmp) / "d5", task_slice="quick", tasks=["task-b"])
            self.assertEqual(_kept(Path(tmp) / "d5"), ["task-b"])
            self.assertEqual(s["source"], "params.tasks")

    def test_slice_label_always_fails_closed_when_the_pack_defines_none(self) -> None:
        """A pack with no slices is a **refusal**, not an informational label.

        This is the LIVE Gate 1 defect. `task_slice=tb4-first-5` on a pack whose
        only slice-ish file was `MANIFEST_FIRST15` (no `slices/`) used to fall
        through to "every task", so a topic that named 5 tasks was scored on all
        10 the pack held — the run overran the wall clock and measured no
        baseline. A label the topic set is an assertion, never a hint.
        """
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b"])
            dest = Path(tmp) / "dest"
            message = _fail_message(pack, tasks, dest, task_slice="some-label")
            self.assertIn("some-label", message)
            self.assertIn("defines no slices", message)
            # Nothing was materialised: a refusal is not a partial run.
            self.assertFalse(dest.exists())

    def test_slice_label_fails_closed_and_names_what_the_pack_does_define(self) -> None:
        """A typo against a pack that *has* slices names the labels it has."""
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b"])
            slices = pack / "slices"
            slices.mkdir()
            (slices / "real-slice.json").write_text(json.dumps(["task-a"]), encoding="utf-8")
            message = _fail_message(pack, tasks, Path(tmp) / "dest", task_slice="typo-slice")
            self.assertIn("typo-slice", message)
            self.assertIn("real-slice.json", message)

    def test_an_unresolved_slice_never_widens_to_the_whole_pack(self) -> None:
        """The regression, stated as the property that matters.

        Whatever the pack holds — slices, an `allow` list, nothing — a topic
        that set a slice it cannot resolve must not be scored on a set it did
        not name. Here the pack even has an `allow` list, which the old escape
        would have fallen through to.
        """
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            (pack / "filter.json").write_text(
                json.dumps({"allow": ["task-a", "task-b", "task-c"]}), encoding="utf-8"
            )
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest", task_slice="tb4-first-5")
            # And the same pack without a slice still uses its own default.
            s = _run(pack, tasks, Path(tmp) / "dest2")
            self.assertEqual(s["source"], "pack allow")
            self.assertEqual(len(_kept(Path(tmp) / "dest2")), 3)

    def test_explicit_tasks_are_the_documented_escape_from_a_missing_slice(self) -> None:
        """The supported fix: name the tasks, and `n_tasks` bounds the set."""
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            dest = Path(tmp) / "dest"
            s = _run(
                pack,
                tasks,
                dest,
                task_slice="tb4-first-5",
                tasks=["task-a", "task-b", "task-c"],
                n_tasks=2,
            )
            # `params.tasks` wins over the slice, so the unresolved label never
            # matters — and `n_tasks` still bounds the scored set.
            self.assertEqual(s["source"], "params.tasks")
            self.assertEqual(_kept(dest), ["task-a", "task-b"])

    def test_task_count_is_a_legacy_alias_for_n_tasks(self) -> None:
        """`task_count` bounds the set; it is a count, never a slice selector."""
        self.assertEqual(filter_tasks.resolve_n_tasks("3", None), (3, "n_tasks"))
        self.assertEqual(filter_tasks.resolve_n_tasks(None, "3"), (3, "task_count"))
        # `n_tasks` wins when both are set — the documented knob is not
        # overridden by the alias.
        self.assertEqual(filter_tasks.resolve_n_tasks("2", "5"), (2, "n_tasks"))
        self.assertEqual(filter_tasks.resolve_n_tasks(None, None), (None, ""))
        self.assertEqual(filter_tasks.resolve_n_tasks("", "  "), (None, ""))

    def test_task_count_never_stands_in_for_a_slice(self) -> None:
        """A count cannot name a set: an unresolved slice is still refused.

        This is the trap the LIVE run hit. `task_count=5` looks like "score 5
        tasks", but it says nothing about *which* — so a topic that also set a
        slice the pack does not define is refused, not silently scored on the
        first 5 tasks the pack happens to hold.
        """
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest", task_slice="tb4-first-5", n_tasks=5)
            # With the set named, the count bounds it as documented.
            dest = Path(tmp) / "dest2"
            s = _run(pack, tasks, dest, tasks=["task-a", "task-b", "task-c"], n_tasks=5)
            self.assertEqual(s["n_kept"], 3)
            self.assertEqual(_kept(dest), ["task-a", "task-b", "task-c"])

    def test_pack_allow_is_the_default_set(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            (pack / "filter.json").write_text(json.dumps({"allow": ["task-b", "task-c"]}), encoding="utf-8")
            dest = Path(tmp) / "dest"
            s = _run(pack, tasks, dest)
            self.assertEqual(s["source"], "pack allow")
            self.assertEqual(_kept(dest), ["task-b", "task-c"])
            (pack / "filter.json").write_text(json.dumps({"allow": ["task-nope"]}), encoding="utf-8")
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest2")

    def test_duration_gate_only_when_set_and_timeouts_are_not_durations(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp) / "pack"
            tasks = pack / "tasks"
            tasks.mkdir(parents=True)
            _task(tasks, "short", duration_s=600)
            _task(tasks, "long", duration_s=7200)
            _task(tasks, "ceiling-only", timeout_sec=28800)
            _task(tasks, "unknown")
            (pack / "task_durations.json").write_text(json.dumps({"unknown": 900}), encoding="utf-8")
            # No gate: everything kept, including the hour-plus task.
            s = _run(pack, tasks, Path(tmp) / "nogate")
            self.assertEqual(s["n_kept"], 4)
            # Gate from the topic: only known durations at/over the ceiling drop.
            s = _run(pack, tasks, Path(tmp) / "gate", max_s=3600)
            self.assertEqual(_kept(Path(tmp) / "gate"), ["ceiling-only", "short", "unknown"])
            reasons = {d["name"]: d["reason"] for d in s["dropped"]}
            self.assertEqual(reasons, {"long": "duration_s=7200 >= 3600"})
            # A declared agent timeout is a ceiling, not a duration.
            kept_names = {k["name"]: k for k in s["kept"]}
            self.assertNotIn("duration_s", kept_names["ceiling-only"])
            self.assertEqual(kept_names["unknown"]["duration_s"], 900, "pack durations map")
            # exclude_unknown_duration drops the task with no metadata at all.
            _task(tasks, "no-meta")
            s = _run(pack, tasks, Path(tmp) / "strict", max_s=3600, drop_unknown=True)
            self.assertNotIn("no-meta", _kept(Path(tmp) / "strict"))
            self.assertNotIn("ceiling-only", _kept(Path(tmp) / "strict"))
            # The pack may only lower the ceiling.
            (pack / "filter.json").write_text(json.dumps({"max_duration_s": 700}), encoding="utf-8")
            s = _run(pack, tasks, Path(tmp) / "lower", max_s=3600)
            self.assertEqual(s["max_duration_s"], 700)
            self.assertEqual(_kept(Path(tmp) / "lower"), ["ceiling-only", "no-meta", "short"])
            (pack / "filter.json").write_text(json.dumps({"max_duration_s": 99999}), encoding="utf-8")
            s = _run(pack, tasks, Path(tmp) / "higher", max_s=3600)
            self.assertEqual(s["max_duration_s"], 3600, "pack cannot raise the topic ceiling")
            # A pack ceiling alone is a gate too (pack content is topic-pinned).
            s = _run(pack, tasks, Path(tmp) / "packonly")
            self.assertEqual(s["max_duration_s"], 99999)

    def test_malformed_pack_filter_fields_fail_closed(self) -> None:
        """A malformed field is never read as absent: that would widen the
        scored set (allow), keep tasks the pack meant to drop (deny), or lift
        a gate (max_duration_s)."""
        bad_specs = [
            {"allow": []},
            {"allow": "task-a"},
            {"allow": ["task-a", 3]},
            {"allow": ["../x"]},
            {"deny": "task-b"},
            {"deny": [None]},
            {"max_duration_s": "3600"},
            {"max_duration_s": 0},
            {"max_duration_s": -5},
            {"max_duration_s": 1.5},
            {"max_duration_s": True},
            {"slices": []},
            {"slices": {}},
            {"slices": {"quick": "task-a,task-b"}, "allow": ["task-a"], "alow": ["task-b"]},
            {"slices": {"quick": []}},
            {"slices": {"bad label!": ["task-a"]}},
            {"durations": [900]},
            {"durations": {"task-a": "900"}},
            {"durations": {"task-a": 0}},
            {"exclude_unknown_duration": "true"},
            {"alow": ["task-a"]},
        ]
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            for i, spec in enumerate(bad_specs):
                (pack / "filter.json").write_text(json.dumps(spec), encoding="utf-8")
                with self.assertRaises(SystemExit, msg=f"spec {spec!r} must be refused"):
                    _run(pack, tasks, Path(tmp) / f"dest{i}")
            # A list-shaped filter file is an allow-list; an empty one is refused too.
            (pack / "filter.json").write_text("[]", encoding="utf-8")
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest-list")
            # Comments are fine; a well-formed spec with every field validates.
            (pack / "filter.json").write_text(
                json.dumps(
                    {
                        "_comment": "pack content",
                        "slices": {"quick": ["task-a"]},
                        "allow": ["task-a", "task-b"],
                        "deny": [],
                        "max_duration_s": 3600,
                        "durations": {"task-b": 100, "_note": "ignored"},
                        "exclude_unknown_duration": False,
                    }
                ),
                encoding="utf-8",
            )
            s = _run(pack, tasks, Path(tmp) / "dest-ok")
            self.assertEqual(_kept(Path(tmp) / "dest-ok"), ["task-a", "task-b"])
            self.assertEqual(s["max_duration_s"], 3600)
            # A malformed task_durations.json fails closed as well.
            (pack / "task_durations.json").write_text(json.dumps({"task-a": "fast"}), encoding="utf-8")
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest-dur")
            (pack / "task_durations.json").write_text(json.dumps(["task-a"]), encoding="utf-8")
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest-dur2")

    def test_shipped_example_filter_validates(self) -> None:
        example = json.loads(
            (HERE.parent / "harness" / "pack_filter.example.json").read_text(encoding="utf-8")
        )
        spec = filter_tasks.validate_pack_filter(example, "pack_filter.example.json")
        self.assertEqual(spec["allow"], ["task-a", "task-b", "task-c", "task-d"])
        self.assertEqual(spec["slices"]["example-smoke"], ["task-a"])
        self.assertEqual(spec["max_duration_s"], 7200)
        self.assertEqual(spec["durations"]["task-c"], 5400)

    def test_empty_result_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a"])
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest", exclude=["task-a"])
            with self.assertRaises(SystemExit):
                _run(pack, tasks, Path(tmp) / "dest2", tasks=["task-a"], exclude=["task-a"])

    def test_dest_is_a_copy_and_summary_is_written(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a"])
            dest = Path(tmp) / "dest"
            _run(pack, tasks, dest)
            self.assertFalse((dest / "task-a").is_symlink())
            self.assertTrue((dest / "task-a" / "task.toml").is_file())
            self.assertTrue((dest / ".proof-task-filter.json").is_file())
            self.assertTrue((tasks / "task-a" / "task.toml").is_file(), "pack untouched")

    def test_no_compiled_task_names_or_modes(self) -> None:
        src = (HERE.parent / "harness" / "filter_tasks.py").read_text(encoding="utf-8").lower()
        for forbidden in ("first15", "first-15", "shortpack", "x0017", "tb4", "duration_hints"):
            self.assertNotIn(forbidden, src, forbidden)
        self.assertFalse((HERE.parent / "harness" / "duration_hints.json").exists())


class CliTests(unittest.TestCase):
    def test_cli_reads_the_guest_env_contract(self) -> None:
        import os

        with tempfile.TemporaryDirectory() as tmp:
            pack, tasks = _pack(tmp, ["task-a", "task-b", "task-c"])
            dest = Path(tmp) / "dest"
            saved = dict(os.environ)
            os.environ.update(
                {
                    "PROOF_PACK_DIR": str(pack),
                    "PROOF_PARAM_TASKS": "task-c task-b",
                    "PROOF_PARAM_N_TASKS": "1",
                    "PROOF_PARAM_TASK_EXCLUDE": "",
                }
            )
            try:
                rc = filter_tasks.main(["--tasks-dir", str(tasks), "--dest-dir", str(dest)])
            finally:
                os.environ.clear()
                os.environ.update(saved)
            self.assertEqual(rc, 0)
            self.assertEqual(_kept(dest), ["task-c"], "first of the topic's order")
            with self.assertRaises(SystemExit):
                filter_tasks.main(
                    ["--tasks-dir", str(tasks), "--dest-dir", str(dest), "--pack-dir", str(pack), "--tasks", "../x"]
                )
            with self.assertRaises(SystemExit):
                filter_tasks.main(
                    ["--tasks-dir", str(tasks), "--dest-dir", str(dest), "--pack-dir", str(pack), "--n-tasks", "one"]
                )

    def test_parse_names(self) -> None:
        self.assertEqual(filter_tasks.parse_names(" a, b  c ", "x"), ["a", "b", "c"])
        self.assertEqual(filter_tasks.parse_names("", "x"), [])
        with self.assertRaises(SystemExit):
            filter_tasks.parse_names("a,a", "x")
        with self.assertRaises(SystemExit):
            filter_tasks.parse_names("a/b", "x")


if __name__ == "__main__":
    unittest.main()
