#!/usr/bin/env python3
"""summarize.py: mean of trial rewards only; never a substitute field."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ADAPTOR = HERE.parent
import sys

sys.path.insert(0, str(ADAPTOR / "harness"))
import summarize  # noqa: E402


class SummarizeTests(unittest.TestCase):
    def test_mean_of_trial_rewards(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            t1 = root / "job" / "a__1"
            t2 = root / "job" / "b__1"
            t1.mkdir(parents=True)
            t2.mkdir(parents=True)
            (t1 / "result.json").write_text(
                json.dumps(
                    {
                        "trial_name": "a__1",
                        "verifier_result": {"rewards": {"reward": 1.0}},
                    }
                ),
                encoding="utf-8",
            )
            (t2 / "result.json").write_text(
                json.dumps(
                    {
                        "trial_name": "b__1",
                        "verifier_result": {"rewards": {"reward": 0.5}},
                    }
                ),
                encoding="utf-8",
            )
            trials = summarize.collect_trials(root)
            self.assertEqual(len(trials), 2)
            self.assertAlmostEqual(summarize.mean_reward(trials), 0.75)

    def test_missing_reward_is_not_a_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            t = root / "job" / "x__1"
            t.mkdir(parents=True)
            (t / "result.json").write_text(
                json.dumps(
                    {
                        "trial_name": "x__1",
                        "score": 1.0,
                        "accuracy": 1.0,
                        "stats": {"mean": 0.99},
                    }
                ),
                encoding="utf-8",
            )
            self.assertEqual(summarize.collect_trials(root), [])

    def test_job_level_result_is_not_a_trial(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            job = root / "job"
            job.mkdir(parents=True)
            (job / "result.json").write_text(
                json.dumps({"stats": {"evals": {"x": {"metrics": [{"mean": 0.9}]}}}}),
                encoding="utf-8",
            )
            self.assertEqual(summarize.collect_trials(root), [])

    def test_redact_secrets_in_tail_and_report(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            secrets = root / "secrets"
            secrets.mkdir()
            (secrets / "inference_key").write_text("sk-secret-owner\n", encoding="utf-8")
            jobs = root / "jobs"
            trial = jobs / "job" / "t__1"
            trial.mkdir(parents=True)
            (trial / "result.json").write_text(
                json.dumps(
                    {
                        "trial_name": "t__1",
                        "verifier_result": {"rewards": {"reward": 1.0}},
                    }
                ),
                encoding="utf-8",
            )
            log = root / "harbor.log"
            log.write_text("called with sk-secret-owner\n", encoding="utf-8")
            out = root / "report.json"
            import os

            os.environ["PROOF_SECRETS_DIR"] = str(secrets)
            os.environ["PROOF_SECRET_FILES"] = "inference_key"
            try:
                rc = summarize.main(
                    [
                        "--jobs-dir",
                        str(jobs),
                        "--log",
                        str(log),
                        "--output",
                        str(out),
                        "--harbor-exit",
                        "0",
                        "--agent",
                        "agent.agent:MinerAgent",
                        "--agent-source",
                        "artifact_dir/agent",
                    ]
                )
            finally:
                os.environ.pop("PROOF_SECRETS_DIR", None)
                os.environ.pop("PROOF_SECRET_FILES", None)
            self.assertEqual(rc, 0)
            dumped = out.read_text(encoding="utf-8")
            self.assertNotIn("sk-secret-owner", dumped)
            self.assertIn("[REDACTED]", dumped)
            report = json.loads(dumped)
            self.assertAlmostEqual(report["primary_value"], 1.0)
            self.assertTrue(report["claim_holds"])

    def test_no_trials_is_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "jobs").mkdir()
            with self.assertRaises(SystemExit) as ctx:
                summarize.main(
                    [
                        "--jobs-dir",
                        str(root / "jobs"),
                        "--output",
                        str(root / "report.json"),
                    ]
                )
            self.assertEqual(ctx.exception.code, 2)
            self.assertFalse((root / "report.json").exists())


if __name__ == "__main__":
    unittest.main()
