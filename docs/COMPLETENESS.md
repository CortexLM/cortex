# Cortex completeness matrix

Per-component implementation status. Updated as phases land. This is not a fresh
production-health check; infrastructure entries record the documented operator
baseline, not observations made by reading this repository.

For new readers: [overview](OVERVIEW.md). For the difference between the
whitepaper's research vision and current code: [implementation comparison](WHITEPAPER.md).

## Legend

| Tag | Meaning |
|-----|---------|
| **done** | Implemented, tested, wired into a running binary. |
| **sim** | Code exists and passes tests, but the running binary uses a simulated backend, not live data. |
| **lib-only** | Library crate is complete; no binary drives it in production. |
| **partial** | Some behavior exists, but the advertised end-to-end mechanism is incomplete. |
| **test-only** | Compiled and exercised by tests; deliberately unreachable from any shipped binary. |
| **missing** | No code, no compose service, no CI image. |

## Chain layer

| Component | Status | Notes |
|-----------|--------|-------|
| `ChainClient` trait | done | 14 methods, full trait surface. |
| `FakeChain` | test-only | Deterministic in-memory. No longer reachable from any binary; used by unit and adversarial tests. |
| `chain-live` crate (`LiveChainClient`) | **done** | Production chain client: full JSON-RPC reads (`Identity` hasher, `Keys` double-map enumeration, `ValueQuery` defaults) + sr25519 signed `set_weights` / `commit_timelocked_mechanism_weights`. The **only** backend in `bins/validator` and `bins/gateway`; both fail fast if the chain is unreachable. Four `#[ignore]` tests read live testnet 541. Obsolete alternative stubs were removed; see [cleanup scope](CLEANUP.md). |
| `BASE_CHAIN_ENDPOINT` / `BASE_CHAIN_ENDPOINTS` | done | Read by `config::Config`; consumed by `chain-live::LiveChainClient::connect`. The plural var is an ordered comma-separated failover list (wins over the singular); a rate-limited (HTTP 429 / `-32005`) or unreachable endpoint cools 60s and the call tries the next in order. |
| CRV4 tlock encryption | **done** | Drand Quicknet TLE via git-pinned `tle` (same rev as subtensor / `bittensor_drand`); `LiveChainClient::submit_timelocked_weights` encrypts SCALE `WeightsTlockPayload` before signing. Fail-closed on encrypt error — never downgrades to `set_weights` while CR is enabled. |

## Validator

| Component | Status | Notes |
|-----------|--------|-------|
| Health endpoints (`/healthz`, `/readyz`, `/metrics`) | done | |
| Attestation (`/v1/attest/*`) | done | Real Intel DCAP via `dcap-qvl` when built `--features dcap` (the container default). Verified against live Intel PCS; a tampered quote yields `CryptoInvalid`. Mock verifiers remain for tests only. |
| Bundle fetch + `compare_bundle` | done | Continuous coordination loop. |
| Match → `submit_intent` | done | `spawn_coordination_loop` submits on Match with per-epoch in-memory dedupe; CR enabled → timelocked path (never downgrades to `set_weights`). Requires validator signing key. Last verified seal may be persisted (`BASE_VALIDATOR_LKG_PATH`, default `/var/lib/base/last-sealed.bundle`) but unsealed `/v1/weights/latest` is not a submit path (no LKG resubmit). Pure burn to the registered owner / a `validator_permit` UID is not submitted. |
| `set_weights` / `submit_timelocked_weights` | done | Live `set_weights` (CR off) + `commit_timelocked_mechanism_weights` with Drand TLE ciphertext (CR on / CRV4). Signing key via `keystore`. |
| Chain backend | done | Live only. `FakeChain` was removed from `bins/validator`; there is no switch left to misconfigure. |

## Gateway

| Component | Status | Notes |
|-----------|--------|-------|
| Master check (`SubnetOwnerHotkey`) | done | Read from the live chain. Prod: `BASE_GATEWAY_REQUIRE_OWNER=1` fail-closed (wallet matches SubnetOwnerHotkey; `gateway_admin_token` required). Staging: `REQUIRE_OWNER=0` advisory until a dedicated netuid-541 owner wallet (disk mainnet `5ExuWpCM…` ≠ 541 SubnetOwnerHotkey). Do not install the mainnet owner as a fake 541 owner. Local smoke defaults to advisory. |
| Registry + proxy | done | |
| Bundle seal (`POST /v1/weights/raw` → `GET /v1/weights/latest`) | done | Unsealed: fail-closed burn (`sealed: false`, uid 0 = 100%) instead of 404. |
| Chain backend | done | Live only. `fake_owner` was removed from `bins/gateway`. |

