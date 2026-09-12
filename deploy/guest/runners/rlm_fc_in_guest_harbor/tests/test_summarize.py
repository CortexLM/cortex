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


def write_exception_trial(
    trial_dir: Path,
    name: str,
    *,
    exc_type: str = "RuntimeError",
    message: str = "Command timed out after 120 seconds\nTraceback…",
    agent_started: bool = True,
    verifier_started: bool = False,
    reward_txt: float | None = None,
) -> None:
    """Harbor-shaped trial that died of an exception (no verifier_result)."""
    trial_dir.mkdir(parents=True, exist_ok=True)
    body: dict = {
        "trial_name": name,
        "exception_info": {
            "exception_type": exc_type,
            "exception_message": message,
            "exception_traceback": "Traceback (most recent call last): …",
            "occurred_at": "2026-09-11T00:00:00Z",
        },
        "agent_execution": (
            {"started_at": "2026-09-11T00:00:00Z", "finished_at": "2026-09-11T00:02:00Z"}
            if agent_started
            else None
        ),
        "verifier": {"started_at": "2026-09-11T00:02:01Z"} if verifier_started else None,
    }
    (trial_dir / "result.json").write_text(json.dumps(body), encoding="utf-8")
    if reward_txt is not None:
        (trial_dir / "verifier").mkdir(exist_ok=True)
        (trial_dir / "verifier" / "reward.txt").write_text(f"{reward_txt}\n", encoding="utf-8")


