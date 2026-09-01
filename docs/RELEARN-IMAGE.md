# Relearn Image (live challenge)

Control-plane notes. Miners start at [`external-miner/relearn-image.md`](./external-miner/relearn-image.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

Eval image and harness live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
This repo pins them in `config/relearn-t2i-pin.toml`.

| Field | Value |
|-------|--------|
| `challenge_id` | `relearn-image` |
| `challenge_scoring_version` | `1` |
| Generator seed | `nvidia/Cosmos3-Super-Text2Image` (OpenMDW 1.1) |
| Judge | Q-Judger — `Qwen/Qwen-Image-Bench` (Apache-2.0, from Qwen3.6-27B) |
| Prompt set | `Qwen/Qwen-Image-Bench` dataset, ids 1..=1000 |
| Port | `8097` (local host `28097`) |
| Emission | `1500` bps |
| Crates / service / env prefix | `relearn-t2i-*`, `relearn-t2i-challenge`, `RELEARN_T2I_*` — the pre-launch spelling, kept because the domain tags are hashed into the committed holdout commitment ([`NAMING.md`](./NAMING.md)) |

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). Operator promote is
`POST /v1/admin/promote`. Epoch emit is champion lattice; others `NoScore` (D24).

## Non-negotiables

- **Flux is rejected.** `relearn_t2i_task::base_is_rejected` refuses any
  Flux-family base at pin load, at submit, and before scoring. Its weights are
  non-commercial, which is incoherent for a subnet that pays for
  redistributable artifacts.
- **Q-Judger is the only judge.** `relearn_t2i_judge::assert_judge_model`
  refuses any other model, and the pin refuses to load with a different
  `judge_model`. Judge inference is fixed by the model card: seed 42,
  temperature 0, `top_k` 1, `top_p` 1.0, repetition penalty 1.05, thinking on,
  4096 max new tokens.
- **Eval prompts are frozen.** NVIDIA recommends upsampling a prompt into a
  JSON document before generation. That is fine for a miner's own training and
  fatal for a benchmark, so the scored strings live in the pin and are replayed
  verbatim. Miners do not bring an upsampler to the scored split.
- **Same prompt ids, same seeds, every miner.** Seeds are
  `sha256(domain ‖ pin_salt ‖ prompt_id ‖ variation_index)`, published on
  `GET /v1/prompts` for the public split so any miner can reproduce a cell.
- **The holdout is not in git.** `config/relearn-t2i-pin.toml` carries only
  `holdout_commitment` and `holdout_size`. Records come from
  `RELEARN_T2I_HOLDOUT_FILE` and are verified against the commitment at boot; a
  wrong file means submissions answer 503 rather than scoring the public split.

## Scoring

Q-Judger returns a thinking trace followed by a JSON score tree over five L1
pillars. Raw `0|1|2` map to `0|60|100`; `N/A` is **excluded, not zeroed** (a
prompt where a criterion does not apply must not be punished). Level 3 averages
into level 2, level 2 into level 1, and the five pillars into the total, exactly
as the paper specifies. Series are normalized to `0..=1` so one
`prism_competition` dead-zone unit equals one paper point.

Promotion requires every gate:

| Gate | Rule |
|------|------|
| Holdout displacement | Bootstrap paired test on the private prompt split (the published split is informational) |
| Paired A/B | Same `(prompt_id, seed)` cells; win rate must be at least 5000 bps |
| Pillar regression | No L1 pillar may drop more than `PILLAR_EPSILON` (2 paper points). A large Alignment gain cannot hide a Quality collapse |
| Seed replay | Three pinned cells regenerated; exact image hash, or embedding drift ≤ `MAX_REPLAY_DRIFT` |
| Prompt faithfulness | ≥ 8 agentic spot checks (counts, rendered text, spatial relations) agreeing with Q-Judger Alignment ≥ 75 % |
| Contamination | Any eval prompt id in the submitted training metadata rejects the submission. An **undeclared** manifest is `contamination_evidence_missing`, not a pass: absence of evidence cannot clear the gate |
| Capability canary | Off the visible score when measured. The published eval image does not emit this series (faithfulness + seed-replay are its off-score controls), so **both-empty is a skip**. One-sided absence, or a drop past `CANARY_EPSILON` when both sides measured it, is a hard zero |
| Judge N/A rate | Above 25 % the run is void, never a score of zero |
| Public–holdout gap | Public far above holdout signals memorization; empty public is fail-closed |

## Operator

```bash
# Rotate the holdout (records never enter git).
cargo run -p xtask -- relearn-t2i-holdout \
  --bench ~/.base-secrets/qwen_image_bench_hf_v0518.jsonl \
  --salt "$RELEARN_T2I_HOLDOUT_SALT" --size 40 \
  --exclude 1 --exclude 26 …  \
  --out deploy/secrets/relearn-t2i/holdout.json
```

Paste the printed `holdout_commitment` into `config/relearn-t2i-pin.toml`, then
re-sign the trust root ([`../config/CEREMONY.md`](../config/CEREMONY.md)).

