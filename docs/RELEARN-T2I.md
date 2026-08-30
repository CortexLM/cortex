# Relearn T2I (live challenge)

Control-plane notes. Miners start at [`external-miner/relearn-t2i.md`](./external-miner/relearn-t2i.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

Eval image and harness live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
This repo pins them in `config/relearn-t2i-pin.toml`.

| Field | Value |
|-------|--------|
| `challenge_id` | `relearn-t2i` |
| `challenge_scoring_version` | `1` |
| Generator seed | `nvidia/Cosmos3-Super-Text2Image` (OpenMDW 1.1) |
| Judge | Q-Judger — `Qwen/Qwen-Image-Bench` (Apache-2.0, from Qwen3.6-27B) |
| Prompt set | `Qwen/Qwen-Image-Bench` dataset, ids 1..=1000 |
| Port | `8097` (local host `28097`) |
| Emission | `1500` bps |

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
| Contamination | Any eval prompt id in the submitted training metadata rejects the submission |
| Judge N/A rate | Above 25 % the run is void, never a score of zero |
| Public–holdout gap | Public far above holdout signals memorization |

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
