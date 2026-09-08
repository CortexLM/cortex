# Proof submit → score E2E (staging / local sim)

Operator check that `POST /v1/submissions` returns a **score** or an **explicit
fail-closed reason**. Healthz alone is not enough.

**Scope:** staging + disposable local sim. **Not** production.
**Never:** `set_weights`, master Lium rent, commit secrets.

Staging (operator-ready at time of writing): `can_score=true`,
`PROOF_FORCE_SIM=true`, `baseline_sealed=true`, offer
`openrouter-glm53flash-v0`, open topics `dt-no-ib-v0` and
`muon-vs-adamw-10m-v0`.

### Ownership: StubWin (A) vs reseal (B)

Skill-only `sim_document` uses `nll = (3.10 - 0.40 * skill).max(1.0)`.
Even skill=1.0 (and `StubScorer::win` skill=0.95) stays at NLL ≥ 1.0, so
a CPU-sealed ~0.29 baseline always trips `quality_floor`. Prefer **A**.

| Option | Owner | What |
|--------|--------|------|
| **A (lasting)** | this PR / code | Under `PROOF_FORCE_SIM`, a sealed topic emits harness numbers relative to the seal: holdout ≤ baseline+floor, splits ≤ baseline+`epsilon_topic_max_regress`, `tokens_per_sec` ≥ ref×(1+`epsilon_rel`). No extra host env. Lium never takes this path. |
| **B (ops, paused)** | Développeur | Reseal staging to `BASELINE_SKILL=0.40` (NLL ≈ 2.94), resign topics, retest. **Paused** — Mathis redirected that lane to prod RLM E2E (1× GPU). |

Do **not** reseal or edit staging host files from the code lane. Deploy A.

Local compose (`env-local.yml`) defaults `LOCAL_PROOF_FORCE_SIM=true`.
`PROOF_SIM_STUB_WIN` is a leftover no-op. Droplet overlays stay sim-off
(`assert-compose-matrix.sh`).

## Commands (in-repo, CI-safe)

```bash
# In-process HTTP contract (Sim + fail-closed 400/503). Always run.
./deploy/scripts/proof-submit-e2e.sh --http-tests

# Spawn proof-challenge --force-sim with synthetic topic/holdout/baseline/offer.
# Scores both staging topic ids. No Docker, no Lium.
./deploy/scripts/proof-submit-e2e.sh --local-sim

# Equivalent cargo invocations:
cargo test -p proof-http
cargo test -p proof-challenge-bin --test submit_e2e
cargo test -p ctx -- topic_list_reads_the_items_wrapper
```

Pass: every test above is green. Fail: any assertion on silent empty body,
missing `error`, missing `verdict` after 201, or holdout leak.

## Probe a running host (local compose or staging)

Do **not** point this at `https://network.cortex.foundation` or
`https://chain.joinbase.ai`.

```bash
# Auto-detect first healthy origin among loopback + documented staging URLs:
./deploy/scripts/proof-submit-e2e.sh --probe

# Or pin the origin (no trailing slash). Prefer the droplet IP on :80 —
# staging.api.joinbase.ai has historically answered a stale Lium/fail-closed
# instance while 159.223.159.205/challenge/proof is the ready sim host.
PROOF_E2E_BASE=http://127.0.0.1:28100 ./deploy/scripts/proof-submit-e2e.sh --probe
PROOF_E2E_BASE=http://159.223.159.205/challenge/proof \
  ./deploy/scripts/proof-submit-e2e.sh --probe

# Same contract as a Rust test (skip if unset):
PROOF_E2E_BASE=http://127.0.0.1:28100 cargo test -p proof-http --test live_submit_e2e
```

The probe **skips POST** when `eval_backend=lium` and `can_score=true`
(would rent a miner-paid pod). Staging sim is the intended POST target.

### Exact curl (Proof)

Gateway prefix on staging is `/challenge/proof` (reachable on the droplet
at `http://159.223.159.205/challenge/proof`; host-local
`http://127.0.0.1:8080/challenge/proof/...`). Direct service is `:8100`
(local overlay `:28100`). Do not POST to `staging.api.joinbase.ai` while
it still reports `eval_backend=lium`.