`RELEARN_T2I_FORCE_SIM=1` selects a deterministic offline judge for CI and local
development. It is reported on `/v1/status` as `judge_backend: sim` so it cannot
be mistaken for a real run, and `deploy/scripts/assert-compose-matrix.sh` fails
if a staging or prod overlay enables it.

## Who is allowed to produce a score

The offline judge is not a fallback. A host scores only when one of these holds:

| Condition | `POST /v1/submissions` |
|-----------|------------------------|
| `RELEARN_T2I_FORCE_SIM=1` (CI / local only) | sim, reported as `judge_backend: "sim"` |
| digest-pinned `eval_image_digest` **and** a wired harvest **and** a recorded champion baseline | live eval on a digest-pinned Lium pod |
| anything less | **503** — naming the first missing piece |

`GET /v1/status` publishes `judge_backend`, `force_sim`, `can_score`,
`live_harvest_wired`, and `champion_baseline_recorded`, so a sim run is never
mistaken for a real verdict and an operator can see which piece is missing.

A refusal is not a submission: nothing is persisted unless scoring produced a
verdict, so a 503 leaves no row behind.

## Champion baseline (required on a live host)

Every gate is a comparison against the champion. With none recorded,
submissions answer **503 `no champion baseline recorded`** before any gate
runs. The baseline must come from the same scorer submissions face: a sim host
uses the sim baseline, and a live host uses either an operator measurement
(`RELEARN_T2I_BASE_CHAMPION_FILE`, verified at boot against the pin's
`eval_image_digest` **and** `holdout_commitment`) or the wired harvest. A live
host never falls back to sim numbers — judging a live challenger against a
simulated champion would let any artifact displace a champion that was never
measured.

## Eval image contract

Live scoring runs the digest-pinned `eval_image` on a Lium pod. The
control-plane client is [`crates/relearn-t2i-harvest`](../crates/relearn-t2i-harvest);
Cosmos3 generation and the Q-Judger pass both live in
[`CortexLM/relearn`](https://github.com/CortexLM/relearn). Nothing in this repo
can compute a live score.

Per run the control plane boots `eval_image@<digest>` with the master SSH
public key on a pod the **miner** pays for, writes `request.json` into
`/tmp/relearn_image_eval` over stdin (run inputs are never interpolated into
the remote command), runs `/usr/bin/relearn-image-eval` (else
`command -v relearn-image-eval`) `score`, reads back
`RELEARN_METRICS=<document>` and `RELEARN_EVAL_OK`, scrubs the
workdir, and **requires verified termination** before accepting any score. An
orphan pod keeps spending the miner's money, so it outranks whatever the run
returned.

### Pod environment

A Lium `InstanceSpec` has no env field, so the pod inherits **nothing** from
the master. Everything the image reads from its environment has to be handed to
it, and a missing judge URL is why a pod can boot, run, and never print
`RELEARN_EVAL_OK`:

| Variable | Source | Required |
|----------|--------|----------|
| `RELEARN_T2I_JUDGE_API_URL` | host env | **yes** — no judge, no score |
| `RELEARN_T2I_JUDGE_MODEL` | host env, else the pin's `judge_model` | yes (defaulted) |
| `RELEARN_T2I_JUDGE_API_KEY` | host env | only if the judge API requires auth |
| `RELEARN_T2I_BASE_MODEL` | the pin's `base` | yes |

Only the names are in git. Values are operator state, delivered in
`teacher.env` over stdin with `umask 077` — never on the remote command line,
where the key would sit in the pod's process table. A host missing the judge
URL reports `can_score: false` and refuses **before** renting, rather than
paying for a pod that cannot score.

**`RELEARN_T2I_JUDGE_API_KEY` crosses to a miner-paid pod.** Use a judge
credential scoped and rate-limited for this purpose, and rotate it on
suspicion; the pod could otherwise spend the operator's Q-Judger quota. If the
judge API can be reached from the pod without auth (network-restricted),
leave the key unset and nothing is forwarded.

A contaminated or empty-evidence `manifest` is rejected **before** the rent
as well: those submissions cannot produce a lattice score, so they must not
spend a pod or the judge key.

The markers are the language challenge's markers. What keeps a digest pinned
into the wrong challenge from scoring something else is `challenge_id` inside
the document and the non-overlapping series keys (`p<id>#v<n>` here), not a
different prefix.

The request carries the frozen prompt strings, the seed lattice (`pin_salt`,
`variations_per_prompt`), the sampler, and the miner's manifest. The image
derives cells itself. `submission_digest` and `artifact_digest` are checked
against what was asked, and `eval_image_digest` / `holdout_commitment` against
the pin. Any mismatch is a 503, not a score.

**The request carries the holdout prompts.** The pod sees the private split for
the duration of the run — a generator cannot be scored on prompts it is not
shown. The mitigations are the digest-pinned image, delivery into a scratch
path, the post-run scrub, and verified termination. Rotate the holdout (salt
**and** bench snapshot, then re-sign) if a pod is ever suspected of
exfiltration.
