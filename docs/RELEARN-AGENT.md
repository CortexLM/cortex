# Relearn Agent (live challenge)

Control-plane notes. Miners start at [`external-miner/relearn-agent.md`](./external-miner/relearn-agent.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

Eval image and harness live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
This repo pins them in `config/relearn-agent-pin.toml`.

| Field | Value |
|-------|--------|
| `challenge_id` | `relearn-agent` |
| `challenge_scoring_version` | `1` |
| Base model | `Qwen/Qwen3.8-27B` — the **same** checkpoint [`relearn`](./RELEARN.md) pins |
| Unit of work | A **recorded tool-use trace**: goal, tool schemas, steps, final answer |
| Tools | Free-form schemas on the wire; CI synthetic catalogue uses `inspect` / `lookup` |
| Port | `8099` (local host `28099`) |
| Emission | `1500` bps |

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). Operator promote is
`POST /v1/admin/promote`. Epoch emit is champion lattice; others `NoScore` (D24).

## Why this is not `relearn` with extra steps

`relearn` scores answers on a holdout of prompts. A model that memorised the
answers scores well, and the vision families are defended by a pixel-shuffle
control. Relearn Agent moves the unit of work: the model is dropped into an
environment it has to *act* in, and task success alone cannot tell an agent
apart from a very good language model.

So the eval produces three pieces of counter-evidence on the same run, and all
three are gates rather than reports:

| Arm | What it catches |
|-----|-----------------|
| **Trace replay** | The emitted tool calls are re-executed against the episode's environment. A call whose arguments are not derivable from the goal or a prior observation, or a final answer that precedes the observation supporting it, is not a grounded solve |
| **Tool ablation** | The same episodes with tools stubbed. If success barely drops, the environment was never load-bearing |
| **Observation shuffle** | The same episodes with another episode's observation. If success barely drops, the model answered the prompt, not the task |

An arm that did not run is a failure, not a pass: an eval that returned no
ablation measurement cannot show the model used its tools, and this challenge
does not crown "unproven".

## Episodes and anti-overfit gates

The episode set is **not** in git. `config/relearn-agent-pin.toml` carries only
`holdout_commitment` and `holdout_size`. Episodes come from
`RELEARN_AGENT_HOLDOUT_FILE` and are verified at boot; a missing or mismatched
file means submissions answer **503** rather than scoring a reconstructable set
or the published split.

An episode that needs no tool call is refused at load, along with one that
exposes no tools: neither can separate an agent from recall, so neither belongs
in the set.

The committed commitment is the CI / local one (a documented dev salt over a
synthetic catalogue). It is **not** the live seal — see the ceremony below.

Promotion requires every gate:

| Gate | Rule |
|------|------|
| Holdout displacement | Bootstrap paired test on private episode success (the only series that may enter the lattice) |
| Trace replay | Mean validity ≥ `MIN_TRACE_VALIDITY`; an empty arm is fail-closed |
| Tool ablation | Success must drop ≥ `MIN_ABLATION_DROP` with the tools stubbed; an empty arm is fail-closed |
| Observation shuffle | Success must drop ≥ `MIN_SHUFFLE_DROP` with the observation swapped; an empty arm is fail-closed |
| Public–holdout gap | Public far above holdout signals memorization; empty public is fail-closed |
| Capability canary | General instruction-following slice is **off** the visible score. Regression past `CANARY_EPSILON` vs the champion is a hard zero |
| Contamination | Any holdout episode id / observation hash in submitted training metadata rejects the run. An **undeclared** manifest is `contamination_evidence_missing`, not a pass |

```bash
# Rotate the episode set (episodes never enter git). Never reuse another
# challenge's salt.
cargo run -p xtask -- relearn-agent-holdout \
  --catalog ~/.base-secrets/relearn-agent-catalogue.json \
  --salt "$RELEARN_AGENT_HOLDOUT_SALT" --size 120 \
  --exclude 1 --exclude 2 … \
  --out deploy/secrets/relearn-agent/episodes.json
```

Paste the printed `holdout_commitment` into `config/relearn-agent-pin.toml`,
then re-sign the trust root ([`../config/CEREMONY.md`](../config/CEREMONY.md)).
Production must rotate the salt **and** the catalogue.

## Who is allowed to produce a score

The deterministic offline harness is not a fallback. A host scores only when
one of these holds:

