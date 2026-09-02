# Cortex completeness matrix

Honest per-component status as of `main` HEAD. Updated as phases land.

## Legend

| Tag | Meaning |
|-----|---------|
| **done** | Implemented, tested, wired into a running binary. |
| **sim** | Code exists and passes tests, but the running binary uses a simulated backend, not live data. |
| **lib-only** | Library crate is complete; no binary drives it in production. |
| **stub** | Trait method returns `NotImplemented`; placeholder for future wiring. |
| **test-only** | Compiled and exercised by tests; deliberately unreachable from any shipped binary. |
| **missing** | No code, no compose service, no CI image. |

## Chain layer

| Component | Status | Notes |
|-----------|--------|-------|
| `ChainClient` trait | done | 14 methods, full trait surface. |
| `FakeChain` | test-only | Deterministic in-memory. No longer reachable from any binary; used by unit and adversarial tests. |
| `NotImplementedChain` | stub | Every method returns `Err(NotImplemented)`. |
| `LiveRpcChain` (feature `live` on older chain helpers) | stub | Legacy stub surface: `current_block` + `block_hash` only; metagraph / weight submit paths `NotImplemented`. **Not** the production backend. |
| `chain-live` crate (`LiveChainClient`) | **done** | Production chain client: full JSON-RPC reads (`Identity` hasher, `Keys` double-map enumeration, `ValueQuery` defaults) + sr25519 signed `set_weights` / `commit_timelocked_mechanism_weights`. The **only** backend in `bins/validator` and `bins/gateway`; both fail fast if the chain is unreachable. Four `#[ignore]` tests read live testnet 541. Do not confuse with stub `LiveRpcChain` above. |
| `BASE_CHAIN_ENDPOINT` / `BASE_CHAIN_ENDPOINTS` | done | Read by `config::Config`; consumed by `chain-live::LiveChainClient::connect`. The plural var is an ordered comma-separated failover list (wins over the singular); a rate-limited (HTTP 429 / `-32005`) or unreachable endpoint cools 60s and the call tries the next in order. |
| CRV4 tlock encryption | **done** | Drand Quicknet TLE via git-pinned `tle` (same rev as subtensor / `bittensor_drand`); `LiveChainClient::submit_timelocked_weights` encrypts SCALE `WeightsTlockPayload` before signing. Fail-closed on encrypt error — never downgrades to `set_weights` while CR is enabled. |

## Validator

| Component | Status | Notes |
|-----------|--------|-------|
| Health endpoints (`/healthz`, `/readyz`, `/metrics`) | done | |
| Attestation (`/v1/attest/*`) | done | Real Intel DCAP via `dcap-qvl` when built `--features dcap` (the container default). Verified against live Intel PCS; a tampered quote yields `CryptoInvalid`. Mock verifiers remain for tests only. |
| Bundle fetch + `compare_bundle` | done | Continuous coordination loop. |
| Match → `submit_intent` | done | `spawn_coordination_loop` submits on Match with per-epoch in-memory dedupe; CR enabled → timelocked path (never downgrades to `set_weights`). Requires validator signing key. Last verified seal is persisted (`BASE_VALIDATOR_LKG_PATH`, default `/var/lib/base/last-sealed.bundle`) so a gateway burn fallback after master restart still Matches and submits. |
| `set_weights` / `submit_timelocked_weights` | done | Live `set_weights` (CR off) + `commit_timelocked_mechanism_weights` with Drand TLE ciphertext (CR on / CRV4). Signing key via `keystore`. |
| Chain backend | done | Live only. `FakeChain` was removed from `bins/validator`; there is no switch left to misconfigure. |

## Gateway

| Component | Status | Notes |
|-----------|--------|-------|
| Master check (`SubnetOwnerHotkey`) | done | Read from the live chain. Advisory by default; `BASE_GATEWAY_REQUIRE_OWNER=1` makes it fail-closed (set in staging). |
| Registry + proxy | done | |
| Bundle seal (`POST /v1/weights/raw` → `GET /v1/weights/latest`) | done | Unsealed: fail-closed burn (`sealed: false`, uid 0 = 100%) instead of 404. |
| Chain backend | done | Live only. `fake_owner` was removed from `bins/gateway`. |

## agent-challenge / hypertraining-challenge / design / prism (products)

