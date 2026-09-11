#!/usr/bin/env python3
"""inspect_scan.py: tick rules; cheat markers fail; rule ids are not cheats."""

from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys_path_parent = str(HERE.parent)
import sys

sys.path.insert(0, sys_path_parent)
import inspect_scan  # noqa: E402

RULES_OFF_LIMITS = [
    {"id": "no_eval_short_circuit", "text": "no short circuit"},
    {"id": "no_tb4_hardcoding", "text": "no tb4 answers"},
    {"id": "miner_byok_openrouter", "text": "miner pays"},
]


def _run_scan(art: Path, rules_body: object) -> dict[str, dict]:
    root = art.parent
    rules = root / "rules.json"
    rules.write_text(json.dumps(rules_body), encoding="utf-8")
    out = root / "out"
    out.mkdir()
    os.environ["PROOF_RULES_FILE"] = str(rules)
    os.environ["PROOF_OUTPUT_DIR"] = str(out)
    os.environ["PROOF_ARTIFACT_DIR"] = str(art)
    try:
        rc = inspect_scan.main([])
    finally:
        os.environ.pop("PROOF_RULES_FILE", None)
        os.environ.pop("PROOF_OUTPUT_DIR", None)
        os.environ.pop("PROOF_ARTIFACT_DIR", None)
    if rc != 0:
        raise AssertionError(f"inspect_scan.main returned {rc}")
    return {i["id"]: i for i in json.loads((out / "checklist.json").read_text())}


class InspectScanTests(unittest.TestCase):
    def test_off_limits_markers_fail_named_rules(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "cheat.py").write_text(
                "# skip_eval\n# skip_verifier\n# always_pass_eval\n"
                "# short_circuit_eval\n# tb4_answers\n# hardcoded_tb4\n",
                encoding="utf-8",
            )
            items = _run_scan(art, {"topic_id": "tbench", "version": 1, "source": "topic_document", "rules": RULES_OFF_LIMITS})
            self.assertFalse(items["no_eval_short_circuit"]["pass"])
            self.assertFalse(items["no_tb4_hardcoding"]["pass"])
            self.assertTrue(items["miner_byok_openrouter"]["pass"])
            self.assertIn("skip_eval", items["no_eval_short_circuit"]["evidence"])
            self.assertIn("tb4_answers", items["no_tb4_hardcoding"]["evidence"])
            blob = (root / "out" / "checklist.json").read_text()
            self.assertNotIn("sk-", blob)

    def test_rule_id_mentions_are_not_cheats(self) -> None:
        """Miners who name the rule in README/comments must not fail it."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "README.md").write_text(
                "This harness complies with no_eval_short_circuit and "
                "no_tb4_hardcoding. We do not short-circuit eval or hardcode tb4.\n",
                encoding="utf-8",
            )
            (art / "agent.py").write_text(
                "# checklist: no_eval_short_circuit, no_tb4_hardcoding\n"
                "class Agent:\n    pass\n",
                encoding="utf-8",
            )
            items = _run_scan(art, RULES_OFF_LIMITS)
            self.assertTrue(items["no_eval_short_circuit"]["pass"])
            self.assertTrue(items["no_tb4_hardcoding"]["pass"])
            self.assertTrue(items["miner_byok_openrouter"]["pass"])
            self.assertNotIn("no_eval_short_circuit", items["no_eval_short_circuit"]["evidence"])
            self.assertNotIn("no_tb4_hardcoding", items["no_tb4_hardcoding"]["evidence"])

    def test_rule_id_filename_is_not_a_cheat(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "no_eval_short_circuit.py").write_text(
                "class Agent:\n    pass\n", encoding="utf-8"
            )
            (art / "no_tb4_hardcoding.md").write_text(
                "notes on the no_tb4_hardcoding rule\n", encoding="utf-8"
            )
            items = _run_scan(art, RULES_OFF_LIMITS)
            self.assertTrue(items["no_eval_short_circuit"]["pass"])
            self.assertTrue(items["no_tb4_hardcoding"]["pass"])

    def test_cheat_marker_in_filename_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "skip_eval.py").write_text("class Agent:\n    pass\n", encoding="utf-8")
            (art / "tb4_answers.json").write_text("{}\n", encoding="utf-8")
            items = _run_scan(art, RULES_OFF_LIMITS)
            self.assertFalse(items["no_eval_short_circuit"]["pass"])
            self.assertFalse(items["no_tb4_hardcoding"]["pass"])
            self.assertIn("skip_eval", items["no_eval_short_circuit"]["evidence"])
            self.assertIn("tb4_answers", items["no_tb4_hardcoding"]["evidence"])

    def test_clean_artefact_passes_off_limits(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact" / "recipe" / "agent"
            art.mkdir(parents=True)
            (art / "agent.py").write_text("class MinerAgent:\n    pass\n", encoding="utf-8")
            items = _run_scan(
                root / "artifact",
                [
                    {"id": "no_eval_short_circuit", "text": "x"},
                    {"id": "no_tb4_hardcoding", "text": "x"},
                    {"id": "same_seed", "text": "x"},
                ],
            )
            self.assertTrue(items["no_eval_short_circuit"]["pass"])
            self.assertTrue(items["no_tb4_hardcoding"]["pass"])
            self.assertTrue(items["same_seed"]["pass"])

    def test_truncated_scan_fails_off_limits(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            for i in range(inspect_scan.MAX_FILES):
                (art / f"a-{i:03d}.txt").write_text("benign\n", encoding="utf-8")
            (art / "z-forbidden.txt").write_text("hardcoded_tb4\n", encoding="utf-8")
            items = _run_scan(
                art,
                [
                    {"id": "no_eval_short_circuit", "text": "x"},
                    {"id": "no_tb4_hardcoding", "text": "x"},
                ],
            )
            self.assertFalse(items["no_tb4_hardcoding"]["pass"])
            self.assertFalse(items["no_eval_short_circuit"]["pass"])
            self.assertIn("incomplete", items["no_tb4_hardcoding"]["evidence"])

    def test_unknown_rule_fails_closed_with_artefact(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "note.txt").write_text("unrelated\n", encoding="utf-8")
            items = _run_scan(
                art,
                [{"id": "must_provide_reproducible_benchmark", "text": "prove it"}],
            )
            self.assertFalse(items["must_provide_reproducible_benchmark"]["pass"])
            self.assertIn("unknown", items["must_provide_reproducible_benchmark"]["evidence"])


if __name__ == "__main__":
    unittest.main()
