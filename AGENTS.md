# AGENTS.md — Cortex research network

Short contract for agents and operators. Prefer linking over restating runbooks.

**Product:** Cortex ([`CortexLM/cortex`](https://github.com/CortexLM/cortex)) — an autonomous research network on Bittensor. **Two live challenge ids:** `bounty` (2000 bps) and `proof` (8000 bps). Proof-weighted 20%/80% lock regardless of eval digest. Proof eval digest is pinned (`ghcr.io/cortexlm/proof-eval@sha256:78b614a1…`, RLM judge via digest-pinned `InferenceOffer`); live submits still 503 until harvest is wired, a baseline is sealed, and ≥1 topic is open. Empty digest stays fail-closed (do not invent a sha256). Sum is 10000. `relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and `prism` are **removed as products** — no trust-root row, no compose services, no emission, and no leaf may verify. Historical miner stubs stay under [`docs/external-miner/`](docs/external-miner/) so old links do not 404. Frozen specs (`docs/DESIGN_CHALLENGE.md`, `docs/PRISM.md`) remain for xtask gates. Leftover `prism-*` crates are the **Lium harvest stack** used by Proof, not a live Prism challenge. Proof scores operator-published research topics (dynamic `topic_id`, digest-pinned RLM judge, `wta` or `discovery` payout). Naming split (Cortex vs leftover `base` / `BASE_*`): [`docs/NAMING.md`](docs/NAMING.md).

**Vision vs implementation:** start with [`docs/OVERVIEW.md`](docs/OVERVIEW.md) and the [`whitepaper comparison`](docs/WHITEPAPER.md). Do not describe the proposed synthesiser, recursive research judge, durable research corpus, or automatic Proof emission as complete. Current Python judging is partial; Proof uses in-memory submission state. The binary polls a leaf emitter (`PROOF_EMIT_POLL_SECS`, default 120) that signs exact-`E` leaves or covers `E` with `NoScore(ChallengeInternal)` (scored-epoch watermark persisted; gateway refuses a burn from replacing a positive leaf); that is not the proposed automatic Proof emission or end-to-end payment. A pinned image and `can_score` are not proof of scientific reproduction or end-to-end payment.

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

`GET /v1/weights/latest` is **fail-closed**: with no sealed bundle (or decode error) the gateway serves a **burn vector** (uid 0 = 100%, `sealed: false`) rather than 404. A missing gateway wallet is unrelated. Validators MUST NOT submit that unsealed vector and MUST NOT submit a persisted LKG seal while latest is unsealed. A **sealed** Match (`sealed: true`, matching digest) MUST still be submitted when it is burn-uid0 (`burn_outcome=true`, `uids: [0]`, `weights: [1.0]`). A sealed 100% allocation to a **nonzero** registered owner or `validator_permit` UID is still not a submit path.

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
5. **Proof — submit:** `POST /v1/submissions` with a `topic_id`, miner `hotkey_signature` (sr25519 over `base-proof-submit-v1`: hotkey, topic, artefact, FLOPs, claim, canonical manifest, nonce; `X-Lium-Api-Key` is not identity), and a single-use 64-hex `submit_nonce` (the `(hotkey, nonce)` pair is reserved in the store before any row or rent; a replay is **401** `submit_nonce reused`). Missing/unknown/not-open → **400** (no row); missing/invalid signature or nonce → **401** (no row); a custom topic without `artifact_uri` → **400** (no row); an `artifact_digest` that is the sha256 of nothing (zero bytes / an empty tar) → **400** (no row) — the KVM host likewise refuses a content-less or compressed `artifact_tar` before any sister, so an empty-artefact fetch stub can never score. Empty `eval_image_digest`, missing/closed/misconfigured RLM judge `InferenceOffer`, missing judge API key, spoofed topic origin, missing/closed/non-`1x` `EvalExecutorOffer` (Lium path), zero open topics, or an unsealed baseline → **503**. Miners submit claim + code + artifact; they do not bind the judge offer or the executor offer. A topic that names miner BYOK variables in its signed `constraints.params` (`miner_byok` required / `miner_env_allowlist` optional) takes them in the body's **`env`** map (`{"<NAME>": "<value>"}`): the allowlist is the topic's, an undeclared or malformed name is **400** (never a silent drop), a missing required one is **400** before any spend, and because `env` is **not** in the signed payload it is checked *before* the signature so a bad `env` never burns the nonce. The value never lands on the row, `/v1/status`, or a drain report: at intake it goes into a **secure file** vault (`PROOF_MINER_BYOK_DIR`, default `/run/proof/miner-byok`; `0700` dir per frozen digest, `0600` file per variable) and the scoring path reads it back from there, so a deferred topic drained after a restart still has it — a vault that cannot hold it is **503, no row**, and a topic that requires BYOK with nothing held is **503 with the row untouched**, never the owner key. In the guest it is exported to the **paid** job only, plus a `0600` `$PROOF_MINER_ENV_DIR/<NAME>` the adaptor reads, and added to the redaction set. Owner key material never travels this way. Contamination / empty manifest persist **rejected** without rent. Custom / agent topics do not reject on `declared_flops` vs measured FLOPs (`declared_flops` is optional unused, still signed). `GET /v1/proof/topics` must never leak holdout records.
6. **Proof — executor:** `GET /v1/proof/executor` is always 200 (`ready` + `reason`); `POST /v1/admin/proof/executor` (operator bearer) rotates or closes the live `1x` offer and 400s anything the pin refuses. Harvest rents the offer's `lium_template_id` at exactly `1x` (any other `rent_gpu_count` aborts before the rent) under `max_proof_deadline_s`; a run cut at the deadline is **503 + `stdout_tail`**. `PROOF_HARVEST_*` env only hot-swaps under the pin ceilings. Never a live Lium rent in CI.
7. **Proof — topic VMs (custom family):** the RLM runs in one Firecracker microVM per `topic_id` on a host with a working `/dev/kvm` (`proof-vm-orchestrator`, HTTPS + bearer **file**) — production: a dedicated DO droplet (`g-8vcpu-32gb`, nyc1, nested `/dev/kvm`) on the VPC, never colocated on the CP; staging: colocating the agent on the CP droplet with nested `/dev/kvm` is an allowed exception, proven on `cortex-staging` (nested stays fragile — if the boot fails, provision the dedicated droplet); the agent's TLS certificate carries a SAN for every host the CP's URL names (`PROOF_VM_AGENT_TLS_SANS`, checked at boot; `deploy/scripts/proof-vm-agent-tls.sh`); never on Lium, never emulated; every paid run is a **sister** Firecracker guest with **no network**, and the host stamps `sandboxed` / guest-measured `flops_used` on the report. `PROOF_VM_ORCHESTRATOR_URL` unset → `UnwiredVmOrchestrator` (503); URL set but token file missing/empty, `PROOF_RLM_VM_IMAGE_DIGEST` unpinned, agent down, or a `firecracker_required` run without the sister attestation → **503, no row, no host fallback**. `PROOF_VM_RUNNER_CUSTOM_IDS` is the only thing that registers a runner. The custom family is wired from that env alone — live orchestrator selected + ≥1 id → `FamilyMux::custom_only` when no Lium harvest is wired (custom topics score; `nll` / `throughput` → **503**, no row); never stage a placeholder Lium key to open custom topics, and the unwired stub never carries a mux. `/v1/status` keeps the families apart: `live_harvest_wired` is the **Lium harvest only** (never true because a custom mux exists); the custom family is `custom_family_wired` / `registered_custom` / `custom_ready`. Hard `topic_id ↔ VM` bind on both sides (agent 409 `topic_mismatch`). **Artefact identity is the served file, verbatim:** `artifact_digest` = sha256 of the exact bytes at `artifact_uri` (an uncompressed tar with content); the RLM guest verifies what it fetched (`proof_vm_proto::tar::verify_artifact`) and forwards those bytes unchanged — never a re-tar, never a substitute tree; a fetch that fails or does not verify is `RlmToHost::Failed` (503, no row); the host runs the same check before any sister jail and refuses gzip / non-tar / content-less / mis-hashed bytes by name. Do not invent an RLM / sister image digest. **Zero live Firecracker in CI** — every test uses the fake hypervisor. Probe the wire with `GET /v1/admin/proof/vm-orchestrator` (operator bearer, loopback) or `deploy/scripts/proof-vm-wire-check.sh` (`all`, `boot-probe`, `matrix` + `submit-probe --expect 503 --reason …`). Runbook: [`docs/runbooks/proof-vm-orchestrator.md`](docs/runbooks/proof-vm-orchestrator.md) § DigitalOcean staging. **Experiment VMs (in-guest runner topics):** a topic whose signed `constraints.params` select an in-guest runner (`baseline_runner` / synonym `in_guest_benchmark_runner` + `experiment_pack_digest`, naming an operator-baked adaptor id) gets **one dedicated Firecracker VM per paid job** (created for the `Baseline` / `Evaluate` job, destroyed after it; parallel experiments are parallel VMs, never containers sharing one), sized under **configurable caps — Architecte lock: 16 vCPU / 32 GiB RAM (the default a silent topic gets, the ceiling a topic may ask up to, and a hard maximum: a CP or host ceiling set above 16 / 32768 does not boot, a smaller host may only lower it), writable disk ≥ 16 GiB (32 GiB by default, not locked)** (`PROOF_EXPERIMENT_VM_*` on the CP, `PROOF_VM_AGENT_EXPERIMENT_MAX_*` + `PROOF_VM_AGENT_MAX_EXPERIMENT_VMS` on the host; over a ceiling = **503**, never a clamp). The result is returned **only when the orchestrator confirms the VM destroyed** — a failed or unconfirmed destroy after a successful run is `TeardownUnconfirmed` (503, no row, no baseline), never a scored result while the VM may still hold capacity. The host re-hashes the pinned pack from `PROOF_VM_AGENT_EXPERIMENT_PACK_DIR` before any jail and stages it over vsock; the guest agent (`bins/proof-vm-guest-agent`, generic) fetches the artefact streamed under a hard 64 MiB cap, refuses two signed params that collide as one `PROOF_PARAM_*` name, drains adaptor output into a bounded rolling tail while it runs, execs the operator adaptor for that runner id, and never reports a value it did not get from `report.json` — no runner / adaptor / pack / report → `Failed` (503, no row); the host attests the run `experiment_vm` for that VM and job. **Generalist rule:** runner ids, pack digests, model pins, sizes, and every adaptor input are signed-topic params; no benchmark, dataset, task list, harness, adaptor, or pack name is compiled in or committed — `deploy/guest/runners/` holds the adaptor contract and a fail-closed skeleton only (the guest image, its harness tooling via the bake's generic `--extra-pkgs` / `--overlay` / `--chroot-hook`, and its adaptors are operator artefacts baked with `deploy/guest/bake-rootfs.sh` — rootless podman on run-as-owned scratch paths, not Docker-in-VM, no nested KVM). Runbook: [`docs/runbooks/proof-experiment-vms.md`](docs/runbooks/proof-experiment-vms.md).
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
