#!/usr/bin/env python3
"""inspect_scan.py: how a rule is ticked is topic data (signed params).

Marker rules fail on cheat markers the topic names; attested rules pass with
evidence; a rule the topic names in neither list fails closed; rule ids are
never markers.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import inspect_scan  # noqa: E402

RULES = [
    {"id": "no_short_circuit", "text": "the evaluator is untouched"},
    {"id": "no_answer_table", "text": "no hardcoded answers"},
    {"id": "miner_pays_provider", "text": "miner pays"},
]
MARKERS = "no_short_circuit:skip_eval|skip_verifier|always_pass_eval;no_answer_table:answer_key|hardcoded_answers"
ATTESTED = "miner_pays_provider, same_seed"


def _run_scan(
    art: Path,
    rules_body: object,
    markers: str | None = MARKERS,
    attested: str | None = ATTESTED,
) -> dict[str, dict]:
    root = art.parent
    rules = root / "rules.json"
    rules.write_text(json.dumps(rules_body), encoding="utf-8")
    out = root / "out"
    out.mkdir()
    env = {
        "PROOF_RULES_FILE": str(rules),
        "PROOF_OUTPUT_DIR": str(out),
        "PROOF_ARTIFACT_DIR": str(art),
    }
    if markers is not None:
        env[inspect_scan.PARAM_MARKER_RULES] = markers
    if attested is not None:
        env[inspect_scan.PARAM_ATTESTED_RULES] = attested
    saved = {k: os.environ.get(k) for k in env}
    os.environ.update(env)
    try:
        rc = inspect_scan.main([])
    finally:
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
    if rc != 0:
        raise AssertionError(f"inspect_scan.main returned {rc}")
    return {i["id"]: i for i in json.loads((out / "checklist.json").read_text())}


class InspectScanTests(unittest.TestCase):
    def test_signed_markers_fail_their_rules(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "cheat.py").write_text(
                "# skip_eval\n# skip_verifier\n# always_pass_eval\n# answer_key\n",
                encoding="utf-8",
            )
            items = _run_scan(
                art, {"topic_id": "t", "version": 1, "source": "topic_document", "rules": RULES}
            )
            self.assertFalse(items["no_short_circuit"]["pass"])
            self.assertFalse(items["no_answer_table"]["pass"])
            self.assertTrue(items["miner_pays_provider"]["pass"])
            self.assertIn("skip_eval", items["no_short_circuit"]["evidence"])
            self.assertIn("answer_key", items["no_answer_table"]["evidence"])

    def test_rule_id_mentions_are_not_cheats(self) -> None:
        """Miners who name the rule in README/comments must not fail it."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "README.md").write_text(
                "This harness complies with no_short_circuit and no_answer_table.\n",
                encoding="utf-8",
            )
            (art / "no_short_circuit.py").write_text("class Agent:\n    pass\n", encoding="utf-8")
            items = _run_scan(art, RULES)
            self.assertTrue(items["no_short_circuit"]["pass"])
            self.assertTrue(items["no_answer_table"]["pass"])
            self.assertIn("signed markers", items["no_short_circuit"]["evidence"])

    def test_cheat_marker_in_filename_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "skip_eval.py").write_text("class Agent:\n    pass\n", encoding="utf-8")
            (art / "hardcoded_answers.json").write_text("{}\n", encoding="utf-8")
            items = _run_scan(art, RULES)
            self.assertFalse(items["no_short_circuit"]["pass"])
            self.assertFalse(items["no_answer_table"]["pass"])

    def test_attested_rules_pass_with_evidence_and_unknown_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact" / "recipe" / "agent"
            art.mkdir(parents=True)
            (art / "agent.py").write_text("class MinerAgent:\n    pass\n", encoding="utf-8")
            items = _run_scan(
                root / "artifact",
                RULES + [{"id": "same_seed", "text": "x"}, {"id": "unnamed_rule", "text": "x"}],
            )
            self.assertTrue(items["same_seed"]["pass"])
            self.assertIn("host/topic-enforced", items["same_seed"]["evidence"])
            self.assertFalse(items["unnamed_rule"]["pass"])
            self.assertIn("inspect_marker_rules", items["unnamed_rule"]["evidence"])
            self.assertIn("inspect_attested_rules", items["unnamed_rule"]["evidence"])

    def test_no_policy_at_all_fails_every_rule_closed(self) -> None:
        """A topic that carries neither param scores nothing: every rule red."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "agent.py").write_text("class Agent:\n    pass\n", encoding="utf-8")
            items = _run_scan(art, RULES, markers=None, attested=None)
            self.assertTrue(all(not i["pass"] for i in items.values()))

    def test_truncated_scan_fails_marker_rules(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            for i in range(inspect_scan.MAX_FILES):
                (art / f"a-{i:03d}.txt").write_text("benign\n", encoding="utf-8")
            (art / "z-forbidden.txt").write_text("hardcoded_answers\n", encoding="utf-8")
            items = _run_scan(art, RULES)
            self.assertFalse(items["no_answer_table"]["pass"])
            self.assertFalse(items["no_short_circuit"]["pass"])
            self.assertIn("incomplete", items["no_answer_table"]["evidence"])
            self.assertTrue(items["miner_pays_provider"]["pass"], "attested rules do not scan")

    def test_malformed_policy_params_fail_closed(self) -> None:
        with self.assertRaises(SystemExit):
            inspect_scan.parse_marker_rules("no-colon-here")
        with self.assertRaises(SystemExit):
            inspect_scan.parse_marker_rules("rule_a:")
        with self.assertRaises(SystemExit):
            inspect_scan.parse_marker_rules("rule_a:x;rule_a:y")
        with self.assertRaises(SystemExit):
            inspect_scan.parse_marker_rules("rule_a:rule_b;rule_b:x")
        with self.assertRaises(SystemExit):
            # a marker inside a rule id would fail compliance language
            inspect_scan.parse_marker_rules("no_answer_table:answer_table")
        with self.assertRaises(SystemExit):
            inspect_scan.parse_attested_rules("Not A Rule")
        self.assertEqual(
            inspect_scan.parse_marker_rules("r_a:X|y ; r_b:z"),
            {"r_a": ("x", "y"), "r_b": ("z",)},
        )
        self.assertEqual(inspect_scan.parse_attested_rules("a_1,b-2 c_3"), frozenset({"a_1", "b-2", "c_3"}))
        self.assertEqual(inspect_scan.parse_marker_rules(""), {})
        self.assertEqual(inspect_scan.parse_attested_rules(None), frozenset())

    def test_no_secret_in_checklist(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            art.mkdir()
            (art / "note.txt").write_text("unrelated\n", encoding="utf-8")
            _run_scan(art, RULES)
            blob = (root / "out" / "checklist.json").read_text()
            self.assertNotIn("sk-", blob)


if __name__ == "__main__":
    unittest.main()
