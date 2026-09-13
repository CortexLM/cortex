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
                exc_type="RuntimeError",
                message="Command timed out after 120 seconds",
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
            self.assertEqual(ev["agent_exception_trials"][0]["exception_type"], "RuntimeError")
            self.assertIn("120 seconds", ev["agent_exception_trials"][0]["exception_message"])
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

    def test_agent_timeout_with_a_verifier_reward_is_measured_not_zeroed(self) -> None:
        """Harbor records AgentTimeoutError and still runs the verifier: measured wins."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            t = jobs / "job" / "task-a__1"
            t.mkdir(parents=True)
            (t / "verifier").mkdir()
            (t / "result.json").write_text(
                json.dumps(
                    {
                        "trial_name": "task-a__1",
                        "exception_info": {"exception_type": "AgentTimeoutError", "exception_message": "x"},
                        "agent_execution": {"started_at": "2026-09-11T00:00:00Z"},
                        "verifier": {"started_at": "2026-09-11T00:10:00Z"},
                        "verifier_result": {"rewards": {"reward": 0.5}},
                    }
                ),
                encoding="utf-8",
            )
            (t / "verifier" / "reward.txt").write_text("0.5\n", encoding="utf-8")
            trials = summarize.collect_trials(jobs, None, "zero")
            self.assertEqual(trials, [{"name": "task-a__1", "reward": 0.5, "outcome": "measured"}])

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
            results_blob = (root / "results.json").read_text(encoding="utf-8")
            self.assertNotIn("sk-or-secret-value-1234", results_blob)
            self.assertIn("[REDACTED]", results_blob)

    def test_results_json_is_complete_harbor_contract(self) -> None:
        import os

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            for i, reward in enumerate([1.0, 0.0, 1.0]):
                write_complete_trial(jobs / "job" / f"t{i}__1", f"t{i}__1", reward)
            out = root / "report.json"
            saved = dict(os.environ)
            os.environ["PROOF_TOPIC_ID"] = "tbench-x0032"
            os.environ["PROOF_CUSTOM_ID"] = "tbench_terminal_bench"
            os.environ["PROOF_SUBMISSION_DIGEST"] = "11" * 32
            os.environ["PROOF_ARTIFACT_DIGEST"] = "22" * 32
            try:
                rc = summarize.main(
                    [
                        "--jobs-dir",
                        str(jobs),
                        "--output",
                        str(out),
                        "--agent",
                        "proof_python_agent:ProofPythonAgent",
                        "--agent-source",
                        "artifact_dir/recipe/agent",
                        "--harness-kind",
                        "python",
                    ]
                )
            finally:
                os.environ.clear()
                os.environ.update(saved)
            self.assertEqual(rc, 0)
            results = json.loads((root / "results.json").read_text(encoding="utf-8"))
            self.assertEqual(results["contract"], "tbench-harbor-v1")
            self.assertEqual(results["topic_id"], "tbench-x0032")
            self.assertEqual(results["n_scored"], 3)
            self.assertEqual(len(results["trials"]), 3)
            self.assertAlmostEqual(results["primary_value"], results["mean_reward"])
            self.assertAlmostEqual(results["primary_value"], 2.0 / 3.0)
            self.assertEqual(results["agent"], "proof_python_agent:ProofPythonAgent")
            self.assertIn("harbor_run_log", results["logs"])

    def test_results_path_pin_is_contained(self) -> None:
        import os

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            write_complete_trial(jobs / "job" / "a__1", "a__1", 1.0)
            out = root / "output" / "report.json"
            out.parent.mkdir()
            outside = root / "outside.json"
            outside.write_text("sentinel\n", encoding="utf-8")
            saved = dict(os.environ)
            os.environ["PROOF_PARAM_RESULTS_PATH"] = "../../outside.json"
            try:
                with self.assertRaises(SystemExit) as ctx:
                    summarize.main(["--jobs-dir", str(jobs), "--output", str(out)])
                self.assertEqual(ctx.exception.code, 2)
            finally:
                os.environ.clear()
                os.environ.update(saved)
            self.assertEqual(outside.read_text(encoding="utf-8"), "sentinel\n")
            self.assertFalse((root / "output" / "results.json").exists())
            self.assertFalse(out.exists(), "bad pin must not leave report.json without results")

    def test_results_file_name_matches_host_ascii_contract(self) -> None:
        self.assertEqual(summarize.results_file_name("audit.Json"), "audit.Json")
        self.assertEqual(summarize.results_file_name(" audit.json "), "audit.json")
        self.assertEqual(summarize.results_file_name(""), "results.json")
        with self.assertRaises(SystemExit) as ctx:
            summarize.results_file_name("résultats.json")
        self.assertEqual(ctx.exception.code, 2)


def assert_results_beside_report(test: unittest.TestCase, report_path: Path) -> None:
    """Harbor summarize must always leave results.json next to report.json."""
    test.assertTrue(report_path.is_file(), f"missing {report_path}")
    results_path = report_path.parent / "results.json"
    test.assertTrue(
        results_path.is_file(),
        f"Harbor summarize must write results.json beside {report_path}",
    )
    report = json.loads(report_path.read_text(encoding="utf-8"))
    results = json.loads(results_path.read_text(encoding="utf-8"))
    test.assertIn(results["contract"], ("tbench-harbor-v1", "harbor-trials-v1"))
    test.assertAlmostEqual(results["primary_value"], report["primary_value"])
    test.assertEqual(results["claim_holds"], report["claim_holds"])
    test.assertEqual(results["n_scored"], report["evidence"]["n_scored"])
    test.assertEqual(len(results["trials"]), report["evidence"]["n_scored"])


class HarborResultsEmitTests(unittest.TestCase):
    """Obligatory results.json on successful Harbor summarize (metal tbench)."""

    def test_successful_summarize_always_writes_results_beside_report(self) -> None:
        """Every successful summarize.main leaves the sibling, including mean 0.0."""
        cases = (
            ([0.0] * 10, 0, True),
            ([1.0, 0.5], 0, False),
            ([0.6], 23, False),
            ([1.0, 0.0, 1.0], 0, False),
        )
        for rewards, harbor_exit, with_allow in cases:
            with self.subTest(rewards=rewards, harbor_exit=harbor_exit):
                with tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    jobs = root / "jobs"
                    argv = [
                        "--jobs-dir",
                        str(jobs),
                        "--output",
                        str(root / "output" / "report.json"),
                        "--harbor-exit",
                        str(harbor_exit),
                    ]
                    (root / "output").mkdir()
                    names = [f"task-{i:02d}" for i in range(len(rewards))]
                    if with_allow:
                        allow = root / "tasks"
                        for name, reward in zip(names, rewards, strict=True):
                            (allow / name).mkdir(parents=True)
                            write_complete_trial(
                                jobs / "job" / f"{name}__1", f"{name}__1", reward
                            )
                        argv.extend(["--allow-tasks-dir", str(allow)])
                    else:
                        for name, reward in zip(names, rewards, strict=True):
                            write_complete_trial(
                                jobs / "job" / f"{name}__1", f"{name}__1", reward
                            )
                    rc = summarize.main(argv)
                    self.assertEqual(rc, 0)
                    assert_results_beside_report(self, root / "output" / "report.json")


    def test_ten_zero_reward_trials_emit_full_tbench_results(self) -> None:
        """Live tbench-x0039 shape: 10/10 measured, mean 0.0, full trial table."""
        import os

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            allow = root / "tasks"
            names = [f"task-{i:02d}" for i in range(10)]
            for name in names:
                (allow / name).mkdir(parents=True)
                write_complete_trial(jobs / "job" / f"{name}__1", f"{name}__1", 0.0)
            out = root / "output" / "report.json"
            out.parent.mkdir()
            saved = dict(os.environ)
            os.environ["PROOF_TOPIC_ID"] = "tbench-x0039"
            os.environ["PROOF_CUSTOM_ID"] = "tbench_terminal_bench"
            os.environ["PROOF_SUBMISSION_DIGEST"] = "aa" * 32
            os.environ["PROOF_ARTIFACT_DIGEST"] = "bb" * 32
            os.environ["PROOF_PARAM_RESULTS_CONTRACT"] = "tbench-harbor-v1"
            try:
                rc = summarize.main(
                    [
                        "--jobs-dir",
                        str(jobs),
                        "--output",
                        str(out),
                        "--allow-tasks-dir",
                        str(allow),
                        "--agent",
                        "proof_python_agent:ProofPythonAgent",
                        "--agent-source",
                        "artifact_dir/recipe/agent",
                        "--harness-kind",
                        "python",
                    ]
                )
            finally:
                os.environ.clear()
                os.environ.update(saved)
            self.assertEqual(rc, 0)
            assert_results_beside_report(self, out)
            report = json.loads(out.read_text(encoding="utf-8"))
            results = json.loads((out.parent / "results.json").read_text(encoding="utf-8"))
            self.assertAlmostEqual(report["primary_value"], 0.0)
            self.assertTrue(report["claim_holds"])
            self.assertEqual(results["contract"], "tbench-harbor-v1")
            self.assertEqual(results["schema_version"], 1)
            self.assertEqual(results["topic_id"], "tbench-x0039")
            self.assertEqual(results["n_scored"], 10)
            self.assertEqual(results["n_measured"], 10)
            self.assertEqual(results["n_agent_exceptions"], 0)
            self.assertEqual(len(results["trials"]), 10)
            self.assertAlmostEqual(results["primary_value"], 0.0)
            self.assertAlmostEqual(results["mean_reward"], 0.0)
            self.assertTrue(results["claim_holds"])
            self.assertEqual(results["agent"], "proof_python_agent:ProofPythonAgent")
            self.assertIn("harbor_run_tail", results["logs"])
            for row in results["trials"]:
                self.assertEqual(row["outcome"], "measured")
                self.assertAlmostEqual(row["reward"], 0.0)

    def test_results_contract_pin_harbor_trials_alias(self) -> None:
        import os

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            write_complete_trial(jobs / "job" / "a__1", "a__1", 1.0)
            out = root / "report.json"
            saved = dict(os.environ)
            os.environ["PROOF_PARAM_RESULTS_CONTRACT"] = "harbor-trials-v1"
            try:
                rc = summarize.main(["--jobs-dir", str(jobs), "--output", str(out), "--agent", "harbor"])
            finally:
                os.environ.clear()
                os.environ.update(saved)
            self.assertEqual(rc, 0)
            results = json.loads((root / "results.json").read_text(encoding="utf-8"))
            self.assertEqual(results["contract"], "harbor-trials-v1")

    def test_unknown_results_contract_fails_closed_without_report(self) -> None:
        import os

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            write_complete_trial(jobs / "job" / "a__1", "a__1", 1.0)
            out = root / "report.json"
            saved = dict(os.environ)
            os.environ["PROOF_PARAM_RESULTS_CONTRACT"] = "generic-custom-v1"
            try:
                with self.assertRaises(SystemExit) as ctx:
                    summarize.main(["--jobs-dir", str(jobs), "--output", str(out)])
                self.assertEqual(ctx.exception.code, 2)
            finally:
                os.environ.clear()
                os.environ.update(saved)
            self.assertFalse(out.exists())
            self.assertFalse((root / "results.json").exists())

    def test_emit_results_from_report_repairs_overlay_that_wrote_report_only(self) -> None:
        import os

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            report_path = root / "report.json"
            report_path.write_text(
                json.dumps(
                    {
                        "primary_value": 0.0,
                        "claim_holds": True,
                        "evidence": {
                            "n_scored": 2,
                            "n_measured": 2,
                            "n_agent_exceptions": 0,
                            "mean_reward": 0.0,
                            "agent": "harbor",
                            "agent_source": "artifact_dir/agent",
                            "harness_kind": "python",
                            "agent_exception_policy": "fail",
                            "harbor_exit": 0,
                            "harbor_run_tail": "done",
                            "trials": [
                                {"name": "task-a__1", "reward": 0.0, "outcome": "measured"},
                                {"name": "task-b__1", "reward": 0.0, "outcome": "measured"},
                            ],
                        },
                    }
                )
                + "\n",
                encoding="utf-8",
            )
            saved = dict(os.environ)
            os.environ["PROOF_TOPIC_ID"] = "tbench-x0039"
            os.environ["PROOF_CUSTOM_ID"] = "tbench_terminal_bench"
            os.environ["PROOF_SUBMISSION_DIGEST"] = "cc" * 32
            os.environ["PROOF_ARTIFACT_DIGEST"] = "dd" * 32
            try:
                rc = summarize.main(["--emit-results-from-report", str(report_path)])
            finally:
                os.environ.clear()
                os.environ.update(saved)
            self.assertEqual(rc, 0)
            results = json.loads((root / "results.json").read_text(encoding="utf-8"))
            self.assertEqual(results["contract"], "tbench-harbor-v1")
            self.assertEqual(results["n_scored"], 2)
            self.assertEqual(len(results["trials"]), 2)
            self.assertAlmostEqual(results["primary_value"], 0.0)

    def test_emit_results_from_truncated_report_is_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            report_path = root / "report.json"
            report_path.write_text(
                json.dumps(
                    {
                        "primary_value": 0.5,
                        "claim_holds": True,
                        "evidence": {
                            "n_scored": 3,
                            "n_measured": 3,
                            "n_agent_exceptions": 0,
                            "mean_reward": 0.5,
                            "evidence_truncated": True,
                            "trials": [{"name": "a__1", "reward": 1.0, "outcome": "measured"}],
                        },
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaises(SystemExit) as ctx:
                summarize.main(["--emit-results-from-report", str(report_path)])
            self.assertEqual(ctx.exception.code, 2)
            self.assertFalse((root / "results.json").exists())


class TrialLogHarvestTests(unittest.TestCase):
    """Per-trial agent_log / verifier_log from Harbor's native job dir."""

    FIXTURE_JOBS = HERE / "fixtures" / "harbor-trial-logs" / "jobs"

    def _by_name(self, trials: list[dict]) -> dict[str, dict]:
        return {str(t["name"]): t for t in trials}

    def test_fixture_trial_dirs_fill_log_bodies(self) -> None:
        trials = summarize.collect_trials(self.FIXTURE_JOBS)
        rows = self._by_name(trials)
        self.assertEqual(
            set(rows),
            {"hello-world__1", "traj-only__1", "pane-only__1", "no-logs__1"},
        )

        hello = rows["hello-world__1"]
        self.assertIn("hello-world agent stdout: ran ls", hello["agent_log"])
        self.assertNotIn("trajectory must not displace", hello["agent_log"])
        self.assertIn("verifier: reward=1.0", hello["verifier_log"])
        self.assertEqual(
            hello["log_sources"],
            ["trial.log", "verifier/test-stdout.txt"],
        )

        traj = rows["traj-only__1"]
        self.assertIn("traj-only agent step", traj["agent_log"])
        self.assertEqual(traj["log_sources"], ["agent/trajectory.json"])
        self.assertNotIn("verifier_log", traj)

        pane = rows["pane-only__1"]
        self.assertIn("optional third source", pane["agent_log"])
        self.assertEqual(pane["log_sources"], ["terminus_2.pane"])

        missing = rows["no-logs__1"]
        self.assertNotIn("agent_log", missing)
        self.assertNotIn("verifier_log", missing)
        self.assertNotIn("log_sources", missing)

    def test_results_json_carries_trial_logs_on_nonzero_harbor_exit(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job" / "t__1"
            write_complete_trial(trial, "t__1", 0.6)
            (trial / "trial.log").write_text("agent ran under FAIL\n", encoding="utf-8")
            (trial / "verifier" / "test-stdout.txt").write_text(
                "verifier stdout on incomplete harbor\n", encoding="utf-8"
            )
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
            results = json.loads((root / "results.json").read_text(encoding="utf-8"))
            self.assertEqual(results["harbor_exit"], 23)
            self.assertEqual(results["logs"]["harbor_run_log"], "logs/harbor.run.log")
            row = results["trials"][0]
            self.assertEqual(row["agent_log"], "agent ran under FAIL\n")
            self.assertEqual(row["verifier_log"], "verifier stdout on incomplete harbor\n")
            self.assertEqual(
                row["log_sources"],
                ["trial.log", "verifier/test-stdout.txt"],
            )

    def test_agent_exception_keeps_exception_fields_and_harvests_logs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            crashed = jobs / "job" / "task-c__1"
            write_exception_trial(crashed, "task-c__1")
            (crashed / "trial.log").write_text("harness crashed here\n", encoding="utf-8")
            trials = summarize.collect_trials(jobs, None, "zero")
            self.assertEqual(len(trials), 1)
            row = trials[0]
            self.assertEqual(row["outcome"], "agent_exception")
            self.assertEqual(row["exception_type"], "RuntimeError")
            self.assertIn("120 seconds", row["exception_message"])
            self.assertEqual(row["agent_log"], "harness crashed here\n")
            self.assertEqual(row["log_sources"], ["trial.log"])

    def test_nested_agent_pane_is_third_source(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job" / "nested__1"
            write_complete_trial(trial, "nested__1", 1.0)
            (trial / "agent").mkdir()
            (trial / "agent" / "terminus_2.pane").write_text(
                "nested pane body\n", encoding="utf-8"
            )
            trials = summarize.collect_trials(root / "jobs")
            self.assertEqual(trials[0]["agent_log"], "nested pane body\n")
            self.assertEqual(trials[0]["log_sources"], ["agent/terminus_2.pane"])

    def test_stdout_fallback_only_when_primaries_missing(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job" / "t__1"
            write_complete_trial(trial, "t__1", 1.0)
            (trial / "agent").mkdir()
            (trial / "agent" / "stdout.txt").write_text("fallback stdout\n", encoding="utf-8")
            trials = summarize.collect_trials(root / "jobs")
            self.assertEqual(trials[0]["agent_log"], "fallback stdout\n")
            self.assertEqual(trials[0]["log_sources"], ["agent/stdout.txt"])

    def test_trial_logs_are_redacted_and_capped(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            secrets = root / "secrets"
            secrets.mkdir()
            (secrets / "inference_key").write_text("sk-secret-owner\n", encoding="utf-8")
            trial = root / "jobs" / "job" / "t__1"
            write_complete_trial(trial, "t__1", 1.0)
            huge = "keep-me\n" + ("x" * (summarize.MAX_TRIAL_LOG_CHARS + 64)) + "sk-secret-owner tail\n"
            (trial / "trial.log").write_text(huge, encoding="utf-8")
            (trial / "verifier" / "test-stdout.txt").write_text(
                "verifier saw sk-secret-owner\n", encoding="utf-8"
            )
            import os

            os.environ["PROOF_SECRETS_DIR"] = str(secrets)
            os.environ["PROOF_SECRET_FILES"] = "inference_key"
            try:
                trials = summarize.collect_trials(root / "jobs")
            finally:
                os.environ.pop("PROOF_SECRETS_DIR", None)
                os.environ.pop("PROOF_SECRET_FILES", None)
            row = trials[0]
            self.assertNotIn("sk-secret-owner", row["agent_log"])
            self.assertIn("[REDACTED]", row["agent_log"])
            self.assertLessEqual(len(row["agent_log"]), summarize.MAX_TRIAL_LOG_CHARS)
            self.assertNotIn("sk-secret-owner", row["verifier_log"])
            self.assertIn("[REDACTED]", row["verifier_log"])

    def test_thirty_three_trials_omit_logs_rather_than_overflow(self) -> None:
        """33 × 8 KiB × 2 would exceed 256 KiB: omit bodies, still score (no 503)."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            body_a = "A" * summarize.MAX_TRIAL_LOG_CHARS
            body_v = "V" * summarize.MAX_TRIAL_LOG_CHARS
            for i in range(33):
                trial = jobs / "job" / f"task-{i:02d}__1"
                write_complete_trial(trial, f"task-{i:02d}__1", 0.0)
                (trial / "trial.log").write_text(body_a, encoding="utf-8")
                (trial / "verifier" / "test-stdout.txt").write_text(body_v, encoding="utf-8")
            harbor_log = root / "harbor.run.log"
            harbor_log.write_text("job-level harbor run tail stays\n", encoding="utf-8")
            out = root / "report.json"
            rc = summarize.main(
                [
                    "--jobs-dir",
                    str(jobs),
                    "--log",
                    str(harbor_log),
                    "--output",
                    str(out),
                    "--harbor-exit",
                    "23",
                ]
            )
            self.assertEqual(rc, 0)
            results_path = root / "results.json"
            size = results_path.stat().st_size
            self.assertLessEqual(
                size,
                summarize.MAX_RESULTS_BYTES,
                f"results.json {size} bytes exceeds {summarize.MAX_RESULTS_BYTES}",
            )
            results = json.loads(results_path.read_text(encoding="utf-8"))
            self.assertEqual(len(results["trials"]), 33)
            self.assertEqual(results["n_scored"], 33)
            self.assertAlmostEqual(results["primary_value"], 0.0)
            self.assertEqual(
                results["logs"]["harbor_run_tail"],
                "job-level harbor run tail stays\n",
            )
            for row in results["trials"]:
                self.assertEqual(row["outcome"], "measured")
                self.assertIn("name", row)
                self.assertNotIn("agent_log", row)
                self.assertNotIn("verifier_log", row)
                self.assertNotIn("log_sources", row)

    def test_sixteen_trials_shrink_to_four_kib(self) -> None:
        """16 × 8 KiB × 2 overflows; 4 KiB step still fits, so bodies are kept."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            body_a = "A" * summarize.MAX_TRIAL_LOG_CHARS
            body_v = "V" * summarize.MAX_TRIAL_LOG_CHARS
            for i in range(16):
                trial = jobs / "job" / f"task-{i:02d}__1"
                write_complete_trial(trial, f"task-{i:02d}__1", 0.0)
                (trial / "trial.log").write_text(body_a, encoding="utf-8")
                (trial / "verifier" / "test-stdout.txt").write_text(body_v, encoding="utf-8")
            out = root / "report.json"
            rc = summarize.main(["--jobs-dir", str(jobs), "--output", str(out)])
            self.assertEqual(rc, 0)
            results = json.loads((root / "results.json").read_text(encoding="utf-8"))
            self.assertLessEqual((root / "results.json").stat().st_size, summarize.MAX_RESULTS_BYTES)
            self.assertEqual(len(results["trials"]), 16)
            for row in results["trials"]:
                self.assertEqual(len(row["agent_log"]), summarize.MAX_TRIAL_LOG_CHARS_STEP)
                self.assertEqual(len(row["verifier_log"]), summarize.MAX_TRIAL_LOG_CHARS_STEP)

    def test_ten_trials_keep_eight_kib_log_bodies(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            jobs = root / "jobs"
            body_a = "A" * summarize.MAX_TRIAL_LOG_CHARS
            body_v = "V" * summarize.MAX_TRIAL_LOG_CHARS
            for i in range(10):
                trial = jobs / "job" / f"task-{i:02d}__1"
                write_complete_trial(trial, f"task-{i:02d}__1", 0.0)
                (trial / "trial.log").write_text(body_a, encoding="utf-8")
                (trial / "verifier" / "test-stdout.txt").write_text(body_v, encoding="utf-8")
            out = root / "report.json"
            rc = summarize.main(["--jobs-dir", str(jobs), "--output", str(out)])
            self.assertEqual(rc, 0)
            results = json.loads((root / "results.json").read_text(encoding="utf-8"))
            self.assertLessEqual((root / "results.json").stat().st_size, summarize.MAX_RESULTS_BYTES)
            self.assertEqual(len(results["trials"]), 10)
            for row in results["trials"]:
                self.assertEqual(len(row["agent_log"]), summarize.MAX_TRIAL_LOG_CHARS)
                self.assertEqual(len(row["verifier_log"]), summarize.MAX_TRIAL_LOG_CHARS)

    def test_empty_log_files_are_omitted_never_invented(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trial = root / "jobs" / "job" / "t__1"
            write_complete_trial(trial, "t__1", 1.0)
            (trial / "trial.log").write_text("", encoding="utf-8")
            (trial / "verifier" / "test-stdout.txt").write_text("", encoding="utf-8")
            trials = summarize.collect_trials(root / "jobs")
            self.assertNotIn("agent_log", trials[0])
            self.assertNotIn("verifier_log", trials[0])
            self.assertNotIn("log_sources", trials[0])


if __name__ == "__main__":
    unittest.main()
