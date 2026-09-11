#!/usr/bin/env python3
"""Host RCA helper: nested orch job.out unwrap; any --jobdir (no baked path)."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ADAPTOR = HERE.parent
sys.path.insert(0, str(ADAPTOR / "host"))
import summarize_job  # noqa: E402

SCRIPT = ADAPTOR / "host" / "summarize_job.py"

NESTED_BASELINE = {
    "topic_id": "tbench-n15",
    "vm_id": "topic-x0015",
    "output": {
        "output": "baseline",
        "body": {
            "primary_value": 0.73,
            "evidence": {
                "n_measured": 6,
                "harbor_exit": 0,
                "trials": [
                    {"name": "bun", "reward": 0.0},
                    {"name": "cargo", "reward": 0.0},
                    {"name": "embedding", "reward": 0.0},
                    {"name": "fin-saccr", "reward": 0.0},
                    {"name": "foodstuff", "reward": 0.0},
                    {"name": "atrx", "reward": 0.0},
                ],
            },
        },
    },
}

NESTED_EVALUATED = {
    "topic_id": "tbench-n15",
    "vm_id": "topic-x0015",
    "output": {
        "output": "evaluated",
        "body": {
            "report": {
                "primary_value": 0.0,
                "evidence": {
                    "n_measured": 6,
                    "trials": [
                        {"name": "atrx", "reward": 0.0},
                        {"name": "bun", "reward": 0.0},
                    ],
                },
            }
        },
    },
}

DONE_WRAP = {
    "type": "done",
    "output": {
        "output": "baseline",
        "body": {
            "primary_value": 0.5,
            "evidence": {"n_measured": 2, "trials": [{"name": "a", "reward": 1.0}, {"name": "b", "reward": 0.0}]},
        },
    },
}


class SummarizeJobTests(unittest.TestCase):
    def test_source_has_no_baked_jobdir(self) -> None:
        src = SCRIPT.read_text(encoding="utf-8")
        self.assertNotIn("pathc-baseline-n15", src)
        self.assertNotIn("/var/lib/proof/pathc", src)
        self.assertNotIn("JOBDIR =", src)

    def test_nested_baseline_job_out(self) -> None:
        summary = summarize_job.summarize(NESTED_BASELINE)
        self.assertTrue(summary["ok"])
        self.assertAlmostEqual(summary["primary_value"], 0.73)
        self.assertEqual(summary["n_measured"], 6)
        self.assertAlmostEqual(summary["cv"], 0.0)

    def test_nested_evaluated_wraps_report(self) -> None:
        summary = summarize_job.summarize(NESTED_EVALUATED)
        self.assertTrue(summary["ok"])
        self.assertAlmostEqual(summary["primary_value"], 0.0)
        self.assertEqual(summary["n_measured"], 6)
        self.assertAlmostEqual(summary["cv"], 0.0)

    def test_rlm_to_host_done_unwrap(self) -> None:
        summary = summarize_job.summarize(DONE_WRAP)
        self.assertTrue(summary["ok"])
        self.assertAlmostEqual(summary["primary_value"], 0.5)
        self.assertGreater(summary["cv"], 0.0)

    def test_missing_primary_is_not_ok(self) -> None:
        summary = summarize_job.summarize({"output": {"output": "archived", "body": None}})
        self.assertFalse(summary["ok"])
        self.assertIsNone(summary["primary_value"])

    def test_jobdir_finds_nested_job_out(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            jobdir = Path(tmp) / "retained" / "any-jail"
            nested = jobdir / "orch"
            nested.mkdir(parents=True)
            (nested / "job.out").write_text(json.dumps(NESTED_BASELINE), encoding="utf-8")
            found = summarize_job.find_job_out(jobdir)
            self.assertEqual(found, nested / "job.out")
            proc = subprocess.run(
                [sys.executable, str(SCRIPT), "--jobdir", str(jobdir)],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("primary_value=0.73", proc.stdout)
            self.assertIn("n_measured=6", proc.stdout)
            self.assertIn("cv=0", proc.stdout)

    def test_positional_directory_is_a_jobdir(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            jobdir = Path(tmp) / "other-job"
            jobdir.mkdir()
            (jobdir / "job.out").write_text(json.dumps(NESTED_EVALUATED), encoding="utf-8")
            proc = subprocess.run(
                [sys.executable, str(SCRIPT), str(jobdir), "--json"],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            payload = json.loads(proc.stdout)
            self.assertAlmostEqual(payload["primary_value"], 0.0)
            self.assertEqual(payload["n_measured"], 6)


if __name__ == "__main__":
    unittest.main()
