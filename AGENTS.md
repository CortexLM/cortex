# AGENTS.md — Cortex control plane

Short contract for agents and operators. Prefer linking over restating runbooks.

**Product:** Cortex ([`CortexLM/cortex`](https://github.com/CortexLM/cortex)) — Bittensor subnet control plane. **Four live challenges:** `relearn` (4000 bps), `relearn-image` (1500), `relearn-agent` (1500), `bounty` (3000). Encoder-attach Multimodal (`relearn-mm`) is **off** — no trust-root row, `mm` compose profile only. `relearn` and `relearn-agent` post-train the **same** base `Qwen/Qwen3.8-27B` (teacher `incoai/GLM-5.3-NVFP4` from `RELEARN_TEACHER_LOCAL_DIR`); Agent is a separate challenge scored on replayed tool traces, not a rename of `relearn`. Relearn eval images live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn). Naming split (Cortex vs leftover `base` / `BASE_*`, and `relearn-image` vs the `relearn-t2i-*` crates): [`docs/NAMING.md`](docs/NAMING.md).

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
| `relearn_sk` | Relearn / smoke | Signed leaves (`POST /v1/weights/raw`); pub must match trust root |
| `relearn_t2i_sk` | Relearn Image | Signed `relearn-image` leaves; pub must match trust root |
| `relearn_agent_sk` | Relearn Agent | Signed `relearn-agent` leaves; pub must match trust root |
| `bounty_sk` | Bounty / smoke | Signed bounty leaves; pub must match trust root |
| Gateway owner wallet + `BASE_GATEWAY_REQUIRE_OWNER` | Gateway | Master-only **identity** check (live/prod). **Not** required to seal or serve `/v1/weights/latest` |
| Validator wallet | Validator | On-chain weight **submit** only — validators *fetch* sealed weights; they do not need a gateway wallet |

`GET /v1/weights/latest` is **fail-closed**: with no sealed bundle (or decode error) the gateway serves a **burn vector** (uid 0 = 100%, `sealed: false`) rather than 404. A missing gateway wallet is unrelated. Validators MUST NOT submit that unsealed vector and MUST NOT submit a persisted LKG seal while latest is unsealed. A sealed uid0=100% vector to the registered owner is also not a submit path.

## Challenge public docs (miner-facing repos)

Each live challenge has a **separate public GitHub repo** for miners. Those repos must contain **only** human miner documentation plus example / test harness code — **never** control-plane, gateway, validator, or orchestrator source. Public repos use a `docs/` layout (hero README + banner under `assets/`).

| Challenge | Public repo | Role |
|-----------|-------------|------|
| Relearn | [`CortexLM/relearn`](https://github.com/CortexLM/relearn) | Eval image, harness, generators, teacher, miner docs |
| Relearn Image | [`CortexLM/relearn`](https://github.com/CortexLM/relearn) | Cosmos3 fine-tune harness + Q-Judger runner; in-repo pointer [`docs/external-miner/relearn-image.md`](docs/external-miner/relearn-image.md) |
| Relearn Agent | [`CortexLM/relearn`](https://github.com/CortexLM/relearn) | Episode environment, trace replay, ablation arms; in-repo pointer [`docs/external-miner/relearn-agent.md`](docs/external-miner/relearn-agent.md) |
| Bounty | this repo [`docs/external-miner/bounty.md`](docs/external-miner/bounty.md) | Miner pairing + report path; subnet **reads** CortexLM/backend public API (does not serve one) |

This control-plane repo is `CortexLM/cortex`. Short miner pointers: [`docs/external-miner/relearn.md`](docs/external-miner/relearn.md), [`docs/external-miner/relearn-image.md`](docs/external-miner/relearn-image.md), [`docs/external-miner/relearn-agent.md`](docs/external-miner/relearn-agent.md), [`docs/external-miner/bounty.md`](docs/external-miner/bounty.md). Historical frozen specs (`docs/DESIGN_CHALLENGE.md`, `docs/PRISM.md`) stay archived; they are not live products. Do not send miners to Design or Prism docs.

**When a challenge product or public API changes**, agents **must** update:

1. The challenge’s public miner repo (README / examples), and
2. [`docs/external-miner/`](docs/external-miner/) in this monorepo as needed.

Do not leave miner-facing docs stale after shipping API, quota, round, or scoring changes.

## Challenge verification (mandatory path coverage)

When verifying a challenge (local-e2e, staging, or focused tests), **simulate a submission** end-to-end — do not stop at process healthz. Challenges evaluate on **master only**; the validator has **no challenge exec** (fetch sealed weights only).

1. Happy-path harness / intake POST (or equivalent) through the challenge service on master.
2. Edge / failure probes: bad harness, sanitize reject, quota, wrong routes/auth.
3. **Relearn — submit:** `POST /v1/submissions` with a 64-hex hotkey + artifact digest (optional `X-Lium-Api-Key`). Poll `GET /v1/submissions/{id}` until `awaiting_admin` or `rejected`. Holdout must stay sealed until the digest freezes. A regression must not become champion.
4. **Relearn — promote:** with operator bearer (`deploy/secrets/relearn/admin_tokens`), `POST /v1/admin/promote` only for an eligible paired win.
5. **Relearn Image — submit:** `POST /v1/submissions` with a manifest naming the pinned Cosmos3 base and OpenMDW 1.1. A Flux-family base must be a `400`, not a low score. `GET /v1/prompts` must publish the public split's frozen strings **and** seeds, and must never leak a holdout id. Probe contamination (declare a scored prompt id) and a pillar collapse; both must reject.
6. **Relearn Agent — submit:** `POST /v1/submissions` with a declared training manifest. An empty manifest must fail `contamination_evidence_missing`, not pass. A run whose tool-ablation or observation-shuffle arm is missing, or whose ablation drop is under the floor, must yield lattice `0` — a model that answers without the tools is not an agent.
7. **Bounty — pair + report:** `cortex-bounty pair --hotkey <ss58> --account-id <id>`, then `POST /v1/pair` (terms + signature) and `POST /v1/reports`. Operator bearer `POST /v1/admin/adjudicate` (`valid` / `already_fixed_not_prod` / `invalid_malicious` / `duplicate`). Scoring **reads** CortexLM/backend public JSON (`BOUNTY_BACKEND_PUBLIC_URL`); do not serve `/v1/public/*` from this repo.
8. **Bounty — fail-closed scorer:** the CortexLM/backend public feed is the only scorer. With no readable `BOUNTY_BACKEND_PUBLIC_URL`, `POST /v1/reports` must answer **503** and the emitter must pay **nobody** — it still covers `E` with `NoScore(ChallengeInternal)`, because a paid challenge with no leaves 409s the seal for every challenge. `BOUNTY_FORCE_SIM` is retired — do not reintroduce an offline bounty scorer. See [`docs/BOUNTY.md`](docs/BOUNTY.md).
9. Leaf emission → `POST /v1/weights/raw` → seal → `GET /v1/weights/latest` with **`sealed: true`** (burn fallback alone is not a real seal).

**Never host Sim in staging/prod** for live scoring. `RELEARN_FORCE_SIM=1`, `RELEARN_T2I_FORCE_SIM=1`, and `RELEARN_MM_FORCE_SIM=1` are CI/local opt-in only (`deploy/scripts/assert-compose-matrix.sh` fails if a droplet overlay sets one). Live rent requires a digest pin in the matching `config/relearn*-pin.toml` plus miner BYOK (`LIUM_API_KEY` / `X-Lium-Api-Key`). Never log or commit that key.

**Relearn Image product rules (do not weaken):** the generator seed is `nvidia/Cosmos3-Super-Text2Image` under OpenMDW 1.1; Flux-family bases are refused; Q-Judger (`Qwen/Qwen-Image-Bench`) is the only judge and its card-fixed inference parameters are part of the contract; eval prompts are frozen in the pin so no miner brings its own upsampler to the scored split; the holdout lives in git only as a commitment. **Relearn Agent product rules (do not weaken):** the unit of work is an episode (goal + tool environment), not a prompt; trace replay, tool ablation, and observation shuffle are all mandatory arms and a missing arm fails closed; the capability canary stays off the visible score. **Bounty product rules:** pay is precision x severity, an unpriced `valid` row is not creditable, and the triage-noise ratio stays off the visible score.

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
| Relearn miners | [`docs/external-miner/relearn.md`](docs/external-miner/relearn.md), [`relearn-image.md`](docs/external-miner/relearn-image.md), [`relearn-agent.md`](docs/external-miner/relearn-agent.md) · long guide: [CortexLM/relearn](https://github.com/CortexLM/relearn) |
| Validators | [`docs/external-miner/validators.md`](docs/external-miner/validators.md) |
| Threat / operator checklist | [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md), [`docs/OPERATOR_SECURITY.md`](docs/OPERATOR_SECURITY.md) |

## Do not commit

- `deploy/env/*.env` (materialized secrets)
- `deploy/secrets/**` (except documented `README.md` placeholders)
- `deploy/terraform/*.tfstate*` / `terraform.tfvars` / local `.terraform/`
- Age identities, wallets, `receipt_sk`, `*.pem` / `*.key` / `*.age`
- Treating `docs/evidence/` or `docs/spikes/` as product code or normative spec
