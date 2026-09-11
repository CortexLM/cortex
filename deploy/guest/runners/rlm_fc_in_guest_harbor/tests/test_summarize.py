#!/usr/bin/env python3
"""summarize.py: mean of complete Harbor trials only; never a substitute field."""

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


def write_complete_trial(trial_dir: Path, name: str, reward: float) -> None:
    """Harbor-shaped trial: matching result.json + verifier/reward.txt."""
    trial_dir.mkdir(parents=True, exist_ok=True)
    (trial_dir / "verifier").mkdir(exist_ok=True)
    (trial_dir / "result.json").write_text(
        json.dumps(
            {
                "trial_name": name,
                "verifier_result": {"rewards": {"reward": reward}},
            }
        ),
        encoding="utf-8",
    )
    (trial_dir / "verifier" / "reward.txt").write_text(f"{reward}\n", encoding="utf-8")


class SummarizeTests(unittest.TestCase):
    def test_mean_of_trial_rewards(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            t1 = root / "job" / "a__1"
            t2 = root / "job" / "b__1"
            write_complete_trial(t1, "a__1", 1.0)
            write_complete_trial(t2, "b__1", 0.5)
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
            write_complete_trial(trial, "t__1", 1.0)
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

    def test_nonzero_harbor_exit_still_scores_measured_trials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job" / "t__1"
            write_complete_trial(trial, "t__1", 0.6)
            out = root / "report.json"
            rc = summarize.main(
                [
                    "--jobs-dir",
                    str(root / "jobs"),
                    "--output",
                    str(out),
                    "--harbor-exit",
                    "23",
                ]
            )
            self.assertEqual(rc, 0)
            report = json.loads(out.read_text(encoding="utf-8"))
            self.assertAlmostEqual(report["primary_value"], 0.6)
            self.assertEqual(report["evidence"]["n_measured"], 1)
            self.assertEqual(report["evidence"]["harbor_exit"], 23)
            self.assertTrue(report["evidence"]["harbor_incomplete"])

    def test_nonzero_harbor_exit_with_zero_trials_is_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "jobs").mkdir()
            out = root / "report.json"
            with self.assertRaises(SystemExit) as ctx:
                summarize.main(
                    [
                        "--jobs-dir",
                        str(root / "jobs"),
                        "--output",
                        str(out),
                        "--harbor-exit",
                        "143",
                    ]
                )
            self.assertEqual(ctx.exception.code, 2)
            self.assertFalse(out.exists())

    def test_reward_txt_without_result_json_is_not_a_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job1" / "hello__1"
            (trial / "verifier").mkdir(parents=True)
            (trial / "verifier" / "reward.txt").write_text("1.0\n", encoding="utf-8")
            self.assertEqual(summarize.collect_trials(root / "jobs"), [])

    def test_json_without_matching_reward_txt_is_not_a_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job" / "t__1"
            trial.mkdir(parents=True)
            (trial / "result.json").write_text(
                json.dumps(
                    {
                        "trial_name": "t__1",
                        "verifier_result": {"rewards": {"reward": 0.5}},
                    }
                ),
                encoding="utf-8",
            )
            self.assertEqual(summarize.collect_trials(root / "jobs"), [])

    def test_incomplete_job_finished_at_null_is_fail_closed(self) -> None:
        """Host harvest refuses unfinished Harbor job snapshots."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            job = root / "jobs" / "job1"
            job.mkdir(parents=True)
            (job / "result.json").write_text(
                json.dumps(
                    {
                        "finished_at": None,
                        "n_running": 1,
                        "n_completed": 7,
                        "stats": {"n_running_trials": 1},
                    }
                ),
                encoding="utf-8",
            )
            rewards = [1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.5]
            for i, reward in enumerate(rewards):
                write_complete_trial(job / f"task-{i}__1", f"task-{i}__1", reward)
            # Complete trial files still parse, but the job snapshot is stale.
            trials = summarize.collect_trials(root / "jobs")
            self.assertEqual(len(trials), 7)
            out = root / "report.json"
            with self.assertRaises(SystemExit) as ctx:
                summarize.main(
                    [
                        "--jobs-dir",
                        str(root / "jobs"),
                        "--output",
                        str(out),
                        "--harbor-exit",
                        "143",
                    ]
                )
            self.assertEqual(ctx.exception.code, 2)
            self.assertFalse(out.exists())

    def test_mismatched_reward_txt_is_not_a_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job" / "t__1"
            (trial / "verifier").mkdir(parents=True)
            (trial / "result.json").write_text(
                json.dumps(
                    {
                        "trial_name": "t__1",
                        "verifier_result": {"rewards": {"reward": 0.25}},
                    }
                ),
                encoding="utf-8",
            )
            (trial / "verifier" / "reward.txt").write_text("0.99\n", encoding="utf-8")
            self.assertEqual(summarize.collect_trials(root / "jobs"), [])

    def test_matching_reward_txt_uses_json_value(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_complete_trial(root / "jobs" / "job" / "t__1", "t__1", 0.25)
            trials = summarize.collect_trials(root / "jobs")
            self.assertEqual(len(trials), 1)
            self.assertAlmostEqual(trials[0]["reward"], 0.25)

    def test_non_numeric_reward_txt_is_not_a_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job" / "t__1"
            (trial / "verifier").mkdir(parents=True)
            (trial / "result.json").write_text(
                json.dumps(
                    {
                        "trial_name": "t__1",
                        "verifier_result": {"rewards": {"reward": 1.0}},
                    }
                ),
                encoding="utf-8",
            )
            (trial / "verifier" / "reward.txt").write_text("nan\n", encoding="utf-8")
            self.assertEqual(summarize.collect_trials(root / "jobs"), [])

    def test_scores_every_measured_trial_beyond_evidence_cap(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            for i in range(260):
                reward = 100.0 if i >= 256 else 0.0
                write_complete_trial(
                    jobs / "job" / f"trial-{i:03d}", f"trial-{i:03d}", reward
                )
            trials = summarize.collect_trials(jobs)
            self.assertEqual(len(trials), 260)
            expected = (4.0 * 100.0) / 260.0
            self.assertAlmostEqual(summarize.mean_reward(trials), expected)
            out = root / "report.json"
            rc = summarize.main(
                [
                    "--jobs-dir",
                    str(jobs),
                    "--output",
                    str(out),
                    "--harbor-exit",
                    "0",
                ]
            )
            self.assertEqual(rc, 0)
            report = json.loads(out.read_text(encoding="utf-8"))
            self.assertAlmostEqual(report["primary_value"], expected)
            self.assertEqual(report["evidence"]["n_measured"], 260)
            self.assertEqual(len(report["evidence"]["trials"]), 256)
            self.assertTrue(report["evidence"]["evidence_truncated"])
            self.assertTrue(report["claim_holds"])

    def test_allow_tasks_dir_drops_excluded_trial_names(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            allow = root / "tasks"
            (allow / "cargo-flight-dispatch").mkdir(parents=True)
            write_complete_trial(
                jobs / "job" / "cargo-flight-dispatch__1",
                "cargo-flight-dispatch__1",
                0.5,
            )
            write_complete_trial(jobs / "job" / "biped__1", "biped__1", 0.99)
            trials = summarize.collect_trials(
                jobs, summarize.allowed_task_names(allow)
            )
            self.assertEqual(len(trials), 1)
            self.assertAlmostEqual(trials[0]["reward"], 0.5)
            out = root / "report.json"
            rc = summarize.main(
                [
                    "--jobs-dir",
                    str(jobs),
                    "--output",
                    str(out),
                    "--allow-tasks-dir",
                    str(allow),
                ]
            )
            self.assertEqual(rc, 0)
            report = json.loads(out.read_text(encoding="utf-8"))
            self.assertAlmostEqual(report["primary_value"], 0.5)
            self.assertEqual(report["evidence"]["n_measured"], 1)

    def test_allow_tasks_dir_only_excluded_is_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            allow = root / "tasks"
            (allow / "cargo-flight-dispatch").mkdir(parents=True)
            write_complete_trial(jobs / "job" / "biped__1", "biped__1", 0.99)
            out = root / "report.json"
            with self.assertRaises(SystemExit) as ctx:
                summarize.main(
                    [
                        "--jobs-dir",
                        str(jobs),
                        "--output",
                        str(out),
                        "--allow-tasks-dir",
                        str(allow),
                    ]
                )
            self.assertEqual(ctx.exception.code, 2)
            self.assertFalse(out.exists())

    def test_allow_tasks_dir_partial_filtered_set_is_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            allow = root / "tasks"
            (allow / "cargo-flight-dispatch").mkdir(parents=True)
            (allow / "embedding-drift-monitor").mkdir(parents=True)
            write_complete_trial(
                jobs / "job" / "cargo-flight-dispatch__1",
                "cargo-flight-dispatch__1",
                1.0,
            )
            out = root / "report.json"
            with self.assertRaises(SystemExit) as ctx:
                summarize.main(
                    [
                        "--jobs-dir",
                        str(jobs),
                        "--output",
                        str(out),
                        "--allow-tasks-dir",
                        str(allow),
                    ]
                )
            self.assertEqual(ctx.exception.code, 2)
            self.assertFalse(out.exists())

    def test_finished_job_snapshot_still_scores_complete_trials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            job = root / "jobs" / "job1"
            job.mkdir(parents=True)
            (job / "result.json").write_text(
                json.dumps(
                    {
                        "finished_at": "2026-09-11T00:00:00Z",
                        "n_running": 0,
                        "n_completed": 1,
                    }
                ),
                encoding="utf-8",
            )
            write_complete_trial(
                job / "cargo-flight-dispatch__1", "cargo-flight-dispatch__1", 0.5
            )
            out = root / "report.json"
            rc = summarize.main(
                [
                    "--jobs-dir",
                    str(root / "jobs"),
                    "--output",
                    str(out),
                    "--harbor-exit",
                    "0",
                ]
            )
            self.assertEqual(rc, 0)
            report = json.loads(out.read_text(encoding="utf-8"))
            self.assertAlmostEqual(report["primary_value"], 0.5)
            self.assertEqual(report["evidence"]["n_measured"], 1)


if __name__ == "__main__":
    unittest.main()
