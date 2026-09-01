# Relearn Multimodal (OFF)

> **Not live.** `relearn-mm` has no row in
> [`../config/challenges.toml`](../config/challenges.toml), so it has no
> emission and no leaf signed by its key can verify. The compose service sits
> behind the `mm` profile and never renders on a default or master stack;
> `deploy/scripts/assert-compose-matrix.sh` asserts both halves of that.
>
> It is off rather than deleted: the encoder pins and licence rules below are
> still the design, and turning it on is a trust-root ceremony (add a row with
> its own key, move bps out of the four live challenges) rather than a rewrite.
> The vision work now lives in [`RELEARN.md`](./RELEARN.md) — the base is a
> native VLM with captioning / VQA / OCR / spatial holdout families and a
> pixel-shuffle control — and the agentic work in
> [`RELEARN-AGENT.md`](./RELEARN-AGENT.md).

Control-plane notes. Miners start at [`external-miner/relearn-mm.md`](./external-miner/relearn-mm.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

Eval image and harness live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
This repo pins them in `config/relearn-mm-pin.toml`.

| Field | Value |
|-------|--------|
| `challenge_id` | `relearn-mm` |
| `challenge_scoring_version` | `1` |
| Language side | the live Relearn LLM champion (`Qwen/Qwen3.8-27B`) |
| Vision encoder pin | `google/siglip2-so400m-patch14-384` (Apache-2.0) |
| Accepted encoder licenses | Apache-2.0, MIT, BSD-2/3-Clause, ISC |
| Port | `8098` (`mm` profile only) |
| Emission | `0` — no trust-root row |

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). Operator promote is
`POST /v1/admin/promote`. Epoch emit is champion lattice; others `NoScore` (D24).

## Two gates, both mandatory

**Gate 1 — the LLM is intact.** The submitted language model is rerun on the
existing Relearn text holdout with the vision modules ignored. A drop past
`LM_EPSILON` (0.01 absolute) makes the submission worth **zero on this
challenge**, not a reduced score, no matter how good the vision numbers are.
This is the whole design: attaching an encoder must never become a way to get
paid for damaging the champion.

An **encoder-only** submission must additionally hash-match the champion LM
weights (`RELEARN_MM_CHAMPION_LM_HASH`, published on `/v1/status`). That hash is
the proof nothing on the text side moved at all. An **encoder-plus-LM**
submission may differ and is judged on the text score alone. Promoting an
encoder-plus-LM champion moves the reference hash, so the next encoder-only
submission is measured against the new language model.

**Gate 2 — the vision side improved.** A frozen image holdout across four task
families — captioning, VQA, OCR / text-in-image, spatial relations — plus
agentic traces where the model must look at a screenshot, diagram, or UI before
calling a tool. The families are deliberately **not** ImageNet or COCO test:
both sit in every candidate encoder's pretraining mix, so a score there would
measure memorization rather than sight.

Every agentic trace is replayed with the image pixels shuffled. A model that
really reads the image loses at least `MIN_SHUFFLE_DROP` (0.10); one that
pattern-matches the text prompt does not move, and a flat shuffle delta fails
the run. Zero traces cannot satisfy the control.

Both comparisons use the same bootstrap paired test as the text challenge, which
refuses a verdict below 100 decided examples — which is why the pin's
`agentic_traces` floor is 100 rather than a smaller, cheaper number.

## Gate summary

| Gate | Rule |
|------|------|
| LM intact | Text holdout ≥ champion − `LM_EPSILON`; hard zero on failure |
| LM weights | Encoder-only submissions must hash-match the champion LM |
| Vision displacement | Bootstrap paired test on the pooled image holdout |
| Task coverage | Every one of the four families must have items |
| Agentic displacement | Bootstrap paired test on the tool-using traces |
| Pixel shuffle | Shuffled-image score must fall by ≥ `MIN_SHUFFLE_DROP` |
| Encoder license | OSI-permissive only; OpenRAIL and non-commercial are refused |

`RELEARN_MM_FORCE_SIM=1` selects a deterministic offline eval for CI and local
development, reported on `/v1/status` as `eval_backend: sim`.
`deploy/scripts/assert-compose-matrix.sh` fails if a staging or prod overlay
enables it.