```bash
BASE="${PROOF_E2E_BASE:-http://127.0.0.1:28100}"   # or …/challenge/proof

curl -sS "$BASE/health"
# {"ok":true,"challenge_id":"proof","scoring_version":1}

curl -sS "$BASE/v1/status"
# {
#   "challenge_id": "proof",
#   "eval_backend": "sim",
#   "force_sim": true,
#   "sim_stub_win": true,
#   "can_score": true,
#   "baseline_sealed": true,
#   "open_topics": ["dt-no-ib-v0", "muon-vs-adamw-10m-v0"],
#   "inference_offer": { "offer_id": "openrouter-glm53flash-v0", "status": "open", ... }
# }
# Never contains api_key, base_url, or holdout records.

curl -sS "$BASE/v1/proof/topics"
# { "items": [ { "id": "dt-no-ib-v0", ... }, { "id": "muon-vs-adamw-10m-v0", ... } ] }
# Never contains content_sha256.

curl -sS -X POST "$BASE/v1/submissions" \
  -H 'content-type: application/json' \
  -d '{
    "miner_hotkey": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "topic_id": "dt-no-ib-v0",
    "artifact_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "claim": "beats the sealed reference under the cap",
    "declared_flops": 1000000000000,
    "manifest": { "train_dataset_ids": ["e2e-mix-v0"] }
  }'
```

### Exact ctx (Proof)

`ctx` talks to a **gateway** (`/challenge/proof/...`). For local compose:

```bash
ctx --gateway http://127.0.0.1:8080 proof status
ctx --gateway http://127.0.0.1:8080 proof topics
ctx --gateway http://127.0.0.1:8080 proof submit \
  --hotkey aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  --topic-id dt-no-ib-v0 \
  --artifact-digest bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
  --claim "beats the sealed reference under the cap" \
  --declared-flops 1000000000000 \
  --train-dataset e2e-mix-v0
ctx --gateway http://127.0.0.1:8080 proof show <id>
```

On staging, talk to the droplet gateway
(`http://159.223.159.205`, host-local `http://127.0.0.1:8080`). Do not
use `http://staging.api.joinbase.ai` while it still reports
`eval_backend=lium`. Do not send `X-Lium-Api-Key` over `http://` —
`ctx` refuses keyed cleartext.

Second topic (same body, different id):

```bash
# topic_id: muon-vs-adamw-10m-v0
```

### Expected response shapes (no secrets)

**201 scored** (`SubmitResp` then full row on GET):

```json
{
  "id": "pf_<16 hex>",
  "submission_digest": "<64 hex>",
  "topic_id": "dt-no-ib-v0",
  "state": "awaiting_admin",
  "eval_backend": "sim",
  "eligible": true
}
```

`state` may be `rejected` (gates failed) — that is still a **score**, not
silence. GET `/v1/submissions/{id}` then includes:

```json
{
  "id": "pf_…",
  "topic_id": "dt-no-ib-v0",
  "claim": "…",
  "declared_flops": 1000000000000,
  "state": "awaiting_admin",
  "verdict": {
    "pass": true,
    "agent": { "verdict": "clean", "reproduced": true, "rationale": "sim reproduced", "topic_id": "dt-no-ib-v0" },
    "harness": { "holdout_nll": 0.29, "tokens_per_sec": 84.8 },
    "failed": [],
    "lattice": 65535
  },
  "receipt_json": "{\"provider\":\"sim\",…}"
}
```

**400 / 503 fail-closed** (no row, no rent):

```json
{ "error": "topic_id is required" }
{ "error": "unknown topic" }
{ "error": "declared_flops exceeds the topic budget" }
{ "error": "inference offer missing; refuse scoring" }
```

A 2xx/4xx/5xx with an empty `{}` and no `id` / no `error` is a **fail**.

| HTTP | When | Stored? |
|------|------|---------|
| 201 | Sim (or stub) finished judge+harness | yes |
| 400 | bad/missing/unknown/not-open `topic_id`, bad hex, FLOP over budget | no |
| 503 | host cannot score (no open sealed topic, no offer, unpinned Lium, …) | no |

## Local compose (disposable)

`env-local.yml` defaults `LOCAL_PROOF_FORCE_SIM=true`. Droplet overlays
(`env-staging.yml` / `env-prod.yml`) must keep `PROOF_FORCE_SIM` (and the
leftover `PROOF_SIM_STUB_WIN`) false; `assert-compose-matrix.sh` fails if
they do not. A staging **host** may already have `PROOF_FORCE_SIM=true`
in `deploy/env/proof-challenge.env` — that is operator state, not a git
overlay. Do not reseal from this lane.