| Condition | `POST /v1/submissions` |
|-----------|------------------------|
| `RELEARN_AGENT_FORCE_SIM=1` (CI / local only) | sim, reported as `eval_backend: "sim"` |
| digest-pinned `eval_image_digest` **and** a wired harvest **and** a recorded champion baseline | live replay on a digest-pinned Lium pod |
| anything less | **503** — naming the first missing piece |

`GET /v1/status` publishes `eval_backend`, `force_sim`, `can_score`,
`live_harvest_wired`, `champion_baseline_recorded`, and the three required
arms. A refusal is not a submission: nothing is persisted unless scoring
produced a verdict, so a 503 leaves no row behind.

`eval_image_digest` is pinned (`CortexLM/relearn` PR #3). A live host still
needs the harvest wired and a champion baseline recorded; without those every
submission answers 503. That is the intended fail-closed state, not an outage.

## Champion baseline (required on a live host)

Every gate compares against the champion, so a live host with none answers
**503 `no champion baseline recorded`**. The baseline must come from the same
scorer submissions face: a sim host uses the sim baseline; a live host uses
either an operator measurement (`RELEARN_AGENT_BASE_CHAMPION_FILE`, verified at
boot against the pin's `eval_image_digest` **and** `holdout_commitment`, and
refused if it is missing a series the gates read) or the wired harvest. Sim
numbers are never a candidate on a live host.

## Eval image contract

Live scoring runs the digest-pinned `eval_image` on a Lium pod. The
control-plane client is [`crates/relearn-agent-harvest`](../crates/relearn-agent-harvest);
the tool environment, the replay, and both ablation arms live in
[`CortexLM/relearn`](https://github.com/CortexLM/relearn). Nothing in this repo
can run an episode.

Per run the control plane boots `eval_image@<digest>` with the master SSH
public key on a pod the **miner** pays for, writes `request.json` into
`/tmp/relearn_agent_eval` over stdin (run inputs are never interpolated into
the remote command), runs `relearn-agent-eval score`, reads back
`RELEARN_METRICS=<document>` and `RELEARN_EVAL_OK`, scrubs the
workdir, and **requires verified termination** before accepting any score.

### Pod environment

A Lium `InstanceSpec` has no env field, so the pod inherits **nothing** from
the master. Everything the image reads from its environment has to be handed to
it, and a missing teacher URL is why a pod can boot, run, and never print
`RELEARN_EVAL_OK`:

| Variable | Source | Required |
|----------|--------|----------|
| `RELEARN_TEACHER_API_URL` | host env | **yes** — no judge, no score |
| `RELEARN_TEACHER_MODEL` | host env, else the pin's `teacher_model` | yes (defaulted) |
| `RELEARN_TEACHER_API_KEY` | host env | only if the teacher API requires auth |
| `RELEARN_BASE_MODEL` | the pin's `base_model` | yes |

Only the names are in git. Values are operator state, delivered in
`teacher.env` over stdin with `umask 077` — never on the remote command line.
A host missing the teacher URL reports `can_score: false` and refuses
**before** renting.

**`RELEARN_TEACHER_API_KEY` crosses to a miner-paid pod.** Scope and
rate-limit that credential, rotate it on suspicion, or leave it unset if the
pod can reach the teacher without auth.

A contaminated or empty-evidence `manifest` is rejected **before** the rent
as well: those submissions cannot produce a lattice score, so they must not
spend a pod or the teacher key.

The markers are the language challenge's markers. What keeps the challenges
apart is `challenge_id` in the document and the non-overlapping series keys
(`t<id>` / `s<id>` / `o<id>` here).

The request carries recorded tool-use traces (goal, tool schemas, every step's
arguments and observation, final answer) under `holdout`. A model that answers
the prompt without using the tools fails the withheld-observation and shuffle
controls. `submission_digest` and `artifact_digest` are checked against what
was asked, and `eval_image_digest` / `holdout_commitment` against the pin. Any
mismatch is a 503, not a score.

**The request carries the episodes.** The pod sees the private set for the
duration of the run — an agent cannot be scored in an environment it is not
given. The mitigations are the digest-pinned image, delivery into a scratch
path, the post-run scrub, and verified termination. Rotate the set (salt **and**
catalogue, then re-sign) if a pod is ever suspected of exfiltration.
