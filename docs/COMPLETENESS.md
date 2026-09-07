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
| Proof v2 round receiver | **partial, opt-in** | `proof-publication` / `gateway-proof` implement full-batch signed `POST /v2/weights/proof/rounds` and exact receipt plus byte readback via `GET /v2/weights/proof/rounds/{round}`; 409 is not success. Sticky `BASE_GATEWAY_PROOF_V2=1` / `BASE_GATEWAY_PROOF_ANCHOR_BLOCK` activation blocks legacy Proof ingress and stale seals. Existing explicit sealing only; local tests do not establish a deployed receiver or live publication. |
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
| Topics | **done** | sr25519 under the `proof` trust-root key (`base-proof-topic-v1`). Admin `POST /v1/admin/proof/topics`. A topic must be sealed to `open`. |
| Holdout | **done** | Per-topic operator file (`PROOF_HOLDOUT_FILE`). Commitment in the topic document, never in the pin. `xtask proof-holdout --topic-id`. |
| Live harvest | **done** | `crates/proof-harvest` over `harvest-pod`; `PROOF_FORCE_SIM` is local-only. |
| Configured allocation | **8000 bps** | Proof-weighted 20%/80% regardless of digest. Payout splits equally across currently `open` topics, then `wta` or `discovery`. Empty digest / missing evaluation prerequisites still fail closed. |
| V1 automatic emission | **lib-only** | `proof-challenge::emit_epoch` signs payout leaves, but `bins/proof-challenge` does not call it or run an emission loop; the HTTP state starts at epoch `0`. Do not infer payments from `can_score`. |
| Autonomous research judge | **partial** | Python `judge.py` requests an acknowledgement; clean static inspection now raises `ContractError` absent agent reproduction and verified FLOP evidence, and forbidden fabric rejects. CLI gates before judge/model calls with no successful metrics. General reproduction and agent-led accounting are not implemented; accounting must fit the experiment with retained reproducible evidence verified by the controller, not a universal formula or model assertion. |
| V1 research persistence | **partial, unproven** | The default v1 store is in memory. The optional SQL journal now passes fresh workspace durability tests (2/2) after fixing embedded-migration tracking. Async writes persist SQL before memory and reject NaN/infinity. Production durability remains unproven: cross-process ID collisions/upserts, separate submission/score transactions, synchronous bypass and cancellation-induced memory lag remain. Public HTTP records are not a durable artifact archive. |
| Atlas experiment persistence / API | **partial, opt-in API** | [`proof-autonomy-pg`](../crates/proof-autonomy-pg/README.md) persists commands, consent/nonces, quotas, revisions, fences, resources and observations. Canonical shared migrations are **0020–0028**; `db::test_pool` matches append-only runtime events and column grants. Restricted DB configuration enables v2 routes in `proof-challenge`, not experiment startup. |
| Experiment worker | **partial, library** | `proof-worker` implements independent model-free cleanup, strict adoption, quote refresh and durable run identity. Same-fence repeat invocation is refused; the original DB runtime deadline is enforced even if an agent ignores stop. Shutdown aborts lease acquisition and drops suspended operation/heartbeat futures before DB bookkeeping. No strict live Lium, credential or quote adapter is wired. |
| V2 local execution / retained science | **partial, local tests** | `proof-executor` runs paired actual CPU scripts in digest-pinned Docker, retaining stdout/stderr/exit/wall and failures. `proof-measure` adds a trusted observer (optionally wired in `proof-experiment`; `DockerObserver`: pinned image, no network by default, read-only rootfs, read-only holdout bind, bounded logs, per-run anti-replay); `collect` still returns `UnobservedMeasurements` whenever FLOPs are unmeasured, and the default `NoObserver` fails closed with zero runs dispatched. Only a test observer image is exercised: the real `proof-eval` image reports no independent FLOPs. Optional `JudgeEgress` demonstrated a synthetic probe with a real completion and four sampled blocked escapes, not universal isolation or science. The eval helper supports explicit `PROOF_JUDGE_PROXY=1` without Authorization only for the exact alias `http://proof-judge:8080/v1` and `chat/completions`; direct mode still requires a key; arbitrary allowed payloads and artifact code mean proxy confidentiality/integrity are not proven. `proof-research` admission tests use synthetic observations; v1 in-memory records are not migrated. The stock W&B SDK stays unusable (v0.28.0 runtime/environment telemetry exceeds the seven-field allowlist); `proof-wandb` uploads the allowlisted record over direct GraphQL but is wired into no binary and has never contacted W&B. |
| Private headless runtime | **partial, scoped tests** | Shared `HeadlessProcess` drives the real `CortexRuntime` with host-only config, attempt sockets and original identity/deadline/budget journal. Seventeen supervision tests pass: inherited pipes, leader exit, five-second TERM→KILL grace, future-drop/reaper behavior and `setsid` escape. Optional `headless.pid_namespace` (`unshare --pid --kill-child`) kills escaped descendants; PID containment only. One authorized Astra/kernel/controller synthetic test passed (7.23 s), not science/cost evidence; it is ignored and requires `CORTEX_TEST_HEADLESS_MODEL_CONFIG`. HTTPS by default; literal loopback HTTP requires `allowLoopbackHttp: true`, private `apiKeyFile`, no Factory fallback. |
| Finalized Atlas rounds / scheduler | **partial, local tests** | `proof-rounds` freezes 360-block inputs/history and signed-byte outboxes; `proof-atlas-worker` schedules/reconciles rounds. Thirteen isolated scheduler tests passed, plus cancellation coverage preventing a blocking RPC completion after shutdown from freezing a new round. Canonical DB migrations passed. The ignored headless-delivery regression below covers the connected local path; deployed publication/admin-seal coordination and payment remain unverified. |
| Atlas service (`bins/proof-atlas`) | **partial, opt-in binary** | `PROOF_ATLAS_CONFIG_FILE` / `--config` selects private operator configuration; raw/hex signer must match the pin. Private-file/signer/restricted-DB checks and three startup/shutdown regressions passed, including SIGTERM while waiting for finality; blocking RPC initialization uses `spawn_blocking`. `--check` does not initialize/validate TS model/provider config, image presence, network/chain/gateway compatibility or deployment. Normal mode starts only the scheduler/publisher, never rental, sealer or chain submit; see the [operator contract](runbooks/proof-autonomy-local.md#separate-atlas-operator-contract). |
| Synthesis / shared-stack adoption | **missing** | The second agent and verified adoption loop described in whitepaper §7 are not implemented. |
| Spec | live | [`PROOF.md`](PROOF.md). |

Verification scope: the executor work reported 13 tests including explicitly run
ignored integrations; the scheduler and supervision counts above are separate.
The explicitly ignored
[`headless_delivery.rs`](../crates/proof-atlas-worker/tests/headless_delivery.rs)
passed real scheduler/Postgres → unmodified `CortexRuntime`/Docker Python →
private controller decision → strict HTTP gateway/exact readback → production
`seal_epoch` → real served router/independent Python vector `[0.4, 0.4, 0.2]`.
Lost-ack recovery replays identical signed bytes without model rerun. Chain,
science and inference are synthetic; this test calls the seal helper directly,
not the operator admin seal HTTP route.

The separate authorized synthetic model test was bounded to 4 calls, 2048 output tokens/call and 120 s.
Its temporary 100 USD/million-token rates were assumed accounting rates, not
verified tariffs or measured charges. The earlier full-workspace checkpoint passed
**1260 tests across 138 binaries** (24 ignored by default), strict Clippy,
formatting and doctests. Separately selected executor/headless integrations and
**123 RLM/runtime tests** passed. Audit and five xtask gates passed, with audit
warnings retained. A later routine checkpoint passed **1304 tests across 151 binaries**
(30 ignored). The final workspace checkpoint passed **1304 tests across 151
executables, 0 failed, 31 ignored**. Separately selected ignored tests passed:
durability **2**, local Docker **14**, synthetic connected `headless_delivery`
**1**, and live-Astra egress **2**. Python **31** and proxy adversarial **3** passed.
Final workspace Clippy, formatting, doctests and five xtask gates passed;
`cargo deny` passed with warnings. The task database was stopped.
The public candidate is not a production repin and predates
the latest proxy-helper corrections. See the [validation boundary](runbooks/proof-autonomy-local.md#local-verification).

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
| Proof v2 integration | Workers, optional observer wiring in `proof-experiment`, local execution and publication components exist, not verified science. One split-GPU rent returned **400**; one whole-host **$0.25/hr** pod remained `PENDING` before termination. Two DELETEs returned **200**; GET returned **404** and the list was empty after **6 s**, with balance unchanged. This does not establish side-effect-free refusals generally, client-id support or its absence, stopped running-pod billing, a whole-hour tariff, or expiry enforcement. Existing `prism-lium` custom templates accept `docker_image=repo@digest`; a null separate digest field does not make digest pinning impossible. Live custom-image enforcement remains untested. No deployed receiver/live publication or end-to-end payment is claimed; default Proof/Bounty allocation remains 8000/2000 bps. |
| Proof scoring vs whitepaper | Current code uses equal topic masses, one primary metric, exact WTA ties, and digest duplicate checks. The paper's multi-metric frontier, method-descriptor novelty, and synthesiser are proposals, not shipped guarantees. |
| DCAP verify holds the attest mutex | A cold Intel PCS fetch (up to 20 s) serialises attestation submissions. |
| DCAP error classification | Matches on `anyhow` message text; re-run `cargo test -p attest-policy --features dcap` after any `dcap-qvl` bump. |
| Bounty severity on the backend feed | Scoring credits a `valid` row only when the backend publishes a `severity`. Until CortexLM/backend emits it, valid rows land as `valid_unpriced`, no miner can be crowned, and the share burns. Fail-closed by design: an unpriced bug cannot be paid for. |
| Bounty scoring backend | The CortexLM/backend public feed is the only scorer. Without a readable `BOUNTY_BACKEND_PUBLIC_URL`, `POST /v1/reports` answers **503** rather than collecting bug-hunting work the host could never pay for, and the emitter pays nobody — it covers `E` with `ChallengeInternal` so the 2000 bps burns to uid 0 without 409ing every other challenge's seal. `BOUNTY_FORCE_SIM` is retired: a local scorer here would pay on adjudications no validator could reproduce. |
| Staging owner check (netuid 541) | `REQUIRE_OWNER=0` until a dedicated 541 owner wallet. Disk mainnet `5ExuWpCM…` ≠ 541 SubnetOwnerHotkey. Do not install the mainnet owner as a fake 541 owner. |
| Spaces backup secrets | First prod promote is fail-closed without `BASE_BACKUP_ENDPOINT` + `SPACES_ACCESS_KEY_ID` / `SPACES_SECRET_ACCESS_KEY` (or AWS_* fallbacks) in GitHub. |
| GitHub `production` environment | Enable required reviewers (and branch protection on `main` as desired) before relying on tag-driven prod; workflow already sets `environment: production`. |
| TLS ACME | Ports 80/443 open on the firewall; gateway TLS termination not shipped yet. |