class AgentExceptionPolicyTests(unittest.TestCase):
    """What a task the miner's harness crashed on counts as is topic data."""

    def _pack(self, root: Path, names: list[str]) -> Path:
        allow = root / "tasks"
        for n in names:
            (allow / n).mkdir(parents=True)
        return allow

    def test_default_fail_keeps_today_behaviour(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            allow = self._pack(root, ["task-a", "task-b"])
            write_complete_trial(jobs / "job" / "task-a__1", "task-a__1", 0.0)
            write_exception_trial(jobs / "job" / "task-b__1", "task-b__1")
            out = root / "report.json"
            with self.assertRaises(SystemExit) as ctx:
                summarize.main(
                    ["--jobs-dir", str(jobs), "--output", str(out), "--allow-tasks-dir", str(allow)]
                )
            self.assertEqual(ctx.exception.code, 2)
            self.assertFalse(out.exists())
            # And without the coverage check, the crashed trial is simply not a measurement.
            trials = summarize.collect_trials(jobs)
            self.assertEqual([t["name"] for t in trials], ["task-a__1"])

    def test_zero_scores_a_harness_crash_as_zero_with_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            allow = self._pack(root, ["task-a", "task-b", "task-c"])
            write_complete_trial(jobs / "job" / "task-a__1", "task-a__1", 1.0)
            write_complete_trial(jobs / "job" / "task-b__1", "task-b__1", 0.0)
            write_exception_trial(
                jobs / "job" / "task-c__1",
                "task-c__1",
                exc_type="AgentTimeoutError",
                message="Agent execution timed out after 900 seconds",
            )
            out = root / "report.json"
            rc = summarize.main(
                [
                    "--jobs-dir",
                    str(jobs),
                    "--output",
                    str(out),
                    "--allow-tasks-dir",
                    str(allow),
                    "--agent-exception-policy",
                    "zero",
                ]
            )
            self.assertEqual(rc, 0)
            report = json.loads(out.read_text(encoding="utf-8"))
            self.assertAlmostEqual(report["primary_value"], 1.0 / 3.0)
            ev = report["evidence"]
            self.assertEqual(ev["n_scored"], 3)
            self.assertEqual(ev["n_measured"], 2)
            self.assertEqual(ev["n_agent_exceptions"], 1)
            self.assertEqual(ev["agent_exception_policy"], "zero")
            self.assertEqual(ev["agent_exception_trials"][0]["name"], "task-c__1")
            self.assertEqual(ev["agent_exception_trials"][0]["exception_type"], "AgentTimeoutError")
            self.assertIn("900 seconds", ev["agent_exception_trials"][0]["exception_message"])
            crashed = [t for t in ev["trials"] if t["name"] == "task-c__1"][0]
            self.assertEqual(crashed["outcome"], "agent_exception")
            self.assertEqual(crashed["reward"], 0.0)

    def test_zero_never_scores_infrastructure_failures(self) -> None:
        """Environment / verifier / setup failures are not the miner's harness."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            # Environment start failed before the agent ran.
            write_exception_trial(
                jobs / "job" / "env__1", "env__1", exc_type="EnvironmentStartTimeoutError", agent_started=False
            )
            # The verifier itself raised after the agent finished.
            write_exception_trial(
                jobs / "job" / "ver__1", "ver__1", exc_type="VerifierTimeoutError", verifier_started=True
            )
            # A reward.txt is on disk: not a clean harness failure.
            write_exception_trial(jobs / "job" / "txt__1", "txt__1", reward_txt=1.0)
            # No exception recorded at all: unmeasured, whatever happened.
            (jobs / "job" / "none__1").mkdir(parents=True)
            (jobs / "job" / "none__1" / "result.json").write_text(
                json.dumps({"trial_name": "none__1", "agent_execution": {"started_at": "x"}}),
                encoding="utf-8",
            )
            trials = summarize.collect_trials(jobs, None, "zero")
            self.assertEqual(trials, [], "nothing here is the miner's harness failing")
            out = root / "report.json"
            with self.assertRaises(SystemExit):
                summarize.main(
                    ["--jobs-dir", str(jobs), "--output", str(out), "--agent-exception-policy", "zero"]
                )
            self.assertFalse(out.exists())

    def test_zero_still_fails_closed_on_an_uncovered_task(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            allow = self._pack(root, ["task-a", "task-b"])
            write_exception_trial(jobs / "job" / "task-a__1", "task-a__1")
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
                        "--agent-exception-policy",
                        "zero",
                    ]
                )
            self.assertEqual(ctx.exception.code, 2)
            self.assertFalse(out.exists())

    def test_unknown_policy_word_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            write_complete_trial(jobs / "job" / "task-a__1", "task-a__1", 1.0)
            with self.assertRaises(SystemExit):
                summarize.main(
                    ["--jobs-dir", str(jobs), "--output", str(root / "r.json"), "--agent-exception-policy", "skip"]
                )
            with self.assertRaises(SystemExit):
                summarize.collect_trials(jobs, None, "zer0")

    def test_exception_message_is_redacted(self) -> None:
        import os

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            allow = self._pack(root, ["task-a"])
            write_exception_trial(
                jobs / "job" / "task-a__1",
                "task-a__1",
                message="provider refused key sk-or-secret-value-1234",
            )
            miner_dir = root / "miner"
            miner_dir.mkdir()
            (miner_dir / "OPENROUTER_API_KEY").write_text("sk-or-secret-value-1234", encoding="utf-8")
            out = root / "report.json"
            saved = dict(os.environ)
            os.environ["PROOF_MINER_ENV_DIR"] = str(miner_dir)
            os.environ["PROOF_MINER_ENV_NAMES"] = "OPENROUTER_API_KEY"
            try:
                rc = summarize.main(
                    [
                        "--jobs-dir",
                        str(jobs),
                        "--output",
                        str(out),
                        "--allow-tasks-dir",
                        str(allow),
                        "--agent-exception-policy",
                        "zero",
                    ]
                )
            finally:
                os.environ.clear()
                os.environ.update(saved)
            self.assertEqual(rc, 0)
            blob = out.read_text(encoding="utf-8")
            self.assertNotIn("sk-or-secret-value-1234", blob)
            self.assertIn("[REDACTED]", blob)


if __name__ == "__main__":
    unittest.main()
