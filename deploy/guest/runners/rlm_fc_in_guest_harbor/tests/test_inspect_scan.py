#!/usr/bin/env python3
"""inspect_scan.py: tick rules; off-limits markers fail; no secrets in evidence."""

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


class InspectScanTests(unittest.TestCase):
    def test_off_limits_markers_fail_named_rules(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "cheat.py").write_text(
                "# no_eval_short_circuit\n# no_tb4_hardcoding\n",
                encoding="utf-8",
            )
            rules = root / "rules.json"
            rules.write_text(
                json.dumps(
                    {
                        "topic_id": "tbench",
                        "version": 1,
                        "source": "topic_document",
                        "rules": [
                            {"id": "no_eval_short_circuit", "text": "no short circuit"},
                            {"id": "no_tb4_hardcoding", "text": "no tb4 answers"},
                            {"id": "miner_byok_openrouter", "text": "miner pays"},
                        ],
                    }
                ),
                encoding="utf-8",
            )
            out = root / "out"
            out.mkdir()
            os.environ["PROOF_RULES_FILE"] = str(rules)
            os.environ["PROOF_OUTPUT_DIR"] = str(out)
            os.environ["PROOF_ARTIFACT_DIR"] = str(art)
            try:
                self.assertEqual(inspect_scan.main([]), 0)
            finally:
                os.environ.pop("PROOF_RULES_FILE", None)
                os.environ.pop("PROOF_OUTPUT_DIR", None)
                os.environ.pop("PROOF_ARTIFACT_DIR", None)
            items = {i["id"]: i for i in json.loads((out / "checklist.json").read_text())}
            self.assertFalse(items["no_eval_short_circuit"]["pass"])
            self.assertFalse(items["no_tb4_hardcoding"]["pass"])
            self.assertTrue(items["miner_byok_openrouter"]["pass"])
            blob = (out / "checklist.json").read_text()
            self.assertNotIn("sk-", blob)

    def test_clean_artefact_passes_off_limits(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact" / "recipe" / "agent"
            art.mkdir(parents=True)
            (art / "agent.py").write_text("class MinerAgent:\n    pass\n", encoding="utf-8")
            rules = root / "rules.json"
            rules.write_text(
                json.dumps(
                    [
                        {"id": "no_eval_short_circuit", "text": "x"},
                        {"id": "no_tb4_hardcoding", "text": "x"},
                        {"id": "same_seed", "text": "x"},
                    ]
                ),
                encoding="utf-8",
            )
            out = root / "out"
            out.mkdir()
            os.environ["PROOF_RULES_FILE"] = str(rules)
            os.environ["PROOF_OUTPUT_DIR"] = str(out)
            os.environ["PROOF_ARTIFACT_DIR"] = str(root / "artifact")
            try:
                self.assertEqual(inspect_scan.main([]), 0)
            finally:
                os.environ.pop("PROOF_RULES_FILE", None)
                os.environ.pop("PROOF_OUTPUT_DIR", None)
                os.environ.pop("PROOF_ARTIFACT_DIR", None)
            items = {i["id"]: i for i in json.loads((out / "checklist.json").read_text())}
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
            (art / "z-forbidden.txt").write_text("no_tb4_hardcoding\n", encoding="utf-8")
            rules = root / "rules.json"
            rules.write_text(
                json.dumps(
                    [
                        {"id": "no_eval_short_circuit", "text": "x"},
                        {"id": "no_tb4_hardcoding", "text": "x"},
                    ]
                ),
                encoding="utf-8",
            )
            out = root / "out"
            out.mkdir()
            os.environ["PROOF_RULES_FILE"] = str(rules)
            os.environ["PROOF_OUTPUT_DIR"] = str(out)
            os.environ["PROOF_ARTIFACT_DIR"] = str(art)
            try:
                self.assertEqual(inspect_scan.main([]), 0)
            finally:
                os.environ.pop("PROOF_RULES_FILE", None)
                os.environ.pop("PROOF_OUTPUT_DIR", None)
                os.environ.pop("PROOF_ARTIFACT_DIR", None)
            items = {i["id"]: i for i in json.loads((out / "checklist.json").read_text())}
            self.assertFalse(items["no_tb4_hardcoding"]["pass"])
            self.assertFalse(items["no_eval_short_circuit"]["pass"])
            self.assertIn("incomplete", items["no_tb4_hardcoding"]["evidence"])

    def test_unknown_rule_fails_closed_with_artefact(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "note.txt").write_text("unrelated\n", encoding="utf-8")
            rules = root / "rules.json"
            rules.write_text(
                json.dumps(
                    [{"id": "must_provide_reproducible_benchmark", "text": "prove it"}]
                ),
                encoding="utf-8",
            )
            out = root / "out"
            out.mkdir()
            os.environ["PROOF_RULES_FILE"] = str(rules)
            os.environ["PROOF_OUTPUT_DIR"] = str(out)
            os.environ["PROOF_ARTIFACT_DIR"] = str(art)
            try:
                self.assertEqual(inspect_scan.main([]), 0)
            finally:
                os.environ.pop("PROOF_RULES_FILE", None)
                os.environ.pop("PROOF_OUTPUT_DIR", None)
                os.environ.pop("PROOF_ARTIFACT_DIR", None)
            items = {i["id"]: i for i in json.loads((out / "checklist.json").read_text())}
            self.assertFalse(items["must_provide_reproducible_benchmark"]["pass"])
            self.assertIn("unknown", items["must_provide_reproducible_benchmark"]["evidence"])


if __name__ == "__main__":
    unittest.main()
