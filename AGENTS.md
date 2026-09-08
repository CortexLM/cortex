# AGENTS.md — Cortex research network

Short contract for agents and operators. Prefer linking over restating runbooks.

**Product:** Cortex ([`CortexLM/cortex`](https://github.com/CortexLM/cortex)) — an autonomous research network on Bittensor. **Two live challenge ids:** `bounty` (2000 bps) and `proof` (8000 bps). Proof-weighted 20%/80% lock regardless of eval digest. Proof eval digest is pinned (`ghcr.io/cortexlm/proof-eval@sha256:78b614a1…`, RLM judge via digest-pinned `InferenceOffer`); live submits still 503 until harvest is wired, a baseline is sealed, and ≥1 topic is open. Empty digest stays fail-closed (do not invent a sha256). Sum is 10000. `relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and `prism` are **removed as products** — no trust-root row, no compose services, no emission, and no leaf may verify. Historical miner stubs stay under [`docs/external-miner/`](docs/external-miner/) so old links do not 404. Frozen specs (`docs/DESIGN_CHALLENGE.md`, `docs/PRISM.md`) remain for xtask gates. Leftover `prism-*` crates are the **Lium harvest stack** used by Proof, not a live Prism challenge. Proof scores operator-published research topics (dynamic `topic_id`, digest-pinned RLM judge, `wta` or `discovery` payout). Naming split (Cortex vs leftover `base` / `BASE_*`): [`docs/NAMING.md`](docs/NAMING.md).

**Vision vs implementation:** start with [`docs/OVERVIEW.md`](docs/OVERVIEW.md) and the [`whitepaper comparison`](docs/WHITEPAPER.md). Do not describe the proposed synthesiser, recursive research judge, durable research corpus, or automatic Proof emission as complete. Current Python judging is partial; Proof uses in-memory submission state and its binary does not drive the payout/emission helpers. A pinned image and `can_score` are not proof of scientific reproduction or end-to-end payment.

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
| `vm_orchestrator_token` (`PROOF_VM_ORCHESTRATOR_TOKEN_FILE` ↔ `PROOF_VM_AGENT_TOKEN_FILE`) | Proof CP ↔ KVM-host agent | Bearer **file** for the topic-VM orchestrator (`proof-vm-orchestrator`, HTTPS). Re-read per request on both sides, never logged, never on `/v1/status`. Not a wallet |
| Gateway owner wallet + `BASE_GATEWAY_REQUIRE_OWNER` | Gateway | Master-only **identity** check (live/prod). **Not** required to seal or serve `/v1/weights/latest` |
| Validator wallet | Validator | On-chain weight **submit** only — validators *fetch* sealed weights; they do not need a gateway wallet |

`GET /v1/weights/latest` is **fail-closed**: with no sealed bundle (or decode error) the gateway serves a **burn vector** (uid 0 = 100%, `sealed: false`) rather than 404. A missing gateway wallet is unrelated. Validators MUST NOT submit that unsealed vector and MUST NOT submit a persisted LKG seal while latest is unsealed. A sealed uid0=100% vector to the registered owner is also not a submit path.

## Challenge public docs (miner-facing repos)

Each live challenge has miner docs in this repo. Public repos (when they exist) must contain **only** human miner documentation plus example / test harness code — **never** control-plane, gateway, validator, or orchestrator source.

| Challenge | Public docs | Role |
|-----------|-------------|------|
| Bounty | this repo [`docs/external-miner/bounty.md`](docs/external-miner/bounty.md) | Miner pairing + report path; subnet **reads** CortexLM/backend public API (does not serve one) |
| Proof | this repo [`docs/external-miner/proof.md`](docs/external-miner/proof.md) | Dynamic operator-published topics + digest-pinned RLM judge |

This network implementation repo is `CortexLM/cortex`. Off/archived miner pointers stay under [`docs/external-miner/`](docs/external-miner/) (`relearn.md`, `relearn-image.md`, `relearn-agent.md`, `relearn-mm.md`) so historical links do not 404; they are not live products. Frozen specs (`docs/DESIGN_CHALLENGE.md`, `docs/PRISM.md`) stay archived. Do not send miners to Design, Prism, or Relearn docs as live work.

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
5. **Proof — submit:** `POST /v1/submissions` with a `topic_id`. Missing/unknown/not-open → **400** (no row); a custom topic without `artifact_uri` → **400** (no row). Empty `eval_image_digest`, missing/closed/misconfigured RLM judge `InferenceOffer`, missing judge API key, spoofed topic origin, missing/closed/non-`1x` `EvalExecutorOffer` (Lium path), zero open topics, or an unsealed baseline → **503**. Miners submit claim + code + FLOPs + artifact; they do not bind the judge offer or the executor offer. Contamination / empty manifest persist **rejected** without rent; on custom topics the runner's measured `flops_used` over the budget or over the miner's `declared_flops` persists **rejected** after the run, and a report without a measurement is **503** (no row). `GET /v1/proof/topics` must never leak holdout records.
6. **Proof — executor:** `GET /v1/proof/executor` is always 200 (`ready` + `reason`); `POST /v1/admin/proof/executor` (operator bearer) rotates or closes the live `1x` offer and 400s anything the pin refuses. Harvest rents the offer's `lium_template_id` at exactly `1x` (any other `rent_gpu_count` aborts before the rent) under `max_proof_deadline_s`; a run cut at the deadline is **503 + `stdout_tail`**. `PROOF_HARVEST_*` env only hot-swaps under the pin ceilings. Never a live Lium rent in CI.
7. **Proof — topic VMs (custom family):** the RLM runs in one Firecracker microVM per `topic_id` on a **dedicated KVM host** (`proof-vm-orchestrator`, HTTPS + bearer **file**), never on the droplet, never on Lium, never nested; every paid run is a **sister** Firecracker guest with **no network**, and the host stamps `sandboxed` / guest-measured `flops_used` on the report. `PROOF_VM_ORCHESTRATOR_URL` unset → `UnwiredVmOrchestrator` (503); URL set but token file missing/empty, `PROOF_RLM_VM_IMAGE_DIGEST` unpinned, agent down, or a `firecracker_required` run without the sister attestation → **503, no row, no host fallback**. `PROOF_VM_RUNNER_CUSTOM_IDS` is the only thing that registers a runner. The custom family is wired from that env alone — live orchestrator selected + ≥1 id → `FamilyMux::custom_only` when no Lium harvest is wired (custom topics score; `nll` / `throughput` → **503**, no row); never stage a placeholder Lium key to open custom topics, and the unwired stub never carries a mux. Hard `topic_id ↔ VM` bind on both sides (agent 409 `topic_mismatch`). Do not invent an RLM / sister image digest. **Zero live Firecracker in CI** — every test uses the fake hypervisor. Runbook: [`docs/runbooks/proof-vm-orchestrator.md`](docs/runbooks/proof-vm-orchestrator.md).
8. Leaf emission → `POST /v1/weights/raw` → seal → `GET /v1/weights/latest` with **`sealed: true`** (burn fallback alone is not a real seal).

**Never host Sim in staging/prod** for live scoring. `PROOF_FORCE_SIM=1` is CI/local opt-in only (`deploy/scripts/assert-compose-matrix.sh` fails if a droplet overlay sets one). Live Proof rent requires a digest pin in `config/proof-pin.toml` plus miner BYOK (`LIUM_API_KEY` / `X-Lium-Api-Key`). Never log or commit that key. Do not invent `eval_image_digest`.

**Bounty product rules:** pay is precision x severity, an unpriced `valid` row is not creditable, and the triage-noise ratio stays off the visible score. **Proof product rules (do not weaken):** topics are operator-published signed documents, not a git catalog; a topic may tighten a floor never loosen it; a baseline must be sealed to open; each topic is `wta` (winner takes the topic mass) or `discovery` (pass floor + novelty); global miner score is the **sum** of per-topic masses, not a mean of binary lattices; empty open set / empty eval digest fails closed (`503`); `custom` ids are **topic data** (any well-formed id drafts; a custom topic may **open** only when a runner is registered under its id, and the registry is **empty by default**); anti-cheat rules are a **vector carried by the signed topic** (re-versioned by the topic's RLM into the DB) and are ticked **before any paid inference** (one red item = persisted reject, no spend); the RLM runs **inside a per-topic VM** behind the `TopicVmOrchestrator` boundary (unwired stub = `503`), never on the control-plane host; the eval executor is exactly `1x` (pin `gpu_class`), a topic may only tighten `eval_executor.max_proof_deadline_s` / pin `require_offer_commitment`, and there is no per-topic `machine_id`. **Zero challenge content in git:** no benchmark, metric, model, rule list, repository, or topic catalog is compiled in — the first live topic is a signed document its RLM sets up (`crates/proof-rlm*`, `docs/PROOF.md` § Dynamic agentic engine).

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
| Purpose, research reuse, and proposal vs implementation | [`docs/OVERVIEW.md`](docs/OVERVIEW.md), [`docs/WHITEPAPER.md`](docs/WHITEPAPER.md) |
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
