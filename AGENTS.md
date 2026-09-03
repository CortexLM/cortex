# AGENTS.md — Cortex control plane

> **Read [`.rules/`](.rules/00-overview.md) first — all of it — before you open
> a pull request or mark one ready.** It is the enforceable contract: hygiene,
> the local pre-prod gates, the PR attestation, automatic versioning, the
> frozen `base` spellings, and
> [mnemonic handling](.rules/70-secrets-mnemonics.md). CI fails a PR whose body
> does not attest to it.

Short contract for agents and operators. Prefer linking over restating.

**Product:** Cortex ([`CortexLM/cortex`](https://github.com/CortexLM/cortex)) — Bittensor subnet control plane for decentralized collaborative AI research via multiple challenges. Naming split (Cortex vs leftover `base` / `BASE_*`): [`.rules/60-naming.md`](.rules/60-naming.md).

## Monorepo map

| Path | Role |
|------|------|
| `bins/` | Runnable processes (validator, gateway, updater, challenges, …) |
| `crates/` | Libraries shared by binaries |
| `xtask/` | Repo gates and maintenance tasks |
| `deploy/` | Compose matrix, Terraform, pins, secrets helpers, remote deploy |
| `.rules/` | Agent + PR contract, and the frozen contracts under [`.rules/contracts/`](.rules/contracts/README.md) |
| `config/` | Shared non-secret configuration (owner-signed trust roots) |

The human front door is [`README.md`](README.md). **There is no `docs/` tree** —
`rules-check` fails if one comes back.

Working branch: **`main`**. Prod ships from annotated tags `v*.*.*` cut on `main`.

## Non-negotiables

- **Digest-only images** in deploy paths — no floating tags in prod pins/compose.
- **Secrets** via age + files under `deploy/env/` / `deploy/secrets/` — never baked into images or cloud-init.
- **Gateway runs on master only** (`--profile master` / `role-master.yml`). Validators point at the master gateway over VPC.
- **`evil-gateway` is test-only** — never enable on prod hosts; assert with `deploy/scripts/assert-evil-gateway-not-default.sh`.
- Platform is **DigitalOcean Droplets + Docker Compose**, not App Platform / DOKS.
- **Do not rename `BASE_*` env vars, deployed paths, or crypto domain tags.** They are measured into miner CVM `app-compose.json` and live on droplets / RTMR3 pin continuity. `CORTEX_*` is an accepted alias in `crates/config` only. See [`.rules/60-naming.md`](.rules/60-naming.md).
- Frozen specs ([`.rules/contracts/BUNDLE_SPEC.md`](.rules/contracts/BUNDLE_SPEC.md), [`.rules/contracts/DESIGN_CHALLENGE.md`](.rules/contracts/DESIGN_CHALLENGE.md)) are pinned by xtask. Do not weaken gates or rewrite incentive / scoring / consensus semantics.
- `unsafe_code = forbid`. No `unwrap` / `expect` in non-test code.
- Every PR bumps the workspace version ([`.rules/50-versioning.md`](.rules/50-versioning.md)) and carries the rules attestation ([`.rules/30-pr.md`](.rules/30-pr.md)).
- **BIP39 mnemonics, `secretPhrase`, and raw hotkey / coldkey seeds never leave a mode-`0600` file.** Not in Chat, argv, GitHub Actions secrets, compose `environment:`, cloud-init, Terraform state, or a `644` agent `audit.jsonl`. `PROD_ROTATE_MNEMONIC` is banned. Keystore already refuses a mnemonic from a plain env var; that is not enough. See [`.rules/70-secrets-mnemonics.md`](.rules/70-secrets-mnemonics.md).

## Wallet / key roles (do not conflate)

| Key | Who | Needed for |
|-----|-----|------------|
| `gateway_sk` | Gateway | Bundle **seal** signatures (`POST /v1/admin/seal`) |
| `gateway_admin_token` | Gateway + seal scripts | Bearer for **`/v1/admin/*`** (seal, backends, attest-grant). **Required** when `BASE_GATEWAY_REQUIRE_OWNER=1` |
| `prism_sk` / `design_sk` | Challenge / smoke | Signed leaves (`POST /v1/weights/raw`); pubs must match trust root |
| Gateway owner wallet + `BASE_GATEWAY_REQUIRE_OWNER` | Gateway | Master-only **identity** check (live/prod). **Not** required to seal or serve `/v1/weights/latest` |
| Validator wallet | Validator | On-chain weight **submit** only — validators *fetch* sealed weights; they do not need a gateway wallet |

Mnemonics for those roles are **path-only** (`BASE_GATEWAY_MNEMONIC_FILE`, `BASE_VALIDATOR_MNEMONIC_FILE`, mode `0600`). Never a GitHub Actions secret, never process argv. Wallet JSON that contains `secretPhrase` is the same secret. See [`.rules/70-secrets-mnemonics.md`](.rules/70-secrets-mnemonics.md).

`GET /v1/weights/latest` is **fail-closed**: with no sealed bundle (or decode error) the gateway serves a **burn vector** (uid 0 = 100%, `sealed: false`) rather than 404. A missing gateway wallet is unrelated.

## Challenge public docs (miner-facing repos)

Each live challenge has a **separate public GitHub repo** for miners. Those repos must contain **only** human miner documentation plus example / test harness code — **never** control-plane, gateway, validator, or orchestrator source. Public repos use a `docs/` layout (hero README + banner under `assets/`).

| Challenge | Public repo | Role |
|-----------|-------------|------|
| Design | [`BaseIntelligence/design-challenge`](https://github.com/BaseIntelligence/design-challenge) | Miner docs + baseline harness |
| Prism | [`BaseIntelligence/prism`](https://github.com/BaseIntelligence/prism) | Miner docs + recipe examples (publish / keep in sync; no control-plane code) |

Those public URLs are historical org names; this control-plane repo is `CortexLM/cortex`. Monorepo mirror for CI and operators: [`.rules/contracts/external-miner/`](.rules/contracts/external-miner/README.md). Frozen contracts stay in this repo ([`.rules/contracts/DESIGN_CHALLENGE.md`](.rules/contracts/DESIGN_CHALLENGE.md), [`.rules/contracts/PRISM.md`](.rules/contracts/PRISM.md), …).

**When a challenge product or public API changes**, agents **must** update:

1. The challenge’s public miner repo (README / examples), and
2. [`.rules/contracts/external-miner/`](.rules/contracts/external-miner/README.md) in this monorepo as needed.

Do not leave miner-facing docs stale after shipping API, quota, round, or scoring changes.

## Challenge verification (mandatory path coverage)

When verifying a challenge (local-e2e, staging, or focused tests), **simulate a submission** end-to-end — do not stop at process healthz. Challenges evaluate on **master only**; the validator has **no challenge exec** (fetch sealed weights only).

1. Happy-path harness / intake POST (or equivalent) through the challenge service on master.
2. Edge / failure probes: bad harness, sanitize reject, quota, wrong routes/auth.
3. **Design — baseline:** submit the reference agent at [`.rules/contracts/external-miner/examples/design-baseline/`](.rules/contracts/external-miner/examples/design-baseline/) (`agent.py` + `pyproject.toml`). After `POST /v1/harness`, poll `GET /v1/runs/{id}` + `/events` + `/logs?since=` until `awaiting_admin` / terminal; assert `GET /v1/runs/{id}/pages` lists `index.html`, `pricing.html`, `components.html` and `GET /v1/view/{run_id}/{page}` returns **200**; probe `GET /v1/stats` and `GET /v1/dashboard`.
4. **Design — cheat:** submit a malicious/copy harness; expect agentic `cheat`/`suspicious` → `Score(0)` (not admin-eligible). Poll events/logs the same way.
5. **Design — admin winners:** with operator bearer (`deploy/secrets/design/annotator_tokens`), `GET /v1/admin/rounds/{id}/candidates` then `POST /v1/admin/rounds/{id}/winners` with 1 or 2 clean harness ids (`SCORE_MAX` or `SCORE_MAX/2`).
6. Leaf emission → `POST /v1/weights/raw` → seal → `GET /v1/weights/latest` with **`sealed: true`** (burn fallback alone is not a real seal).

**Never host Sim in staging/prod** — Docker sandbox only there. `SimSandbox` / `BASE_ALLOW_HOST_SIM=1` is CI/local opt-in only; do **not** treat stub pages (`sim-install-ok` / `sim-run-ok` without executing `agent.py`) as proof. Prefer `DESIGN_FORCE_SIM=false` + OpenRouter when `deploy/secrets/openrouter/api_key` is present.

Local smoke automates the weights seal step via `weights-smoke` inside `./deploy/scripts/local-e2e.sh --smoke` (see [`deploy/AGENTS.md`](deploy/AGENTS.md) and [`.rules/20-pre-prod-local.md`](.rules/20-pre-prod-local.md)).

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
cargo run -p xtask -- rules-check
cargo run -p xtask -- version check
```

Version bump for the current branch (Conventional Commits decide the level):

```bash
cargo run -p xtask -- version bump
cargo run -p xtask -- version verify-bump --base origin/main
```

Commit subjects: `type(scope): summary` (lowercase, ≤72 chars). Hooks: `./scripts/install-githooks.sh`.

## Required gates (before merge)

Match CI (`.github/workflows/ci.yml` + `.github/workflows/pr-gate.yml`):

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- tests + `cargo deny`
- `cargo run -p xtask -- loc-cap`
- `cargo run -p xtask -- consensus-lint`
- `cargo run -p xtask -- spec-check`
- `cargo run -p xtask -- design-check`
- `cargo run -p xtask -- external-docs-check`
- `cargo run -p xtask -- rules-check`
- `cargo run -p xtask -- version check`
- PR gate: rules attestation in the PR body + `version verify-bump`

Full local list, including the challenge-submission and pre-prod checks: [`.rules/20-pre-prod-local.md`](.rules/20-pre-prod-local.md).

## Where to read what

| Need | Start here |
|------|------------|
| **Agent + PR contract (read first)** | [`.rules/00-overview.md`](.rules/00-overview.md) |
| System map / process topology | [`README.md`](README.md) § Architecture |
| Cortex vs leftover `base` names | [`.rules/60-naming.md`](.rules/60-naming.md) |
| BIP39 / `secretPhrase` / audit.jsonl | [`.rules/70-secrets-mnemonics.md`](.rules/70-secrets-mnemonics.md) |
| Local gates + full-subnet test before prod | [`.rules/20-pre-prod-local.md`](.rules/20-pre-prod-local.md) · [`deploy/AGENTS.md`](deploy/AGENTS.md) § Local testnet E2E · `./deploy/scripts/local-e2e.sh --help` |
| Deploy / Compose / DO topology | [`deploy/README.md`](deploy/README.md) + [`deploy/AGENTS.md`](deploy/AGENTS.md) |
| Versioning + release tags | [`.rules/50-versioning.md`](.rules/50-versioning.md) |
| Frozen contracts | [`.rules/contracts/README.md`](.rules/contracts/README.md) |
| Miner HTTP submit | [`.rules/contracts/external-miner/`](.rules/contracts/external-miner/README.md) · public: [design-challenge](https://github.com/BaseIntelligence/design-challenge), [prism](https://github.com/BaseIntelligence/prism) |
| Threat claim (D19) | [`.rules/contracts/THREAT_MODEL.md`](.rules/contracts/THREAT_MODEL.md) |
| Vulnerability reporting | [`SECURITY.md`](SECURITY.md) |

## Do not commit

- `deploy/env/*.env` (materialized secrets)
- `deploy/secrets/**` (except documented `README.md` placeholders)
- `deploy/terraform/*.tfstate*` / `terraform.tfvars` / local `.terraform/`
- Age identities, wallets, `receipt_sk`, `*.pem` / `*.key` / `*.age`
- BIP39 mnemonics, wallet JSON containing `secretPhrase`, or an agent `audit.jsonl` that captured either ([`.rules/70-secrets-mnemonics.md`](.rules/70-secrets-mnemonics.md))
- A re-created `docs/` tree, or any doc surface outside `README.md`, `AGENTS.md`, `.rules/`, and `deploy/*.md`
