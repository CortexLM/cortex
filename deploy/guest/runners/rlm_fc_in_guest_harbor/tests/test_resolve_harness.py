#!/usr/bin/env python3
"""resolve_harness.py: custom Python primary, no Terminus-2 default."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import resolve_harness  # noqa: E402


class ResolveHarnessTests(unittest.TestCase):
    def test_evaluate_prefers_custom_python(self) -> None:
        art = HERE / "fixtures" / "python_agent"
        fields = resolve_harness.select("evaluate", art, "terminus-2")
        self.assertEqual(fields["kind"], "python")
        self.assertEqual(fields["import_path"], "agent.agent:Agent")
        self.assertEqual(fields["wrapper"], "1")
        self.assertNotEqual(fields["builtin"], "terminus-2")

    def test_evaluate_harbor_baseagent_still_works(self) -> None:
        art = HERE / "fixtures"
        fields = resolve_harness.select("evaluate", art, "terminus-2")
        self.assertEqual(fields["kind"], "harbor")
        self.assertEqual(fields["import_path"], "agent.agent:MinerAgent")
        self.assertEqual(fields["wrapper"], "0")

    def test_harness_json_wins(self) -> None:
        art = HERE / "fixtures" / "harness_json"
        fields = resolve_harness.select("evaluate", art, "terminus-2")
        self.assertEqual(fields["kind"], "python")
        self.assertEqual(fields["import_path"], "agent.agent:Agent")
        self.assertIn("harness.json", fields["source"])

    def test_evaluate_run_sh_is_script_not_terminus(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            art = Path(tmp)
            recipe = art / "recipe"
            recipe.mkdir()
            (recipe / "run.sh").write_text("#!/bin/sh\necho hi\n", encoding="utf-8")
            fields = resolve_harness.select("evaluate", art, "terminus-2")
            self.assertEqual(fields["kind"], "script")
            self.assertEqual(fields["entry"], "recipe/run.sh")
            self.assertEqual(fields["builtin"], "")

    def test_evaluate_empty_refuses_topic_builtin(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            art = Path(tmp)
            art.mkdir(exist_ok=True)
            with self.assertRaises(SystemExit):
                resolve_harness.select("evaluate", art, "terminus-2")

    def test_baseline_without_artefact_uses_topic_builtin(self) -> None:
        fields = resolve_harness.select("baseline", None, "terminus-2")
        self.assertEqual(fields["kind"], "builtin")
        self.assertEqual(fields["builtin"], "terminus-2")
        self.assertEqual(fields["source"], "topic")

    def test_explicit_builtin_in_harness_json(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            art = Path(tmp)
            (art / "harness.json").write_text(
                json.dumps({"kind": "builtin", "name": "oracle"}),
                encoding="utf-8",
            )
            fields = resolve_harness.select("evaluate", art, "terminus-2")
            self.assertEqual(fields["kind"], "builtin")
            self.assertEqual(fields["builtin"], "oracle")

    def test_cli_evaluate_without_artefact_fails(self) -> None:
        with self.assertRaises(SystemExit):
            resolve_harness.main(["--job", "evaluate", "--topic-agent", "terminus-2"])


if __name__ == "__main__":
    unittest.main()