```bash
./deploy/scripts/materialize-env.sh
./deploy/scripts/local-e2e.sh --smoke --no-tunnel
# soft-probes Proof health + this submit contract when the service is up

# Minimal cleartext (no testnet):
docker compose -f docker-compose.yml -f docker-compose.e2e.yml up -d proof-challenge
# still needs operator files under deploy/secrets/proof/ or submits 503
# Prefer --local-sim (self-contained fixtures) when those files are absent.
```

`local-e2e.sh` will not invent a sha256 digest and will not POST if the
running host is Lium + `can_score`.

## Bounty smoke (optional)

```bash
./deploy/scripts/proof-submit-e2e.sh --bounty
# GET /v1/status → scoring_backend, can_score, backend_public_configured
# unconfigured feed: POST /v1/reports → 503 + error (no offline scorer)
# configured feed: thin POST must 401/400/503 — do not file a real report
```

## Pass / fail log (fill when you run)

| Step | Command | Result |
|------|---------|--------|
| In-process Sim submit | `cargo test -p proof-http sim_submit` | PASS (in-repo) |
| StubWin → awaiting_admin | `cargo test -p proof-http sim_stub_win_submit_reaches_awaiting_admin` | PASS (in-repo) |
| Sealed-relative win (0.29 NLL) | `cargo test -p proof-eval stub_win_clears_quality_floor` | PASS (in-repo) |
| Both staging topic ids | `cargo test -p proof-http sim_submit_accepts_staging_topic_ids` | PASS (in-repo) |
| Process-level `--force-sim` | `cargo test -p proof-challenge-bin --test submit_e2e` | PASS (in-repo) |
| Live probe | `PROOF_E2E_BASE=http://159.223.159.205/challenge/proof ./deploy/scripts/proof-submit-e2e.sh --probe` | PASS 201×2 `rejected` (see below) |
| Live Rust | `PROOF_E2E_BASE=http://159.223.159.205/challenge/proof cargo test -p proof-http --test live_submit_e2e` | PASS |

### Live staging 2026-09-07 (sim, no Lium, no merge)

Origin: `http://159.223.159.205/challenge/proof` (`eval_backend=sim`,
`force_sim=true`, `can_score=true`, `baseline_sealed=true`, offer
`openrouter-glm53flash-v0` open). Status has **no** `sim_stub_win` field
— host binary predates this PR / env is unset.

| topic | HTTP | id | state | eligible | gates |
|-------|------|----|-------|----------|-------|
| (empty) | 400 | — | — | — | `topic_id is required` |
| `not-a-real-topic` | 400 | — | — | — | `unknown topic` |
| `dt-no-ib-v0` | 201 | `pf_0000000000000002` | `rejected` | false | QualityFloor holdout 2.827 vs baseline 0.291 floor 0.02; split_regress; ThroughputMiss 113.9 vs 213.4 |
| `muon-vs-adamw-10m-v0` | 201 | `pf_0000000000000003` | `rejected` | false | NllMiss holdout 3.042 vs baseline 0.344 ε 0.02; split_regress |

Receipts: `"provider":"sim"`. Agent: `clean` / `reproduced`. No rent.
`staging.api.joinbase.ai` still answers `eval_backend=lium` /
`can_score=false` / empty topics — do not POST there.

To reach `awaiting_admin` on this host (**option A**): deploy this branch
(no host-file edit, no reseal). Status then reports `sim_stub_win=true`
whenever `eval_backend=sim`. Re-run `--probe`. **Option B** (reseal to
`BASELINE_SKILL=0.40` / NLL ≈ 2.94 and resign topics) is Développeur-only
— do not reseal from this lane. Admin adjudicate needs the host bearer at
`/opt/base/deploy/secrets/proof/admin_tokens` (do not log). No
`set_weights`.

## Related

- Miner HTTP: [`../external-miner/proof.md`](../external-miner/proof.md)
- Operator Proof: [`../PROOF.md`](../PROOF.md)
- Local stack: [`local-testnet-e2e.md`](local-testnet-e2e.md)
- Staging droplets: [`staging-testnet-e2e.md`](staging-testnet-e2e.md)
