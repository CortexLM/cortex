#!/usr/bin/env python3
"""Turn Harbor job directories into the adaptor report.

usage: summarize.py <jobs_dir> <report.json> [flops_per_trial]

Harbor writes one ``result.json`` per trial under ``<jobs_dir>/<job>/<trial>/``
with ``verifier_result.rewards.reward`` (and ``exception_info`` when the trial
errored). ``primary_value`` is the mean reward over every trial; an errored
or reward-less trial counts 0. ``flops_used`` is emitted only when the topic
supplies a per-trial accounting figure — never invented.
"""
import json
import pathlib
import sys


def trial_reward(doc):
    if doc.get("exception_info") is not None:
        return 0.0, "exception"
    rewards = (doc.get("verifier_result") or {}).get("rewards") or {}
    if not isinstance(rewards, dict) or not rewards:
        return 0.0, "no reward"
    value = rewards.get("reward", next(iter(rewards.values())))
    try:
        return float(value), None
    except (TypeError, ValueError):
        return 0.0, "non-numeric reward"


def main(argv):
    if len(argv) < 3:
        sys.exit(__doc__)
    jobs, out = pathlib.Path(argv[1]), pathlib.Path(argv[2])
    fpt = argv[3] if len(argv) > 3 else ""
    trials = []
    for res in sorted(jobs.glob("*/*/result.json")):
        try:
            doc = json.loads(res.read_text())
        except (OSError, ValueError) as e:
            trials.append({"trial": res.parent.name, "reward": 0.0, "error": f"unreadable result.json: {e}"})
            continue
        reward, err = trial_reward(doc)
        trials.append({"trial": res.parent.name, "task": doc.get("task_name"), "reward": reward, "error": err})
    if not trials:
        sys.exit("no trial result.json under the job dirs; nothing to report")
    mean = sum(t["reward"] for t in trials) / len(trials)
    report = {"primary_value": mean, "claim_holds": False, "evidence": {"trials": trials, "n_trials": len(trials)}}
    if fpt.isdigit():
        report["flops_used"] = int(fpt) * len(trials)
    out.write_text(json.dumps(report, indent=2))
    print(f"trials={len(trials)} mean_reward={mean:.4f}", file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv)
