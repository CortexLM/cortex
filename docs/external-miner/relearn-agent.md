<!-- protocol_version: 1 -->

# Relearn Agent — miners

Challenge id: **`relearn-agent`**.

Post-train the pinned checkpoint into an agent that solves tasks **by using the
tools it is given**, and beat the champion on episodes you cannot see. Long
guide, eval image, and harness:
[CortexLM/relearn](https://github.com/CortexLM/relearn).
Cortex pin: [`config/relearn-agent-pin.toml`](../../config/relearn-agent-pin.toml).

**Gateway:** `https://network.cortex.foundation`  
**CLI:** `ctx agent submit` (install: [README](./README.md#1-install-ctx))  
Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

## What you start from

| Thing | Value |
|-------|-------|
| Base model | `Qwen/Qwen3.8-27B` — Apache-2.0, public, ungated, native VLM |
| Unit of work | A **recorded tool-use trace**: goal, tool schemas, steps (arguments + observations), final answer |
| Eval image | `ghcr.io/cortexlm/relearn-agent-eval@sha256:4db52b13…` ([`CortexLM/relearn`](https://github.com/CortexLM/relearn) PR #3) |

This is the same checkpoint the [`relearn`](./relearn.md) challenge pins, and it
is **not** the same challenge. `relearn` scores answers. This one scores
*traces*: the model is dropped into an environment with tools it has to call,
and only a run that reached the answer through those tools counts. The scored
holdout is a set of recorded traces, not a list of prompts. Tool names are
whatever that episode's environment exposes — the CI catalogue uses
`inspect` / `lookup`; live catalogues are free-form.

## How you are scored

Your lattice score comes from one thing: a bootstrap paired test on **task
success over the private episode set**. Everything else on this page is a gate.
Gates can zero a run; none of them can raise one.

The gates exist because task success alone cannot tell an agent apart from a
model that memorised the answer. So the same eval run also measures:

| Gate | What it means for you |
|------|----------------------|
| **Holdout win** | You are compared on a **private** episode set you never see. Winning the published split proves nothing |
| **Trace replay** | Your emitted tool calls are re-executed against the episode's environment. A call whose arguments are not derivable from the goal or an earlier observation, or a final answer that appears before the observation supporting it, is not a grounded solve. Mean validity below 0.80 fails |
| **Tool ablation** | The same episodes are re-run with the tools stubbed out. If your success barely moves, the environment was never load-bearing and you are not an agent. The drop must be at least 0.10 |
| **Observation shuffle** | The same episodes are re-run with another episode's observation. If your success barely moves, you answered the prompt, not the task. The drop must be at least 0.10 |
| **Capability canary** | A general instruction-following slice, scored on the same run and **kept out of the number you are paid on**. Regressing more than ~2 points below the champion is a hard zero |
| **Public–holdout gap** | A published-split score far above your holdout reads as memorization and fails. An empty public split fails too |
| **No contamination** | Holdout episode ids or observation hashes in your training metadata reject the submission. **Declaring nothing fails too** — an empty manifest leaves the gate with nothing to check (`contamination_evidence_missing`) |

An arm that did not run is a failure, not a pass: an eval that returned no
ablation measurement cannot show you used your tools, and this challenge does
not crown "unproven".

The blunt version: **if your model answers the goal without using the image or
the tools, it fails.** There is no score to be had from a very good language
model here.

## Before you submit

```bash
ctx agent status
```

`can_score` is the field to read: while it is `false` every submit answers
**503**, nothing is stored, and no pod is rented. The same reply carries
`eval_backend`, `live_harvest_wired`, `champion_baseline_recorded`, and
`base_weights`, plus the base pin, the episode commitment and size, and the
three arms the eval must run. It never reveals episode ids, goals, or
observation hashes. A run scored offline is reported as `eval_backend: sim`
and is never a real verdict.

## Submit

```bash
ctx agent submit \
  --hotkey 64-hex-hotkey \
  --artifact-digest sha256-of-your-artifact \
  --train-dataset your-training-environment-id \
  --wait
```

`--train-id` declares episode ids, `--train-hash` declares observation hashes,
and `--train-dataset` declares environment ids; they land in
`manifest.train_episode_ids`, `train_observation_hashes`, and
`train_environment_ids`. Declare at least one. The contamination gate only
rejects *overlap* with the scored set, but a manifest that declares nothing
gives it nothing to check and fails as `contamination_evidence_missing`.

The same submit with `curl`:

```bash
curl -sS -X POST https://network.cortex.foundation/challenge/relearn-agent/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "64-hex hotkey",
    "artifact_digest": "sha256 of your artifact",
    "artifact_uri": "optional-url",
    "manifest": {
      "train_episode_ids": [],
      "train_observation_hashes": [],
      "train_environment_ids": ["your-training-environment-id"]
    }
  }'
```

## Read the verdict

```bash
ctx agent show ag_0123456789abcdef --wait
```

Eligible runs sit at `awaiting_admin` until an operator promotes. You do not
promote. A contaminated or empty-evidence manifest is rejected without renting
anything.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