## Retired products (removed)

`relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and `prism`
are **gone from the tree** as products: no crates, bins, compose services, or
pins. No trust-root row, so they have no emission and no leaf may verify.
Historical miner stubs stay under [`external-miner/`](external-miner/). Frozen
specs (`DESIGN_CHALLENGE.md`, `PRISM.md`) remain for `xtask` gates. Leftover
`prism-*` crates are the **Lium harvest stack** used by Proof.

## bounty-challenge

| Component | Status | Notes |
|-----------|--------|-------|
| Crates (`crates/bounty-*`) | **done** | task (pairing), score (precision × severity, triage-noise canary off the lattice), store, http (fail-closed ingest + quotas), challenge (backend public **consumer** + fail-closed leaf emitter; the two public routes are re-read until they agree, and `/leaderboard` `valid_count` must match the `valid` reports, so a mid-publish pair or a stable A+B mix is never signed as one snapshot). |
| Binary (`bins/bounty-challenge`) | **done** | Internal HTTP on `:8096` plus the emitter (backend feed → exact-`E` leaves → gateway `POST /v1/weights/raw`, `BOUNTY_EMIT_POLL_SECS`). Does **not** serve `/v1/public/*`. No feed (or an unreadable one) pays nobody: `E` is covered with `NoScore(ChallengeInternal)` so D24 holds and the share burns to uid 0. A scored epoch is never downgraded to a burn mid-epoch. |
| Miner CLI (`bins/ctx`) | **done** | `ctx bounty pair|report|show|status`. `bins/cortex-bounty` deprecates to `ctx bounty pair`. |
| Compose / images | **done** | Default compose + `images.yml` target `bounty-challenge`. |
| Emission | **2000 bps** | Payable share (20%). Sum `10000`. |
| Spec | live | [`BOUNTY.md`](BOUNTY.md). |

## proof-challenge

| Component | Status | Notes |
|-----------|--------|-------|
| Challenge id | **done** | `proof` on the wire. Topics are operator-published signed documents; git carries no catalog. |
| Crates (`crates/proof-*`) | **partial** | Signed topics, holdout commitments, global pin, per-topic pass + WTA/discovery payout, in-memory store, readiness checks, harvest, and HTTP exist. They do not constitute the full autonomous research loop. |
| Binary (`bins/proof-challenge`) | **done** | HTTP API on `:8100`. |
| Miner CLI (`bins/ctx`) | **done** | `ctx proof submit|show|status|topics`. Unpinned digest / unwired harvest / no open topic → 503. |
| Compose / images | **done** | Default compose + `images.yml` target `proof-challenge`. |
| Eval pin | **done** | `config/proof-pin.toml` — `eval_image` `ghcr.io/cortexlm/proof-eval`, digest `sha256:78b614a1…` (publish-proof-eval-image run 33892650063, commit `51f937c7`). No HF bake; `proxy_model` stays empty. Live submits still **503** until harvest is wired, a baseline is sealed, and ≥1 topic is open. Do not re-pin a guessed sha256. |
| Inference offer | **v0** | Digest-pinned RLM **judge** backend (`proof-eval` / harvest call it). Pin `[inference]` defaults plus schema v1 / ceilings / modes / commitment. `config_commitment` hashes config knobs **and** `provider.base_url`; a topic that spoofs origin is **503** before lattice. Topic `require_judge_offer_commitment` is optional and not a miner bind. Live `InferenceOffer` is operator state. Auth is `PROOF_INFERENCE_API_KEY_FILE` staged as harvest `teacher.env` (never git, never `/v1/status`). Missing/closed/judge down / missing key → `can_score=false` / 503. No baked Qwen; architecture ≠ HF stays retired. |
| Eval executor offer | **v0** | `crates/proof-executor`: live `1x` `EvalExecutorOffer` (Lium template, `machine_shape`, `max_proof_deadline_s`, digest, `config_commitment`, status) — a sibling of the judge offer, not the same document. Pin ceilings `eval_executor_schema_version` / `gpu_class = "1x"` / `max_proof_deadline_s_ceiling = 7200` / optional `allowed_lium_template_prefixes` / `eval_executor_commitment_alg`. Public on `GET /v1/status` + `GET /v1/proof/executor`; rotated via `POST /v1/admin/proof/executor` (in-memory until restart; boot from `PROOF_EVAL_EXECUTOR_OFFER_FILE`). Topic tighten-only `eval_executor.{require_offer_commitment, max_proof_deadline_s}`, no per-topic `machine_id`. Lium path: missing/closed/shape ≠ `1x` → `can_score=false` / 503; harvest rents the offer's digest-scoped template (raw Lium UUIDs refused under any allowlist; the resolver binds the template to `eval_image@digest`) at exactly `1x` (`rent_gpu_count ≠ 1` aborts pre-rent) and holds the run to the deadline (the deadline is the pod `timeout`, never clamped by the host fallback; harvest wait = deadline + grace; wrapper-cut run → 503 + `stdout_tail`, external SIGKILL named separately). `PROOF_HARVEST_TEMPLATE_ID` / `_GPU_COUNT` / `_DEADLINE_SECS` hot-swap under the pin ceilings; refused when the topic pins the offer commitment; the run request and row stamp the commitment of what actually ran. Sim does not consult it. No live Lium rent in CI. |
| Topics | **done** | sr25519 under the `proof` trust-root key (`base-proof-topic-v1`). Admin `POST /v1/admin/proof/topics`. A topic must be sealed to `open`. |
| Holdout | **done** | Per-topic operator file (`PROOF_HOLDOUT_FILE`). Commitment in the topic document, never in the pin. `xtask proof-holdout --topic-id`. |
| Live harvest | **partial** | `crates/proof-harvest` over `harvest-pod` stages `request.json`, `teacher.env`, `PROOF_PROXY_MODEL_DIR`, and `PROOF_HOLDOUT_STORE`. `PROOF_FORCE_SIM` is local-only. Live rent still needs a republished proof-eval digest (current pin still has the invalid HF default) plus operator-staged proxy dir + holdout shards. |
| Configured allocation | **8000 bps** | Proof-weighted 20%/80% regardless of digest. Payout splits equally across currently `open` topics, then `wta` or `discovery`. Empty digest / missing evaluation prerequisites still fail closed. |
| Automatic emission | **lib-only** | `proof-challenge::emit_epoch` signs payout leaves, but `bins/proof-challenge` does not call it or run an emission loop; the HTTP state starts at epoch `0`. Do not infer payments from `can_score`. |
| RLM engine (`crates/proof-rlm*`, `proof-canon`) | **generic / fail-closed** | Topic schema carries generic bindings (`constraints.{firecracker_required, model_pin, task_slice, params}`, `checklist` rule vector, `eval_executor.{require_offer_commitment, max_proof_deadline_s}`); `custom_id` is topic data (open needs a registered runner). Core: versioned rule sets + checklist + spend token (no paid inference behind a red checklist), lifecycle `draft → owner_presend → awaiting_owner_keys → provisioning → baselining → open ⇄ evaluating → promoting → closed` with owner hooks, `CustomRunner` + `RunnerRegistry` (**empty by default**), `TopicVmOrchestrator` boundary with `UnwiredVmOrchestrator` and the generic `VmBackedRunner`, promotion rule. Store: migration `0020_proof_rlm.sql` + `PgRlmStore` / `MemoryRlmStore` (topic versions, rule versions, checklists, transitions, baseline, artefact metadata, promotion continuum). Host: `RlmScorer` routed through `FamilyMux` (per-topic lease from score to persist, promotion decided against the store's best with a compare-and-swap on the pointer; runner-measured `flops_used` in the verdict, missing → 503, over budget → reject; `artifact_uri` reaches the runner), artefact zips + `best.json` + `events.jsonl`, `TopicSetup` driver (`mark_sealed` opens only a signed, valid, open document sealing the RLM's measured value). **No registered runner, no challenge content by default:** every custom topic answers **503** until the operator lists ids in `PROOF_VM_RUNNER_CUSTOM_IDS`. The registry is wired from the topic-VM orchestrator env alone: live orchestrator + ≥1 id with no Lium harvest → `FamilyMux::custom_only` (custom scores, `nll` / `throughput` **503**, no row); no placeholder Lium key is needed to open custom topics. `/v1/status` reports the families apart — `live_harvest_wired` is Lium only; `custom_family_wired` / `registered_custom` / `custom_ready` are the custom family. |
| Topic-VM orchestrator (`crates/proof-vm-proto`, `proof-vm-fc`, `proof-vm-agent`, `proof-fc-host`, `bins/proof-vm-orchestrator`) | **implemented / operator-gated** | `FirecrackerOrchestrator` is the live `TopicVmOrchestrator`: HTTPS client (bearer file, never logged; https only off loopback) of the `proof-vm-orchestrator` agent on a **dedicated KVM host**. Preferred by `bins/proof-challenge` when `PROOF_VM_ORCHESTRATOR_URL` + `PROOF_VM_ORCHESTRATOR_TOKEN_FILE` are set; `PROOF_RLM_VM_IMAGE_DIGEST` pins the RLM rootfs (4 vCPU / 8192 MiB; unpinned → 503). Agent: one jailed Firecracker RLM VM per `topic_id` (digest re-hashed before boot, hard topic bind on envelope + job, per-VM job lock), vsock jobs, owner key material staged from the host's own dir, per-VM nftables egress allowlist, **sister** miner guest with no network for every paid run, host-stamped `sandboxed` / guest-measured `flops_used` (the attestation names the job's topic / submission / artefact and both agent and CP run `bind_evidence` before accepting it), jail guard so a failed boot or a cancelled sister leaves nothing on the host, dead-VM reaping per retain policy (`crashed`, topic may recreate), destroy-or-retain teardown. `deploy/systemd/proof-vm-orchestrator.service` + [`runbooks/proof-vm-orchestrator.md`](runbooks/proof-vm-orchestrator.md). Operator probe `GET /v1/admin/proof/vm-orchestrator` (the CP's own client: `ready` / `reason`, agent health, `custom_family_wired` / `registered_custom`, `live_harvest_wired` Lium-only) and the staging harness `deploy/scripts/proof-vm-wire-check.sh` (env / agent / cp / boot-probe / submit-probe / matrix; tested against the fake agent) with placeholder overlays `deploy/env/*.staging*.example`. **Staging:** the agent booted Firecracker colocated on `cortex-staging` (nested DO `/dev/kvm` — an allowed exception, proven) and the runbook's § 4 fail-closed matrix came back green; image digests are operator state on that host (computed from images built outside this repo; nothing in git invents one), the § 5 happy path and § 6 sign-off are still to be recorded, and nested KVM stays fragile (boot fails → provision metal). **Production:** dedicated DO metal preferred, never colocated on the CP; not deployed. CI runs the fake hypervisor only. mTLS is a follow-up. |
| Autonomous research judge | **partial** | Python `judge.py` requests an acknowledgement, while `agent.py` uses static text checks. General recipe reproduction and the paper's recursive investigation are not implemented. |
| Research persistence | **missing** | The service uses `MemoryStore`; submissions and scores are lost on restart. Public HTTP records are not a durable artifact archive. |
| Synthesis / shared-stack adoption | **missing** | The second agent and verified adoption loop described in whitepaper §7 are not implemented. |
| Spec | live | [`PROOF.md`](PROOF.md). |

## Infrastructure

Agent/operator contracts: root [`AGENTS.md`](../AGENTS.md), [`deploy/AGENTS.md`](../deploy/AGENTS.md), [`docs/AGENTS.md`](AGENTS.md). Deploy detail remains in [`deploy/README.md`](../deploy/README.md).

| Component | Status | Notes |
|-----------|--------|-------|
| Terraform droplets | done | 4 of 4: staging master, staging validator, prod master, prod validator. |
| Staging master | done | Migrated to `/opt/base` CI-managed; old `/opt/gbase` stack torn down. |
| Staging validator | done | Redeployed from same commit; `bundle gateway signature invalid` resolved. |
| Prod master | done | Droplet up. Mainnet owner wallet on disk matches SubnetOwnerHotkey; `env-prod.yml` sets `BASE_GATEWAY_REQUIRE_OWNER=1` (`gateway_admin_token` required). Recreate the gateway on droplets after that compose change. |
| `deploy-staging.yml` | done | Auto on CI green; `--build-from source` for fast iteration; fail-closed health gate. |
| `deploy-prod.yml` | done | Tag-based (`v*.*.*`); preflight (CI green + `origin/main` staging pins `commit_sha`); fail-closed Spaces backup; `promote.sh --confirm-prod`; `--build-from registry` (GHCR digest pull, no Rust compile on droplet). |
| `images.yml` pin ladder | done | After GHCR push: write `deploy/digests/<sha>.json`, `promote.sh --env staging` for pin services, commit/push so prod preflight can match. |
| GitHub secrets | done | Host/SSH/gateway secrets set. Prod promote also needs Spaces: `BASE_BACKUP_ENDPOINT`, `SPACES_ACCESS_KEY_ID` / `SPACES_SECRET_ACCESS_KEY` (fail-closed if absent). |

## Keys and identity

| Component | Status | Notes |
|-----------|--------|-------|
| `keystore` crate | done | BIP39 (pinned 2048-word English list) → Substrate `PBKDF2-HMAC-SHA512(entropy, "mnemonic"+password, 2048)` → sr25519. Cross-checked against `substrateinterface` and against all six local wallets. |
| Bittensor wallet reader | done | Reads `~/.bittensor/wallets/<name>/hotkeys/<hotkey>`; re-derives the key and rejects the file if the derived public key disagrees with the stored one. |
| Hotkey resolution | done | `keystore::resolve_*_from_env`: wallet → mnemonic file → secret-key file → public-only hex/SS58. A mnemonic is never read from a plain env var. |
| Gateway / validator hotkeys | done | Both resolve from `BASE_*_WALLET`. Staging uses `base-owner` (gateway) and `base-validator`. |
| Wallets on hosts | done | Only the hotkey file is shipped, mode 0400, owned by uid 65532, under `deploy/secrets/wallets/`. |

## Challenge backends

| Component | Status | Notes |
|-----------|--------|-------|
| bounty HTTP / adjudicate | **done** | Internal ingest: `POST /v1/pair` (sr25519) + `POST /v1/reports`; operator bearer on `GET /v1/reports` and `POST /v1/admin/adjudicate`. Scoring **fetches** CortexLM/backend `GET /v1/bounty/public/leaderboard` + `/reports` and emits signed leaves from those rows. Unset / unreachable / unparseable `BOUNTY_BACKEND_PUBLIC_URL` → `can_score: false`, reports **503**, and an all-`NoScore(ChallengeInternal)` leaf set that pays nobody while keeping D24. `BOUNTY_FORCE_SIM` is retired and ignored. |
| proof HTTP / topics | **done** | Operator-published signed topics; `POST /v1/submissions` with `topic_id`. Empty digest / unwired harvest / unsealed baseline / empty open set / missing RLM judge offer / missing judge API key / spoofed topic origin → **503**. Architecture ≠ HF is retired. Contamination / empty manifest persist **rejected** without rent. |
| Proof Lium harvest | **done** (fail-closed) | `crates/proof-harvest` over `harvest-pod` + leftover `prism-lium*` client. Live rent refuses without a `sha256:` eval digest; `PROOF_FORCE_SIM` is CI/local only; miner BYOK never logged. |
| Retired challenge products | **removed** | `relearn*`, `design`, `prism` crates/bins/compose gone. Frozen specs remain. SQL migrations for historical tables stay applied. |
| Phala / agent-v1 miner path | removed | External miners use HTTP submit only ([`external-miner/`](external-miner/)). |

## Known gaps

| Gap | Impact |
|-----|--------|
| Proof research-to-payment path | Partial Python judging, in-memory results, epoch `0` HTTP state, and no automatic emitter prevent treating the current service as the full whitepaper mechanism. See [the source comparison](WHITEPAPER.md#proposal-versus-current-code). |
| Proof scoring vs whitepaper | Current code uses equal topic masses, one primary metric, exact WTA ties, and digest duplicate checks. The paper's multi-metric frontier, method-descriptor novelty, and synthesiser are proposals, not shipped guarantees. |
| DCAP verify holds the attest mutex | A cold Intel PCS fetch (up to 20 s) serialises attestation submissions. |
| DCAP error classification | Matches on `anyhow` message text; re-run `cargo test -p attest-policy --features dcap` after any `dcap-qvl` bump. |
| Bounty severity on the backend feed | Scoring credits a `valid` row only when the backend publishes a `severity`. Until CortexLM/backend emits it, valid rows land as `valid_unpriced`, no miner can be crowned, and the share burns. Fail-closed by design: an unpriced bug cannot be paid for. |
| Bounty scoring backend | The CortexLM/backend public feed is the only scorer. Without a readable `BOUNTY_BACKEND_PUBLIC_URL`, `POST /v1/reports` answers **503** rather than collecting bug-hunting work the host could never pay for, and the emitter pays nobody — it covers `E` with `ChallengeInternal` so the 2000 bps burns to uid 0 without 409ing every other challenge's seal. `BOUNTY_FORCE_SIM` is retired: a local scorer here would pay on adjudications no validator could reproduce. |
| Staging owner check (netuid 541) | `REQUIRE_OWNER=0` until a dedicated 541 owner wallet. Disk mainnet `5ExuWpCM…` ≠ 541 SubnetOwnerHotkey. Do not install the mainnet owner as a fake 541 owner. |
| Spaces backup secrets | First prod promote is fail-closed without `BASE_BACKUP_ENDPOINT` + `SPACES_ACCESS_KEY_ID` / `SPACES_SECRET_ACCESS_KEY` (or AWS_* fallbacks) in GitHub. |
| GitHub `production` environment | Enable required reviewers (and branch protection on `main` as desired) before relying on tag-driven prod; workflow already sets `environment: production`. |
| TLS ACME | Ports 80/443 open on the firewall; gateway TLS termination not shipped yet. |