Removed as **live products**. Shared rails (`prism-lium*`, `prism-competition` paired tests, receipts, emit/carry) stay as libraries. Frozen specs (`DESIGN_CHALLENGE.md`, `PRISM.md`) remain for `xtask` gates.

## relearn-challenge

| Component | Status | Notes |
|-----------|--------|-------|
| Crates (`crates/relearn-*`) | **done** | task (holdout commitment + contamination fingerprints), score (public–holdout gap, contamination evidence, vision shuffle, off-path general-bench canary), store, eval, http, challenge. |
| Binary (`bins/relearn-challenge`) | **done** | HTTP API on `:8095`. |
| Compose / images | **done** | Default compose + `images.yml` target `relearn-challenge`. |
| Eval pin | **blocked** | `config/relearn-pin.toml` — `eval_image_digest` empty on purpose, so submissions 503 `eval image digest not pinned`. Needs a `CortexLM/relearn` image whose CUDA scoring layer ships **vLLM + torchvision**. Every digest tried so far failed on a rented pod and is named in the pin: `cbc4bbb8…` (exit 1, transformers fallback then `Qwen3VLVideoProcessor` wanted torchvision), `201cc5d2…`, `303c6357…`, `00839671…`, `86240d76…`. Do not re-pin or re-harvest any of them, and do not guess a digest. |
| Holdout | **done** | Commitment in git, records operator-side (`RELEARN_HOLDOUT_FILE`) and verified at boot. Mismatch → submissions 503. |
| Teacher | **v0** | Weights `incoai/GLM-5.3-NVFP4` served from `RELEARN_TEACHER_LOCAL_DIR` (never pass the Hugging Face repo id to vLLM). HTTP wire `glm-5.3`. Missing `RELEARN_TEACHER_API_URL` → `can_score: false` and 503 before rent. Judge-only. |
| Emission | **4000 bps** | Default share (sum across all four challenges is `10000`). |
| Spec | live | [`RELEARN.md`](RELEARN.md). |

## relearn-t2i-challenge (`relearn-image`)

