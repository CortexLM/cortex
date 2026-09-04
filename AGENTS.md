# AGENTS.md — Cortex control plane

Short contract for agents and operators. Prefer linking over restating runbooks.

**Product:** Cortex ([`CortexLM/cortex`](https://github.com/CortexLM/cortex)) — Bittensor subnet control plane. **Two live challenges:** `bounty` (2000 bps) and `proof` (8000 bps). Proof-weighted 20%/80% lock regardless of eval digest. Proof eval digest is pinned (`ghcr.io/cortexlm/proof-eval@sha256:78b614a1…`, RLM judge `Qwen/Qwen3.8-0.6B`); live submits still 503 until harvest is wired, a baseline is sealed, and ≥1 topic is open. Empty digest stays fail-closed (do not invent a sha256). Sum is 10000. `relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and `prism` are **removed as products** — no trust-root row, no compose services, no emission, and no leaf may verify. Historical miner stubs stay under [`docs/external-miner/`](docs/external-miner/) so old links do not 404. Frozen specs (`docs/DESIGN_CHALLENGE.md`, `docs/PRISM.md`) remain for xtask gates. Leftover `prism-*` crates are the **Lium harvest stack** used by Proof, not a live Prism challenge. Proof scores operator-published research topics (dynamic `topic_id`, digest-pinned RLM judge, `wta` or `discovery` payout). Naming split (Cortex vs leftover `base` / `BASE_*`): [`docs/NAMING.md`](docs/NAMING.md).

PRs require a [Greptile](https://greptile.com) review (`.greptile/`). If the bot is silent, comment `@greptileai review`.

## Monorepo map

| Path | Role |
|------|------|
| `bins/` | Runnable processes (validator, gateway, updater, challenges, …) |
| `crates/` | Libraries shared by binaries |
| `xtask/` | Repo gates and maintenance tasks |
| `deploy/` | Compose matrix, Terraform, pins, secrets helpers, remote deploy |
| `docs/` | Architecture, frozen specs, runbooks, completeness |
| `config/` | Shared non-secret configuration |

Working branch: **`main`**. Prod ships from annotated tags `v*.*.*` cut on `main`.

## Non-negotiables

- **Digest-only images** in deploy paths — no floating tags in prod pins/compose.
- **Secrets** via age + files under `deploy/env/` / `deploy/secrets/` — never baked into images or cloud-init.
- **Gateway runs on master only** (`--profile master` / `role-master.yml`). Validators point at the master gateway over VPC.
- **`evil-gateway` is test-only** — never enable on prod hosts; assert with `deploy/scripts/assert-evil-gateway-not-default.sh`.
- Platform is **DigitalOcean Droplets + Docker Compose**, not App Platform / DOKS.
- **Do not rename `BASE_*` env vars, deployed paths, or crypto domain tags.** They are measured into miner CVM `app-compose.json` and live on droplets / RTMR3 pin continuity. `CORTEX_*` is an accepted alias in `crates/config` only. See [`docs/NAMING.md`](docs/NAMING.md).
- Frozen specs (`docs/BUNDLE_SPEC.md`, `docs/DESIGN_CHALLENGE.md`) are pinned by xtask. Do not weaken gates or rewrite incentive / scoring / consensus semantics.
- `unsafe_code = forbid`. No `unwrap` / `expect` in non-test code.

## Wallet / key roles (do not conflate)

| Key | Who | Needed for |
|-----|-----|------------|
| `gateway_sk` | Gateway | Bundle **seal** signatures (`POST /v1/admin/seal`) |
| `gateway_admin_token` | Gateway + seal scripts | Bearer for **`/v1/admin/*`** (seal, backends, attest-grant). **Required** when `BASE_GATEWAY_REQUIRE_OWNER=1` |
| `bounty_sk` | Bounty / smoke | Signed bounty leaves; pub must match trust root |
| `proof_sk` | Proof / smoke | Signed `proof` leaves and topic documents; pub must match trust root |
| Gateway owner wallet + `BASE_GATEWAY_REQUIRE_OWNER` | Gateway | Master-only **identity** check (live/prod). **Not** required to seal or serve `/v1/weights/latest` |
| Validator wallet | Validator | On-chain weight **submit** only — validators *fetch* sealed weights; they do not need a gateway wallet |

`GET /v1/weights/latest` is **fail-closed**: with no sealed bundle (or decode error) the gateway serves a **burn vector** (uid 0 = 100%, `sealed: false`) rather than 404. A missing gateway wallet is unrelated. Validators MUST NOT submit that unsealed vector and MUST NOT submit a persisted LKG seal while latest is unsealed. A sealed uid0=100% vector to the registered owner is also not a submit path.

## Challenge public docs (miner-facing repos)

Each live challenge has miner docs in this repo. Public repos (when they exist) must contain **only** human miner documentation plus example / test harness code — **never** control-plane, gateway, validator, or orchestrator source.

| Challenge | Public docs | Role |
|-----------|-------------|------|
| Bounty | this repo [`docs/external-miner/bounty.md`](docs/external-miner/bounty.md) | Miner pairing + report path; subnet **reads** CortexLM/backend public API (does not serve one) |
| Proof | this repo [`docs/external-miner/proof.md`](docs/external-miner/proof.md) | Dynamic operator-published topics + digest-pinned RLM judge |

This control-plane repo is `CortexLM/cortex`. Off/archived miner pointers stay under [`docs/external-miner/`](docs/external-miner/) (`relearn.md`, `relearn-image.md`, `relearn-agent.md`, `relearn-mm.md`) so historical links do not 404; they are not live products. Frozen specs (`docs/DESIGN_CHALLENGE.md`, `docs/PRISM.md`) stay archived. Do not send miners to Design, Prism, or Relearn docs as live work.

**When a challenge product or public API changes**, agents **must** update:

1. The challenge’s public miner repo (README / examples), and
2. [`docs/external-miner/`](docs/external-miner/) in this monorepo as needed.

Do not leave miner-facing docs stale after shipping API, quota, round, or scoring changes.

## Challenge verification (mandatory path coverage)

When verifying a challenge (local-e2e, staging, or focused tests), **simulate a submission** end-to-end — do not stop at process healthz. Challenges evaluate on **master only**; the validator has **no challenge exec** (fetch sealed weights only).

1. Happy-path harness / intake POST (or equivalent) through the challenge service on master.
2. Edge / failure probes: bad harness, sanitize reject, quota, wrong routes/auth.
3. **Bounty — pair + report:** `ctx bounty pair --hotkey <ss58> --account-id <id> --accept-terms`, then `POST /v1/pair` (terms + signature) and `POST /v1/reports`. Operator bearer `POST /v1/admin/adjudicate` (`valid` / `already_fixed_not_prod` / `invalid_malicious` / `duplicate`). Scoring **reads** CortexLM/backend public JSON (`BOUNTY_BACKEND_PUBLIC_URL`); do not serve `/v1/public/*` from this repo.
4. **Bounty — fail-closed scorer:** the CortexLM/backend public feed is the only scorer. With no readable `BOUNTY_BACKEND_PUBLIC_URL`, `POST /v1/reports` must answer **503** and the emitter must pay **nobody** — it still covers `E` with `NoScore(ChallengeInternal)`, because a paid challenge with no leaves 409s the seal for every challenge. `BOUNTY_FORCE_SIM` is retired — do not reintroduce an offline bounty scorer. See [`docs/BOUNTY.md`](docs/BOUNTY.md).
5. **Proof — submit:** `POST /v1/submissions` with a `topic_id`. Missing/unknown/not-open → **400** (no row). Stale `inference_offer_id` / `config_commitment` → **400**. Empty `eval_image_digest`, missing/closed inference offer, zero open topics, or an unsealed baseline → **503**. Contamination / empty manifest persist **rejected** without rent. `GET /v1/proof/topics` must never leak holdout records.
6. Leaf emission → `POST /v1/weights/raw` → seal → `GET /v1/weights/latest` with **`sealed: true`** (burn fallback alone is not a real seal).

**Never host Sim in staging/prod** for live scoring. `PROOF_FORCE_SIM=1` is CI/local opt-in only (`deploy/scripts/assert-compose-matrix.sh` fails if a droplet overlay sets one). Live Proof rent requires a digest pin in `config/proof-pin.toml` plus miner BYOK (`LIUM_API_KEY` / `X-Lium-Api-Key`). Never log or commit that key. Do not invent `eval_image_digest`.

**Bounty product rules:** pay is precision x severity, an unpriced `valid` row is not creditable, and the triage-noise ratio stays off the visible score. **Proof product rules (do not weaken):** topics are operator-published signed documents, not a git catalog; a topic may tighten a floor never loosen it; a baseline must be sealed to open; each topic is `wta` (winner takes the topic mass) or `discovery` (pass floor + novelty); global miner score is the **sum** of per-topic masses, not a mean of binary lattices; empty open set / empty eval digest fails closed (`503`); `custom` unknown ids refuse at publish; `harness_success_rate` is listed and fail-closes until the real harness exists.

Local smoke automates the weights seal step via `weights-smoke` inside `./deploy/scripts/local-e2e.sh --smoke` (see [`deploy/AGENTS.md`](deploy/AGENTS.md) and [`docs/runbooks/local-testnet-e2e.md`](docs/runbooks/local-testnet-e2e.md)).

## Commands (local)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo run -p xtask -- loc-cap
cargo run -p xtask -- consensus-lint
cargo run -p xtask -- spec-check
cargo run -p xtask -- design-check
cargo run -p xtask -- external-docs-check
```

Commit subjects: `type(scope): summary` (lowercase, ≤72 chars). Hooks: `./scripts/install-githooks.sh`.

## Required gates (before merge)

Match CI (`.github/workflows/ci.yml`):

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- tests + `cargo deny`
- `cargo run -p xtask -- loc-cap`
- `cargo run -p xtask -- consensus-lint`
- `cargo run -p xtask -- spec-check`
- `cargo run -p xtask -- design-check`
- `cargo run -p xtask -- external-docs-check`
- Greptile review (template checkbox; `@greptileai review` if silent)

## Where to read what

| Need | Start here |
|------|------------|
| System map / process topology | [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) |
| Cortex vs leftover `base` names | [`docs/NAMING.md`](docs/NAMING.md) |
| Deploy / Compose / DO topology | [`deploy/README.md`](deploy/README.md) + [`deploy/AGENTS.md`](deploy/AGENTS.md) |
| **Local full-subnet test** (master+gateway+validator on testnet 541 + tunnel) | [`docs/runbooks/local-testnet-e2e.md`](docs/runbooks/local-testnet-e2e.md) · [`deploy/AGENTS.md`](deploy/AGENTS.md) § Local testnet E2E · `./deploy/scripts/local-e2e.sh --help` |
| Doc authority vs evidence | [`docs/AGENTS.md`](docs/AGENTS.md) |
| Component status | [`docs/COMPLETENESS.md`](docs/COMPLETENESS.md) |
| Frozen contracts | [`docs/BUNDLE_SPEC.md`](docs/BUNDLE_SPEC.md), [`docs/DESIGN_CHALLENGE.md`](docs/DESIGN_CHALLENGE.md), [`docs/PRISM.md`](docs/PRISM.md) |
| Bounty miners | [`docs/external-miner/bounty.md`](docs/external-miner/bounty.md) · operator spec: [`docs/BOUNTY.md`](docs/BOUNTY.md) |
| Proof miners | [`docs/external-miner/proof.md`](docs/external-miner/proof.md) · operator spec: [`docs/PROOF.md`](docs/PROOF.md) |
| Validators | [`docs/external-miner/validators.md`](docs/external-miner/validators.md) |
| Threat / operator checklist | [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md), [`docs/OPERATOR_SECURITY.md`](docs/OPERATOR_SECURITY.md) |

## Do not commit

- `deploy/env/*.env` (materialized secrets)
- `deploy/secrets/**` (except documented `README.md` placeholders)
- `deploy/terraform/*.tfstate*` / `terraform.tfvars` / local `.terraform/`
- Age identities, wallets, `receipt_sk`, `*.pem` / `*.key` / `*.age`
- Treating `docs/evidence/` or `docs/spikes/` as product code or normative spec
