# AGENTS.md — Cortex control plane

> **Read [`.rules/`](.rules/00-overview.md) first — all of it — before you open
> a pull request or mark one ready.** It is the enforceable contract: hygiene,
> the local pre-prod gates, the PR attestation, automatic versioning, and the
> frozen `base` spellings. CI fails a PR whose body does not attest to it.

Short contract for agents and operators. Prefer linking over restating.

**Product:** Cortex ([`CortexLM/cortex`](https://github.com/CortexLM/cortex)) — Bittensor subnet control plane. **Two live challenges:** `bounty` (7000 bps) and `proof` (3000 bps). Proof's `eval_image_digest` is empty (submits 503), so 7000/3000 keeps most emission payable; retune to 5000/5000 in the same ceremony that pins a non-empty proof-eval digest. Sum is 10000. `relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and `prism` are **off** — no trust-root row, so they have no emission and no leaf may verify. Relearn* code stays behind the `relearn` / `mm` compose profiles. Proof scores operator-published research topics (dynamic `topic_id`, digest-pinned RLM judge); empty `eval_image_digest` → 503 (do not invent a sha256). Naming split (Cortex vs leftover `base` / `BASE_*`): [`.rules/60-naming.md`](.rules/60-naming.md).

PRs require a [Greptile](https://greptile.com) review (`.greptile/`). If the bot is silent, comment `@greptileai review`.

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
| Bounty | this repo [`.rules/contracts/external-miner/bounty.md`](.rules/contracts/external-miner/bounty.md) | Miner pairing + report path; subnet **reads** CortexLM/backend public API (does not serve one) |
| Proof | this repo [`.rules/contracts/external-miner/proof.md`](.rules/contracts/external-miner/proof.md) | Dynamic operator-published topics + digest-pinned RLM judge |

This control-plane repo is `CortexLM/cortex`. Off/archived miner pointers stay under [`.rules/contracts/external-miner/`](.rules/contracts/external-miner/README.md) (`relearn.md`, `relearn-image.md`, `relearn-agent.md`, `relearn-mm.md`) so historical links do not 404; they are not live products. Frozen specs ([`.rules/contracts/DESIGN_CHALLENGE.md`](.rules/contracts/DESIGN_CHALLENGE.md), [`.rules/contracts/PRISM.md`](.rules/contracts/PRISM.md)) stay archived. Do not send miners to Design, Prism, or Relearn docs as live work.

**When a challenge product or public API changes**, agents **must** update:

1. The challenge’s public miner repo (README / examples), and
2. [`.rules/contracts/external-miner/`](.rules/contracts/external-miner/README.md) in this monorepo as needed.

Do not leave miner-facing docs stale after shipping API, quota, round, or scoring changes.

## Challenge verification (mandatory path coverage)

When verifying a challenge (local-e2e, staging, or focused tests), **simulate a submission** end-to-end — do not stop at process healthz. Challenges evaluate on **master only**; the validator has **no challenge exec** (fetch sealed weights only).

1. Happy-path harness / intake POST (or equivalent) through the challenge service on master.
2. Edge / failure probes: bad harness, sanitize reject, quota, wrong routes/auth.
3. **Bounty — pair + report:** `ctx bounty pair --hotkey <ss58> --account-id <id> --accept-terms`, then `POST /v1/pair` (terms + signature) and `POST /v1/reports`. Operator bearer `POST /v1/admin/adjudicate` (`valid` / `already_fixed_not_prod` / `invalid_malicious` / `duplicate`). Scoring **reads** CortexLM/backend public JSON (`BOUNTY_BACKEND_PUBLIC_URL`); do not serve `/v1/public/*` from this repo.
4. **Bounty — fail-closed scorer:** the CortexLM/backend public feed is the only scorer. With no readable `BOUNTY_BACKEND_PUBLIC_URL`, `POST /v1/reports` must answer **503** and the emitter must pay **nobody** — it still covers `E` with `NoScore(ChallengeInternal)`, because a paid challenge with no leaves 409s the seal for every challenge. `BOUNTY_FORCE_SIM` is retired — do not reintroduce an offline bounty scorer. See [`.rules/contracts/BOUNTY.md`](.rules/contracts/BOUNTY.md).
5. **Proof — submit:** `POST /v1/submissions` with a `topic_id`. Missing / unknown / not-open → **400** (no row). Architecture ≠ the baked proxy → **400**. Empty `eval_image_digest`, zero open topics, or an unsealed baseline → **503**. Contamination / empty manifest persist **rejected** without rent. `GET /v1/proof/topics` must never leak holdout records.
6. Leaf emission → `POST /v1/weights/raw` → seal → `GET /v1/weights/latest` with **`sealed: true`** (burn fallback alone is not a real seal).

**Never host Sim in staging/prod** for live scoring. `PROOF_FORCE_SIM=1` is CI/local opt-in only (`deploy/scripts/assert-compose-matrix.sh` fails if a droplet overlay sets one). Live Proof rent requires a digest pin in `config/proof-pin.toml` plus miner BYOK (`LIUM_API_KEY` / `X-Lium-Api-Key`). Never log or commit that key. Do not invent `eval_image_digest`.

**Bounty product rules:** pay is precision x severity, an unpriced `valid` row is not creditable, and the triage-noise ratio stays off the visible score. **Proof product rules (do not weaken):** topics are operator-published signed documents, not a git catalog; a topic may tighten a floor never loosen it; a baseline must be sealed to open; the paid score is the mean of per-topic lattices over currently `open` ids; empty open set / empty eval digest fails closed (`503`); `custom` unknown ids refuse at publish.

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
- Greptile review (template checkbox; `@greptileai review` if silent)

Full local list, including the challenge-submission and pre-prod checks: [`.rules/20-pre-prod-local.md`](.rules/20-pre-prod-local.md).

## Where to read what

| Need | Start here |
|------|------------|
| **Agent + PR contract (read first)** | [`.rules/00-overview.md`](.rules/00-overview.md) |
| System map / process topology | [`README.md`](README.md) § Architecture |
| Cortex vs leftover `base` names | [`.rules/60-naming.md`](.rules/60-naming.md) |
| Local gates + full-subnet test before prod | [`.rules/20-pre-prod-local.md`](.rules/20-pre-prod-local.md) · [`deploy/AGENTS.md`](deploy/AGENTS.md) § Local testnet E2E · `./deploy/scripts/local-e2e.sh --help` |
| Deploy / Compose / DO topology | [`deploy/README.md`](deploy/README.md) + [`deploy/AGENTS.md`](deploy/AGENTS.md) |
| Versioning + release tags | [`.rules/50-versioning.md`](.rules/50-versioning.md) |
| Frozen contracts | [`.rules/contracts/README.md`](.rules/contracts/README.md) |
| Bounty miners | [`.rules/contracts/external-miner/bounty.md`](.rules/contracts/external-miner/bounty.md) · operator spec: [`.rules/contracts/BOUNTY.md`](.rules/contracts/BOUNTY.md) |
| Proof miners | [`.rules/contracts/external-miner/proof.md`](.rules/contracts/external-miner/proof.md) · operator spec: [`.rules/contracts/PROOF.md`](.rules/contracts/PROOF.md) |
| Validators | [`.rules/contracts/external-miner/validators.md`](.rules/contracts/external-miner/validators.md) |
| Threat claim (D19) | [`.rules/contracts/THREAT_MODEL.md`](.rules/contracts/THREAT_MODEL.md) |
| Vulnerability reporting | [`SECURITY.md`](SECURITY.md) |

## Do not commit

- `deploy/env/*.env` (materialized secrets)
- `deploy/secrets/**` (except documented `README.md` placeholders)
- `deploy/terraform/*.tfstate*` / `terraform.tfvars` / local `.terraform/`
- Age identities, wallets, `receipt_sk`, `*.pem` / `*.key` / `*.age`
- A re-created `docs/` tree, or any doc surface outside `README.md`, `AGENTS.md`, `.rules/`, and `deploy/*.md`