| Component | Status | Notes |
|-----------|--------|-------|
| Challenge id | **done** | `relearn-image` on the wire; crates, service, env prefix, and pin filename keep the pre-launch `t2i` spelling because the domain tags are hashed into the committed holdout commitment ([`NAMING.md`](NAMING.md)). |
| Crates (`crates/relearn-t2i-*`) | **done** | task (Cosmos3 pin, frozen prompts, seed lattice), judge (Q-Judger wire format), score (pillar / replay / faithfulness / contamination-evidence / capability-canary / public-gap gates), store, eval, harvest, http, challenge. |
| Binary (`bins/relearn-t2i-challenge`) | **done** | HTTP API on `:8097`. |
| Compose / images | **done** | Default compose + `images.yml` target `relearn-t2i-challenge`. |
| Generator pin | **done** | `nvidia/Cosmos3-Super-Text2Image` (OpenMDW 1.1, card verified). Flux-family bases refused at pin load, submit, and eval. |
| Judge pin | **done** | Q-Judger (`Qwen/Qwen-Image-Bench`), card-fixed inference. No alternate judge is accepted. |
| Eval pin | **done** | `config/relearn-t2i-pin.toml` — `eval_image` `ghcr.io/cortexlm/relearn-image-eval`, digest `sha256:81c40dc6…`, `relearn_git_sha` `54d3537f…` ([`CortexLM/relearn`](https://github.com/CortexLM/relearn) PR #3). Same digest is also published as `relearn-t2i-eval`. Digest-only, no floating tag. |
| Holdout | **done** | Commitment in git, records operator-side (`RELEARN_T2I_HOLDOUT_FILE`) and verified at boot. Mismatch → submissions 503. |
| Live harvest | **done** | `crates/relearn-t2i-harvest` over the shared `harvest-pod` transport; wired from `LIUM_API_KEY` + `LIUM_SSH_PUBLIC_KEY_FILE`, reported as `live_harvest_wired`. Missing `RELEARN_T2I_JUDGE_API_URL` → `can_score: false` and 503 before rent. Contaminated / empty-evidence manifests are rejected without a pod. |
| Champion baseline | **done** | `RELEARN_T2I_BASE_CHAMPION_FILE` (verified against the pin) or the wired harvest. A live host never inherits sim numbers; without one every submission 503s. |
| Emission | **1500 bps** | Default share. |
| Spec | live | [`RELEARN-IMAGE.md`](RELEARN-IMAGE.md). |

## relearn-agent-challenge

| Component | Status | Notes |
|-----------|--------|-------|
| Crates (`crates/relearn-agent-*`) | **done** | task (episode type, commitment, pin), score (trace-replay / tool-ablation / observation-shuffle / canary / contamination gates), store, eval, harvest, http, challenge. |
| Binary (`bins/relearn-agent-challenge`) | **done** | HTTP API on `:8099`. |
| Compose / images | **done** | Default compose + `images.yml` target `relearn-agent-challenge`. |
| Base pin | **done** | `Qwen/Qwen3.8-27B` — the same checkpoint as `relearn`, refused at pin load if swapped. A second post-train of that base, not a rename of it. |
| Eval pin | **done** | `config/relearn-agent-pin.toml` — `eval_image_digest` `sha256:4db52b13…` + `relearn_git_sha` `54d3537f…` ([`CortexLM/relearn`](https://github.com/CortexLM/relearn) PR #3). Digest-only, no floating tag. |
| Episodes | **done** | Commitment in git, episodes operator-side (`RELEARN_AGENT_HOLDOUT_FILE`) and verified at boot. An episode needing no tool call is refused at load. |
| Live harvest | **done** | `crates/relearn-agent-harvest`; the request names the three required arms so a missing arm is the image's fault, not an ambiguity. Missing `RELEARN_TEACHER_API_URL` → `can_score: false` and 503 before rent. Contaminated / empty-evidence manifests are rejected without a pod. |
| Emission | **1500 bps** | Default share. |
| Spec | live | [`RELEARN-AGENT.md`](RELEARN-AGENT.md). |

## relearn-mm-challenge (off)

| Component | Status | Notes |
|-----------|--------|-------|
| Trust root | **off** | No row in `config/challenges.toml`: no emission, and no leaf signed by its key can verify. |
| Compose | **off** | `mm` profile only; `assert-compose-matrix.sh` fails if it renders on a default or master stack. |
| Crates (`crates/relearn-mm-*`) | **done** | task (permissive encoder policy), score (LM-intact hard gate, vision + agentic paired tests, pixel-shuffle control), store, eval, http, challenge. |
| Binary (`bins/relearn-mm-challenge`) | **done** | HTTP API on `:8098`. |
| Compose / images | **profile-gated** | `mm` profile; no `images.yml` target while it is off. |
| Encoder pin | **done** | `google/siglip2-so400m-patch14-384` (Apache-2.0, card verified). Miner encoders must be Apache-2.0 / MIT / BSD / ISC. |
| Eval pin | **v0** | `config/relearn-mm-pin.toml` — `eval_image_digest` empty until first green challenge CI. |
| LM gate | **done** | Text holdout rerun vs champion − ε; encoder-only submissions must hash-match `RELEARN_MM_CHAMPION_LM_HASH`. |
| Emission | **0** | No trust-root row. |
| Spec | off | [`RELEARN-MM.md`](RELEARN-MM.md). |

## bounty-challenge

| Component | Status | Notes |
|-----------|--------|-------|
| Crates (`crates/bounty-*`) | **done** | task (pairing), score (precision × severity, triage-noise canary off the lattice), store, http (fail-closed ingest + quotas), challenge (backend public **consumer** + fail-closed leaf emitter). |
| Binary (`bins/bounty-challenge`) | **done** | Internal HTTP on `:8096` plus the emitter (backend feed → exact-`E` leaves → gateway `POST /v1/weights/raw`, `BOUNTY_EMIT_POLL_SECS`). Does **not** serve `/v1/public/*`. No feed (or an unreadable one) pays nobody: `E` is covered with `NoScore(ChallengeInternal)` so D24 holds and the share burns to uid 0. A scored epoch is never downgraded to a burn mid-epoch. |
| Miner CLI (`bins/cortex-bounty`) | **done** | `pair --hotkey`; Chat inject from `BOUNTY_CHAT_COMMAND`. |
| Compose / images | **done** | Default compose + `images.yml` target `bounty-challenge`. |
| Emission | **3000 bps** | Default share; operator can retune (sum `10000`). |
| Spec | live | [`BOUNTY.md`](BOUNTY.md). |

## Infrastructure

Agent/operator contracts: root [`AGENTS.md`](../AGENTS.md), [`deploy/AGENTS.md`](../deploy/AGENTS.md), [`docs/AGENTS.md`](AGENTS.md). Deploy detail remains in [`deploy/README.md`](../deploy/README.md).

| Component | Status | Notes |
|-----------|--------|-------|
| Terraform droplets | done | 4 of 4: staging master, staging validator, prod master, prod validator. |
| Staging master | done | Migrated to `/opt/base` CI-managed; old `/opt/gbase` stack torn down. |
| Staging validator | done | Redeployed from same commit; `bundle gateway signature invalid` resolved. |
| Prod master | pending | Droplet up, awaiting the mainnet owner wallet and the first `v*.*.*` tag. |
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
| relearn HTTP / promote | **done** | `POST /v1/submissions` freeze → unseal → paired judge; `POST /v1/admin/promote` bearer; never crowns a regression. Refuses to score at all without a digest-pinned eval image (or the `RELEARN_FORCE_SIM` opt-in). Contaminated / empty-evidence manifests are rejected **before** a Lium rent. |
| bounty HTTP / adjudicate | **done** | Internal ingest: `POST /v1/pair` (sr25519) + `POST /v1/reports`; `POST /v1/admin/adjudicate`. Scoring **fetches** CortexLM/backend `GET /v1/bounty/public/leaderboard` + `/reports` and emits signed leaves from those rows. Unset / unreachable / unparseable `BOUNTY_BACKEND_PUBLIC_URL` → `can_score: false`, reports **503**, and an all-`NoScore(ChallengeInternal)` leaf set that pays nobody while keeping D24 (fail-closed; never skip, and there is no sim — `BOUNTY_FORCE_SIM` is retired and ignored). Same fingerprint as any prior report (including closed ones) lands `duplicate`; title==body and token-thin bodies are `400`. |
| relearn Lium rails | **done** (fail-closed) | Reuses `prism-lium` client + `SimLiumBackend`. Live rent **and** live scoring refuse without a `sha256:` eval digest; sim only via `RELEARN_FORCE_SIM`; miner BYOK never logged. |
| design / prism product APIs | retired | Crates remain as unused libraries. |
| prism orchestration | done | DB-backed claim/execute/review/similarity/score state machine (`prism_submission` + append-only `prism_stage_event`), pre-pod screens (copy gate + static cheat + AST similarity) before Lium rent, sweeper (10h grace + pre-reclaim log harvest; skips live workers), **detached harness + resume-first boot/periodic reconcile** (reattach live pods via sealed BYOK; fail-closed only when unreattachable — `control_plane_restart` / `harness_detached`; `GET /v1/submissions/{id}/logs`), epoch-close batched D24 leaf emission with **WTA** (`prism-emit` outbox: `emitted_epoch` watermark + `prism_emit_cursor` + positive-score carry + `apply_wta`, migration 0012). `PRISM_MAX_CONCURRENT_EVALS` default/prod = 8. |
| prism recipe v1 | done | `prism-recipe` contract, fineweb-edu pinned shard (URL + SHA-256, harness re-verifies), 6h train / 7h pod caps, baseline sources, recipe pin hex on the API. |
| prism v3 harness | done (branch `prism-better`) | Multi-file harness package (`main.py` + `prismlib/`, miner code in `unshare --net` subprocess), seeded train stream with authoritative token counter, G6 probes, `prismlib/cheatguard.py` AST audit, METRICS_JSON v2, miner-chosen tokenizer, G5 RULER/BABILong/natural (pretrain-only), `RECIPE_VERSION 1.4.0`. |
| prism v3 eval battery | done (branch `prism-better`) | G1–G8 under `harness/eval/` (intrinsic, downstream, recall, reasoning, long-context, curve, inference, stability) + `eval/public_dev/` anchors family + `tests/smoke_battery.py`. |
| prism v3 two-phase pod flow | done (branch `prism-better`) | `PHASE_TRAIN_DONE` marker → post-train staging of private assets + secret seed (SSH on Lium, dir on Sim) → `.ready` gate (fail-closed) → eval phase. Private tier recorded as `eval_tier`. |
| prism v4 G2 benchmark leaf | **live default** | `PRISM_SCORING_MODE=benchmarks` (default) → equal-weight G2 public accuracies → `scoring_version` 4; never falls back to bits/token bpb. `prism-challenge rescore-g2` rewrites historical rows from stored `metrics_json`. |
| prism v3 composite scoring | done, opt-in | `prism-pipeline::composite` + `ScoringMode::Composite` (`PRISM_SCORING_MODE=composite` → `scoring_version` 3). Orchestrator persists the battery via `EvalStore`; parameter-cap breach is terminal `Score(0)` (`CAP_EXCEEDED`). |
| prism v3 eval store + Zone B | done (branch `prism-better`) | Migration 0017 (7 tables), `prism_store::eval::EvalStore`, memory + Postgres impls (`prism-eval-store`), composite finalization glue, `prism-zoneb` contract types; Zone B validated, labelled, never scored. |
| prism v3 attribution | done (branch `prism-better`) | `prism_recipe::attribution` 2×2 matrix builder + `POST /v1/submissions/{id}/attribution` returning run plans as JSON (operator-triggered execution); the route lives in `crates/prism-attribution` (split for the per-crate LOC cap). |
| prism v3 baselines | done (branch `prism-better`) | `crates/prism-recipe/baselines/` Transformer++ + hybrid delta reference trees embedded as `prism_recipe::baselines`; G1 scored keys promoted to tokenizer-neutral `org.g1.bits_per_byte_*`; `harness/eval/calibrate_anchors.py` fills references from baseline METRICS; numeric refs in `anchors/v0.json` remain **placeholder** until E6 GPU measurement + pre-registration. |
| prism top-model weights | done (branch `prism-better`) | Post-eval **secure receive** (master SSH pull → allowlisted stage + `RECEIPT.json`); admin `POST /v1/admin/artifacts/{id}/receive` for re-stage; publish verifies receipt (`PRISM_TOPMODEL_REQUIRE_WEIGHTS=1` fail-closed) + Release `prism-top-model`. |
| prism playground API | done (branch `prism-better`) | `POST /v1/admin/playground/complete` (admin Bearer) returns text + logprobs against parked top/specified checkpoint; journals to `{artifact_dir}/playground_journal.jsonl`; Sim stubs return structured diagnostics without pretending to infer. |
| prism inference traces | done (branch `prism-better`) | Battery G2–G5 persist prompt/choices/gold/selected/logprobs (G1 excerpts) in METRICS_JSON `inference_traces` (size-capped); `GET /v1/submissions/{id}/inference` paginated complete-view. |
| prism LLM review | done | `prism-review` quality + similarity-v3 prompts (versioned), OpenRouter client (key file only, never env), deterministic sim fallback; cheap `Copied` hard-zeros; cheap `Suspicious` hard-zeros only at `score ≥ 0.9` with non-trope evidence (`SUSPICIOUS_HARD_ZERO_THRESHOLD`); generic LM tropes coerced; copy/similarity/agentic corpus = champions (Score>0) + baseline. |
| prism API | done | Full status surface: submissions list/detail/events/status/jobs/recipe/baseline, idempotent accept, advisory `/precheck`; v3: `metrics?zone=a\|b`, `/inference`, `/anchors`, `/preregistration`, `/attribution`; admin Bearer on retry / gating reset / playground. |
| Phala / agent-v1 miner path | removed | External miners use HTTP submit only ([`external-miner/`](external-miner/)). |

## Known gaps

| Gap | Impact |
|-----|--------|
| DCAP verify holds the attest mutex | A cold Intel PCS fetch (up to 20 s) serialises attestation submissions. |
| DCAP error classification | Matches on `anyhow` message text; re-run `cargo test -p attest-policy --features dcap` after any `dcap-qvl` bump. |
| Relearn eval image digests | `relearn-image-eval` `sha256:81c40dc6…` and `relearn-agent-eval` `sha256:4db52b13…` are pinned (PR #3, `54d3537`). **`relearn-eval` is not pinned**: the last candidate, `sha256:cbc4bbb8…` (`f3cfa69`), exited 1 on a rented pod without `RELEARN_EVAL_OK` — no vLLM on the CUDA scoring image, transformers fallback, then `Qwen3VLVideoProcessor` crashed for want of torchvision. `relearn` submissions 503 until [`CortexLM/relearn`](https://github.com/CortexLM/relearn) ships an image with vLLM + torchvision. A live host still 503s after that until harvest + champion baseline are recorded. |
| Relearn Image / Agent holdout salts | Both committed commitments use documented **dev** salts so local and staging work out of the box. Production must rotate to a private salt **and** a private catalogue for each, replace the commitments, and re-sign. |
| Relearn holdout salt | The committed `holdout_commitment` is the CI / local one — a documented dev salt over a synthetic catalog so the stack boots without operator secrets. It is **not** the live seal. Production must rotate to a private salt **and** a private catalog, replace the commitment in `config/relearn-pin.toml`, and re-sign the trust root ([`../config/CEREMONY.md`](../config/CEREMONY.md)). |
| Relearn live scoring | Blocked on the eval image (no working digest). Behind that, the remaining blockers are operator state: the harvest (`live_harvest_wired`) and the champion baseline (`champion_baseline_recorded`). Each has its own **503** and its own boot-log line. Sim is opt-in (`RELEARN_FORCE_SIM=1`, CI / local only), reported as `eval_backend` on `/v1/status` and on the submit row — never a fallback. Refusals persist no row. |
| Relearn live harvest | Control-plane client is **done** (`crates/relearn-lium-harvest`): boot the digest-pinned image, deliver the request, read `RELEARN_METRICS=`, verify against pin + run identity, terminate with verification. Wired on the Lium path from `LIUM_API_KEY` + `LIUM_SSH_PUBLIC_KEY_FILE`; `/v1/status` reports `live_harvest_wired`. The **scoring code** ships in [`CortexLM/relearn`](https://github.com/CortexLM/relearn) and must implement the contract in [`RELEARN.md`](RELEARN.md) § Eval image contract — an image that does not print a bound `RELEARN_METRICS=` document is a 503, never a sim score. |
| Relearn holdout on rented pods | The harvest request carries the holdout items, so a rented pod sees the private split for the run. Mitigated by the digest-pinned image, `/tmp/relearn_eval` delivery, post-run scrub, and verified termination — not eliminated. Rotate salt + catalog and re-sign on suspicion. An in-enclave design would be needed to remove the exposure. |
| Relearn teacher / judge keys on rented pods | `RELEARN_TEACHER_API_KEY` and `RELEARN_T2I_JUDGE_API_KEY` are forwarded into the pod when set, because a Lium `InstanceSpec` carries no env and the image judges over HTTP. Delivered over stdin (never a command line) and scrubbed after the run. A missing URL refuses **before** rent. A miner-controlled pod could still spend the operator's quota — scope and rate-limit those credentials, or leave them unset when the pod reaches the API without auth. |
| Relearn champion baseline | Live hosts need an operator-recorded measurement (`RELEARN_BASE_CHAMPION_FILE`, verified against the pin's `eval_image_digest` + `holdout_commitment`). Unset means `champion_baseline_recorded: false` and every submission 503s before the gates. Sim hosts seed the sim baseline. |
| Bounty severity on the backend feed | Scoring credits a `valid` row only when the backend publishes a `severity`. Until CortexLM/backend emits it, valid rows land as `valid_unpriced`, no miner can be crowned, and the share burns. Fail-closed by design: an unpriced bug cannot be paid for. |
| Bounty scoring backend | The CortexLM/backend public feed is the only scorer. Without a readable `BOUNTY_BACKEND_PUBLIC_URL`, `POST /v1/reports` answers **503** rather than collecting bug-hunting work the host could never pay for, and the emitter pays nobody — it covers `E` with `ChallengeInternal` so the 3000 bps burns to uid 0 without 409ing every other challenge's seal. `BOUNTY_FORCE_SIM` is retired: a local scorer here would pay on adjudications no validator could reproduce. |
| Relearn public repo | [`CortexLM/relearn`](https://github.com/CortexLM/relearn) exists; `relearn_git_sha` is empty in `config/relearn-pin.toml` because no image from it has scored on a rented pod. Bump the SHA and the digest together, from a commit reachable on that repo's default branch. Seed mirror: `docs/external-miner/relearn-seed/`. |
| Mainnet (netuid 100) | Owner wallet not yet on this machine, so prod runs with `BASE_GATEWAY_REQUIRE_OWNER=0`. |
| Prod pin placeholders | `deploy/pins/prod.json` still ships zero-digests until the first successful promote; registry mode rejects placeholders. |
| Spaces backup secrets | First prod promote is fail-closed without `BASE_BACKUP_ENDPOINT` + `SPACES_ACCESS_KEY_ID` / `SPACES_SECRET_ACCESS_KEY` (or AWS_* fallbacks) in GitHub. |
| GitHub `production` environment | Enable required reviewers (and branch protection on `main` as desired) before relying on tag-driven prod; workflow already sets `environment: production`. |
| TLS ACME | Ports 80/443 open on the firewall; gateway TLS termination not shipped yet. |
