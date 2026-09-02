# Relearn (live challenge)

Sibling challenges: [`RELEARN-IMAGE.md`](./RELEARN-IMAGE.md) (`relearn-image`,
image generation judged by Q-Judger) and [`RELEARN-AGENT.md`](./RELEARN-AGENT.md)
(`relearn-agent`, replayed tool traces on **this challenge's base checkpoint**,
not on its champion). They share the champion-versus-challenger holdout shape
and the Lium payment model, and each signs leaves under its own key.
[`RELEARN-MM.md`](./RELEARN-MM.md) is off.

Control-plane notes. Miners start at [`external-miner/relearn.md`](./external-miner/relearn.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

Eval image and harness live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
This repo pins them in `config/relearn-pin.toml`.

| Field | Value |
|-------|--------|
| `challenge_id` | `relearn` |
| `challenge_scoring_version` | `1` |
| Base model | `Qwen/Qwen3.8-27B` (Apache-2.0, native VLM). Not Flash-Next. |
| Teacher weights | `incoai/GLM-5.3-NVFP4` (full GLM-5.3 NVFP4) — download, then serve from `RELEARN_TEACHER_LOCAL_DIR`. Never pass the Hugging Face repo id to vLLM. Not Flash. Never DFlash2 (CC BY-NC-ND). |
| Teacher / judge | HTTP API (operator sets `RELEARN_TEACHER_*`; wire id `glm-5.3`) |
| Operator GPUs | 2×B300, tensor-parallel 2 (docs only). If OOM, raise tp on those 2 GPUs — do not add an 8-GPU layout. |
| Port | `8095` (local host `28095`) |
| Emission | `4000` bps (default) |

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). Operator promote is
`POST /v1/admin/promote`. Epoch emit is champion lattice; others `NoScore` (D24).

## Who is allowed to produce a score

The deterministic offline harness is not a fallback. A host scores only when
one of these holds:

| Condition | `POST /v1/submissions` |
|-----------|------------------------|
| `eval_image_digest` is a `sha256:` pin | live eval on a digest-pinned Lium pod |
| `RELEARN_FORCE_SIM=1` (CI / local only) | sim, reported as `eval_backend: "sim"` |
| neither | **503** — `eval image digest not pinned` |

`GET /v1/status` publishes `eval_backend`, `force_sim`, `can_score`,
`live_harvest_wired`, and `champion_baseline_recorded`, and every submit row
echoes `eval_backend`, so a sim run is never mistaken for a real verdict.

