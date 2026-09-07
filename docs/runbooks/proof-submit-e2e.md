# Proof submit → score E2E (staging / local sim)

Operator check that `POST /v1/submissions` returns a **score** or an **explicit
fail-closed reason**. Healthz alone is not enough.

**Scope:** staging + disposable local sim. **Not** production.
**Never:** `set_weights`, master Lium rent, commit secrets.

Staging (operator-ready at time of writing): `can_score=true`,
`PROOF_FORCE_SIM=true`, `baseline_sealed=true`, offer
`openrouter-glm53flash-v0`, open topics `dt-no-ib-v0` and
`muon-vs-adamw-10m-v0`.

### StubWin (required for `awaiting_admin` on a real seal)

Default `sim_document` uses `nll = (3.10 - 0.40 * skill).max(1.0)`. Even
skill=1.0 stays at NLL ≥ 1.0. A CPU-sealed staging baseline of ~0.29 then
always trips `quality_floor`. Test `StubScorer::win` only passes because
those baselines are also sim-derived (`BASELINE_SKILL=0.40`).

`PROOF_SIM_STUB_WIN=1` (and `PROOF_FORCE_SIM=true` so `eval_backend=sim`)
emits harness numbers **relative to the sealed vector**: NLL ≤ sealed +
quality floor, no split regress, primary beat by `epsilon_rel + 0.01`.
The Lium path never reads this env.

Enable on the **staging host env file** (not `env-staging.yml` — compose
overlays stay sim-off; `assert-compose-matrix.sh` fails if they set this):

```bash
# deploy/env/proof-challenge.env on cortex-staging (never commit)
PROOF_FORCE_SIM=true
PROOF_SIM_STUB_WIN=true
# restart proof-challenge, then:
curl -sS "$BASE/v1/status"
# expect eval_backend=sim, force_sim=true, sim_stub_win=true
```

Local compose (`env-local.yml`) defaults `LOCAL_PROOF_SIM_STUB_WIN=true`.

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

# Or pin the origin (no trailing slash):
PROOF_E2E_BASE=http://127.0.0.1:28100 ./deploy/scripts/proof-submit-e2e.sh --probe
PROOF_E2E_BASE=http://staging.api.joinbase.ai/challenge/proof \
  ./deploy/scripts/proof-submit-e2e.sh --probe

# Same contract as a Rust test (skip if unset):
PROOF_E2E_BASE=http://127.0.0.1:28100 cargo test -p proof-http --test live_submit_e2e
```

The probe **skips POST** when `eval_backend=lium` and `can_score=true`
(would rent a miner-paid pod). Staging sim is the intended POST target.

### Exact curl (Proof)

Gateway prefix on staging is `/challenge/proof`. Direct service is `:8100`
(local overlay `:28100`).

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

On staging, replace the gateway with the public staging API (cleartext
`http://staging.api.joinbase.ai` when that name resolves). Do not send
`X-Lium-Api-Key` over `http://` — `ctx` refuses keyed cleartext.

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

`env-local.yml` defaults `LOCAL_PROOF_FORCE_SIM=true` and
`LOCAL_PROOF_SIM_STUB_WIN=true`. Droplet overlays (`env-staging.yml` /
`env-prod.yml`) must keep both `PROOF_FORCE_SIM` and `PROOF_SIM_STUB_WIN`
false; `assert-compose-matrix.sh` fails if they do not. A **host** may set
them in `deploy/env/proof-challenge.env` on staging for this test window —
that is operator state, not a git overlay.

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
| In-process Sim submit | `cargo test -p proof-http sim_submit` | |
| StubWin → awaiting_admin | `cargo test -p proof-http sim_stub_win_submit_reaches_awaiting_admin` | |
| Sealed-relative win (0.29 NLL) | `cargo test -p proof-eval stub_win_clears_quality_floor` | |
| Both staging topic ids | `cargo test -p proof-http sim_submit_accepts_staging_topic_ids` | |
| Process-level `--force-sim` | `cargo test -p proof-challenge-bin --test submit_e2e` | |
| Live probe | `./deploy/scripts/proof-submit-e2e.sh --probe` | |
| Bounty | `./deploy/scripts/proof-submit-e2e.sh --bounty` | |

## Related

- Miner HTTP: [`../external-miner/proof.md`](../external-miner/proof.md)
- Operator Proof: [`../PROOF.md`](../PROOF.md)
- Local stack: [`local-testnet-e2e.md`](local-testnet-e2e.md)
- Staging droplets: [`staging-testnet-e2e.md`](staging-testnet-e2e.md)