`eval_image_digest` is pinned (`CortexLM/relearn` PR #2), so the image half is
done. A live host must still wire the harvest and record a champion baseline
before `can_score` turns true; each has its own 503.

A refusal is not a submission: nothing is persisted unless scoring produced a
verdict, so a 503 leaves no row behind.

## Champion baseline (required on a live host)

Every gate is a comparison against the champion — the paired holdout test, the
public–holdout gap, the general-bench canary, pixel-shuffle. With no champion
recorded there is nothing to compare against and submissions answer **503
`no champion baseline recorded`** before any gate runs.

The baseline must come from the same scorer submissions face. A sim host uses
the sim baseline. A live host has two sources, tried in this order:

1. **Operator-recorded measurement** — `RELEARN_BASE_CHAMPION_FILE`. Run the
   digest-pinned eval image on `base_model` once and install the result. It is
   verified at boot against the pin's `eval_image_digest` **and**
   `holdout_commitment`, so a measurement from another image or another holdout
   is refused, as is one missing a series the gates read.
2. **Wired harvest** — the live scorer below, which measures the base model
   through the eval image at boot.

A live host never falls back to the sim baseline. Judging a live challenger
against simulated champion scores would let any artifact displace a champion
that was never measured.

```json
{
  "eval_image_digest": "sha256:<the pin's digest>",
  "holdout_commitment": "<the pin's commitment>",
  "holdout":         { "<item key>": 0.0 },
  "public":          { "<item key>": 0.0 },
  "perturbed":       { "<item key>": 0.0 },
  "canaries":        { "<item key>": 0.0 },
  "general_canary":  { "<item key>": 0.0 },
  "agent_trace": 0.0,
  "vision_shuffle": { "captioning": { "items": 40, "score": 0.0, "shuffled_score": 0.0 } }
}
```

`holdout` must carry one entry per verified holdout item. `public` and
`general_canary` must be non-empty: a champion the gates cannot read would
reject every challenger for a reason the miner cannot act on, so that is
refused at boot instead. The file is operator state — never commit it.

## Eval image contract

Live scoring runs the digest-pinned `eval_image` on a Lium pod. The control
plane client is [`crates/relearn-lium-harvest`](../crates/relearn-lium-harvest);
the scoring code itself lives in
[`CortexLM/relearn`](https://github.com/CortexLM/relearn). Nothing in this repo
can compute a live score, and there is no sim fallback: a pod that does not
return a well-formed, correctly bound document is a 503.

Per run the control plane:

1. Boots `eval_image@<digest>` with the master SSH public key
   (`LIUM_SSH_PUBLIC_KEY_FILE`) on a pod the **miner** pays for
   (`LIUM_API_KEY`), under the price / GPU / lifetime guardrails.
2. Writes `request.json` and `teacher.env` into `/tmp/relearn_eval` over stdin
   — nothing is interpolated into the remote command.
3. Sources `teacher.env` with `set -a`, then runs
   `/usr/bin/relearn-eval` (else `command -v relearn-eval`)
   `score --request request.json --out metrics.json`. A non-interactive
   SSH PATH that cannot see the binary is why a pod exits 127 with no
   `RELEARN_EVAL_OK`.
4. Reads back `RELEARN_METRICS=<document>` and `RELEARN_EVAL_OK`.
5. Scrubs the workdir, terminates, and **requires verified termination** before
   accepting any score. An orphan pod keeps spending the miner's money, so it
   outranks whatever the run returned.

### Pod environment

A Lium `InstanceSpec` has no env field, so the pod inherits **nothing** from
the master. Everything the image reads from its environment has to be handed to
it, and a missing judge URL is why a pod can boot, run, and never print
`RELEARN_EVAL_OK`:

| Variable | Source | Required |
|----------|--------|----------|
| `RELEARN_TEACHER_API_URL` | host env | **yes** — no judge, no score |
| `RELEARN_TEACHER_MODEL` | host env, else the pin's `teacher_model` | yes (defaulted) |
| `RELEARN_TEACHER_API_KEY` | host env | only if the teacher API requires auth |
| `RELEARN_BASE_MODEL` | the pin's `base_model` | yes |
| `RELEARN_BASE_MODEL_DIR` | host env | **pod path** to Qwen (e.g. `/models/base`). Not a teacher-host path. Required unless `RELEARN_ALLOW_MODEL_DOWNLOAD=1` |
| `HF_HOME` / `HF_HUB_CACHE` | host env | optional pod cache paths after a first pull |
| `RELEARN_ALLOW_MODEL_DOWNLOAD` | host env | first champion pull only. **Never defaulted.** After Lium caches weights, pin `RELEARN_BASE_MODEL_DIR` to that path and unset this |

Only the names are in git. Values are operator state, delivered in
`teacher.env` over stdin with `umask 077` — never on the remote command line,
where the key would sit in the pod's process table. A host missing the judge
URL, or missing both `RELEARN_BASE_MODEL_DIR` and
`RELEARN_ALLOW_MODEL_DOWNLOAD=1`, reports `can_score: false` and
`base_weights.primed: false` and refuses **before** renting, rather than
paying for a pod that cannot score. The image does not bake Qwen. `/v1/status`
publishes the priming **var name** only, never the path.

**`RELEARN_TEACHER_API_KEY` crosses to a miner-paid pod.** Use a teacher
credential scoped and rate-limited for this purpose, and rotate it on
suspicion; the pod could otherwise spend the operator's teacher quota. If the
teacher API can be reached from the pod without auth (network-restricted),
leave the key unset and nothing is forwarded.

A contaminated or empty-evidence `manifest` is rejected **before** the rent
as well: those submissions cannot produce a lattice score, so they must not
spend a pod or the teacher key.

### Diagnosing a run that produced no marker

The remote command echoes `exit=<rc>` and the last 8 KiB of `run.log`. When the
`RELEARN_EVAL_OK` marker is missing, the control plane returns that tail in the
503 (2 KiB) and logs it (8 KiB), with the metrics document and marker lines
stripped and the forwarded secrets redacted — the tail is miner-visible.

The image must print `RELEARN_METRICS=` followed by one line of JSON, then
`RELEARN_EVAL_OK` on success. The document is the baseline envelope above plus
the run identity:

```json
{
  "schema_version": 1,
  "submission_digest": "<echo of the request>",
  "artifact_digest": "<echo of the request>",
  "eval_image_digest": "sha256:…",
  "holdout_commitment": "…",
  "holdout": {}, "public": {}, "perturbed": {},
  "canaries": {}, "general_canary": {},
  "agent_trace": 0.0,
  "vision_shuffle": {}
}
```

`submission_digest` and `artifact_digest` are checked against what was asked, so
a pod cannot answer with another artifact's numbers or replay an earlier run.
`eval_image_digest` and `holdout_commitment` are checked against the pin. Any
mismatch is a 503, not a score.

**The request carries the holdout items.** The pod sees the private split for
the duration of the run — a model cannot be scored on prompts it is not shown.
The mitigations are the digest-pinned image, delivery into `/tmp/relearn_eval`
rather than any persisted path, the post-run scrub, and verified termination.
Rotate the holdout (private salt **and** catalog, then re-sign) if a pod is ever
suspected of exfiltration; the commitment makes drift detectable but not
exfiltration. Operators who consider that exposure unacceptable should keep the
challenge on the recorded-baseline path and treat live scoring as gated on a
future in-enclave design.

## Holdout and anti-overfit gates

The holdout is **not** in git. `config/relearn-pin.toml` carries only
`holdout_commitment` and `holdout_size`. Records come from
`RELEARN_HOLDOUT_FILE` and are verified at boot; a missing or mismatched file
means submissions answer **503** rather than scoring a reconstructable seed
or the public split.

The committed commitment is the CI / local one (a documented dev salt over a
synthetic catalog). It is **not** the live seal — see the ceremony step below.

Promotion requires every gate:

| Gate | Rule |
|------|------|
| Holdout displacement | Bootstrap paired test on the private split (the only series that may enter the lattice) |
| Public–holdout gap | Public far above holdout signals memorization; empty public is fail-closed |
| Contamination | Any holdout id / image hash in submitted training metadata rejects the run. An **undeclared** `manifest` is `contamination_evidence_missing`, not a pass: absence of evidence cannot clear the gate |
| Pixel shuffle | Every vision family present in the holdout (caption / VQA / OCR / spatial) must drop ≥ `MIN_SHUFFLE_DROP` when pixels are shuffled |
| General-bench canary | MMLU / MMMU-style slice is **off** the visible score. Regression past `CANARY_EPSILON` vs the champion is a hard zero |
| Perturbation / base canaries / agent-trace | Existing retention floors still apply |

```bash
# Rotate the holdout (records never enter git). Never reuse the T2I/dev salt.
cargo run -p xtask -- relearn-holdout \
  --catalog ~/.base-secrets/relearn-catalog.json \
  --salt "$RELEARN_HOLDOUT_SALT" --size 120 \
  --exclude 1 --exclude 2 … \
  --out deploy/secrets/relearn/holdout.json
```

Install the printed `holdout_commitment` on the host as
`RELEARN_HOLDOUT_COMMITMENT` (or a secret-store file). Do **not** paste it
into `config/relearn-pin.toml` — that file stays the public CI fixture, and
live scoring refuses a pin that equals the fixture. Ceremony:
[`../config/CEREMONY.md`](../config/CEREMONY.md).

Production must rotate the salt **and** the catalog. Keeping the committed
CI commitment on a live host means the split is reconstructable from public
material. The records themselves never enter git, and neither does the salt.
