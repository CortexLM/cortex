# Proof v2 local integration

This is an opt-in development interface, not an end-to-end autonomous research
service. V1 topic payout rules and the Proof/Bounty 8000/2000 bps allocation
are unchanged. The default v1 store is in memory. The optional SQL journal now passes fresh workspace durability tests (2/2) after fixing embedded-migration tracking. Async writes persist SQL before memory and reject NaN/infinity. Production durability remains unproven: cross-process ID collisions/upserts, separate submission/score transactions, synchronous bypass and cancellation-induced memory lag remain.

## What is connected

`proof-challenge` optionally mounts signed v2 HTTP → Postgres commands. It starts
**no experiment worker**. The separate `proof-worker` library connects durable
intents → strict broker → resource adoption → headless runtime, with an
independent, model-free cleanup lane. No strict live Lium, credential-enrollment
or quote adapter is wired into that path.

### Measured Lium API semantics (live probe, 2026-09-07)

One split-GPU rent returned **400**; one whole-host **$0.25/hr** pod remained `PENDING` before termination. Two DELETEs returned **200**; GET returned **404** and the list was empty after **6 s**, with balance unchanged. This does not establish side-effect-free refusals generally, client-id support or its absence, stopped running-pod billing, a whole-hour tariff, or expiry enforcement. Existing `prism-lium` custom templates accept `docker_image=repo@digest`; a null separate digest field does not make digest pinning impossible. Live custom-image enforcement remains untested.

`proof-executor` runs paired scripts on actual local CPU in digest-pinned Docker
containers and retains stdout, stderr, exit status, wall time and failures.
Metrics and FLOPs come only from a trusted observer
([`crates/proof-measure`](../../crates/proof-measure)), optionally wired in
`proof-experiment`; `collect` still fails
closed with `UnobservedMeasurements` whenever FLOPs are unmeasured, and the
default `NoObserver` dispatches no runs at all. Only the test observer image has
been exercised, so these executions still create no admissible science: the real
`proof-eval` image reports no independent FLOPs. Clean static inspection raises
`ContractError` absent agent reproduction and verified FLOP evidence; forbidden
fabric rejects. CLI gates before judge/model calls without successful metrics.
Agent-led accounting is not implemented: the agent must determine experiment-appropriate accounting, retain reproducible evidence, and have that evidence verified by the controller. Neither a universal formula nor an arbitrary model assertion is sufficient.

### Judge egress (`JudgeEgress`)

The eval helper supports explicit `PROOF_JUDGE_PROXY=1` without Authorization only for the exact alias `http://proof-judge:8080/v1` and `chat/completions`; direct mode still requires a key. It loads miner artifacts
with `trust_remote_code=True`, putting arbitrary code in the process that reads
the holdout. Optional `JudgeEgress` is wired into `proof-experiment`, but does not
yet prove confidentiality or integrity: allowed arbitrary judge payloads and
artifact code remain risks. Its configured isolation is:

- the scoring container joins a per-run **`Internal`** Docker network with no
  route off the host, and is given empty `Dns`/`ExtraHosts`;
- the only other member is a controller-owned, digest-pinned proxy, dual-homed on
  the default bridge, that forwards **one** upstream origin and only the judge's
  own paths (`/chat/completions`, `/completions`, `/embeddings`);
- the API key **and the upstream address** are read by the controller and staged
  together into the proxy's anonymous volume, so neither appears in any
  container's environment, in a bind mount, in an image layer or in
  `docker inspect`; the proxy unlinks the file once loaded;
- `base_url` handed to the image is rewritten to the proxy alias, so the real
  judge host is never disclosed to the workload;
- a controller-side judge on literal loopback is supported through
  `allow_loopback_upstream`, which maps the Docker host gateway **into the proxy
  only**; the opt-in never covers private, link-local or named hosts;
- upstreams that are loopback, private, link-local, `.internal`, credentialed or
  non-HTTP(S) are refused, so the workload cannot be pointed at controller-side
  services.

`tests/egress.rs` exercises a TEST-ONLY synthetic hostile image, not the real
eval helper or scientific reproduction. Run only with authorization for a real
endpoint and key:

```sh
CORTEX_TEST_JUDGE_KEY_FILE=/absolute/private/judge.key \
CORTEX_TEST_JUDGE_BASE_URL=http://127.0.0.1:20128/v1 \
CORTEX_TEST_JUDGE_MODEL=<model> \
cargo test -p proof-measure --test egress -- --include-ignored --nocapture
```

The proxy rejects redirects, malformed framing, unexpected paths/queries and
oversized responses, and sanitizes errors. Rust upstream parsing rejects public
plaintext HTTP, loopback HTTPS (unsupported SNI), and local/mapped/link-local IP
variants (except explicit loopback HTTP opt-in). DNS-based enforcement and
exfiltration/integrity through arbitrary successful payloads remain unproven.

Observed against a live OpenAI-compatible endpoint: `judge: reachable:200`
carrying a genuine model completion, while `public_dns`, `public_http`,
`direct_upstream` and `docker_gateway` were all **blocked**, the environment held
no credential, the staged key was unreadable, and the workload saw only
`http://proof-judge:8080/v1` — never the upstream host. The test refuses to run
without an explicit base URL, model and key file, so it can never quietly pass
against a guessed endpoint. Leave `JudgeEgress` unset to keep the container fully
network-free; the live image then fails closed. These four sampled escapes and
a real completion do not establish universal isolation.

The separate opt-in `proof-atlas` binary connects finalized-round scheduling,
private Atlas IPC and signed-byte publication to a concrete strict gateway
transport/receiver. Local tests cover frozen evidence → signed Proof leaves →
gateway intake/seal → independent validator recomputation, using **synthetic
science and chain/provider inputs**. An explicitly ignored regression also passed
the real scheduler/Postgres → headless runtime/Docker Python → private decision →
strict HTTP publication/readback → production seal helper → served-vector path.
Its inference is synthetic; it does not exercise the operator admin seal HTTP
route. A separate real authorized model → isolated Python → private controller
round trip passed as a synthetic IPC test, not scientific reproduction.
No receiver deployment or live round publication has been established.

Neither an accepted experiment nor accepted consent means that it has run,
earned credit or triggered a rental. A controller must publish the quote and
drive the durable intent. `can_score` on the historical v1 API does not describe
v2 readiness.

## Explicit activation

1. Apply the canonical shared migrations through
   [`0028_proof_atlas_worker.sql`](../../crates/db/migrations/0028_proof_atlas_worker.sql)
   using a separate owner connection. The additive v2 sequence is **0020–0028**,
   including gateway publication (0025), execution retention (0026), experiment
   workers (0027) and Atlas workers (0028). Test only with a disposable database;
   neither service migrates it. `db::test_pool` now matches these migrations,
   including append-only runtime events and column-level update grants.
2. Store a restricted **`base_app`** database URL in an operator-only secret
   file. Never put its contents in arguments, logs, examples or git.
3. Set `PROOF_AUTONOMY_DATABASE_URL_FILE` to that file when launching
   `proof-challenge`. Missing/invalid configuration or an owner connection
   refuses startup. No in-memory or simulated fallback is selected.

Without this setting, the binary does not mount v2 routes. This change does
not enable them in Compose, start experiments or deploy anything. The HTTP router follows the
existing Axum **0.8** API (documentation checked against 0.8.4); persistence uses
SQLx **0.8.6**.

Existing `0020` in-flight dispatches have no recorded dispatch timestamp. Do not
invent a timestamp or renew their spend budget during migration. Reconcile
those jobs with the provider out of band before enabling a new worker.

### Separate Atlas operator contract

[`bins/proof-atlas/src/main.rs`](../../bins/proof-atlas/src/main.rs) is the
authoritative binary configuration. There is no automatic enablement or default
provider/endpoint. Provision an operator-private JSON file outside the repository
with all fields below; unknown fields are rejected:

| Object | Required JSON fields |
|--------|----------------------|
| Top level | `schema_version` (exactly `1`), `database_url_file`, `proof_secret_file`, `proof_pin_file`, `chain_endpoints`, `gateway_url`, `netuid`, `anchor_block`, `policy`, `headless` |
| `headless` | `node`, `loader`, `entrypoint`, `tsconfig`, `private_root`, `model_config_file`, `kernel_python`, `runtime_pythonpath`, `kernel`, `budget`; optional `pid_namespace` (absolute `unshare` path) |
| `headless.kernel` | `image`, `memoryMb`, `workspaceMb`, `cpus`, `pids`, `seconds` |
| `headless.budget` | `maxDepth`, `maxChildren`, `maxConcurrentCalls`, `maxCalls`, `maxReservedTokens`, `maxReservedMicroUsd`, `timeoutMs` |

The nested field casing is intentional; see
[`HeadlessConfig`](../../crates/proof-worker/src/launch.rs).
`chain_endpoints` is an operator-selected endpoint string (comma-separated for
failover), `gateway_url` is the strict receiver's base URL, and `netuid` /
`anchor_block` must match the operator's intended network and round boundary.
`policy` is nonempty Atlas policy text, at most 64 KiB, not a path.

Use absolute canonical paths. The config, restricted database URL, Proof pin
copy and signing-key files must be owner-private regular files (use `0600`),
with private `0700` immediate parents, safe ancestors and no symlinks or hard
links. The signer accepts a raw 32-byte key or its hex encoding; bytes are read
from the checked open file, without reopening the key path. Its derived public
key must equal the pin's `topic_pubkey`. The database must already be migrated
and restricted; never use its owner connection for this service.

Pre-create `headless.private_root` as a canonical, owner-private `0700` directory.
Use installed Node, the fork's `node_modules/tsx/dist/loader.mjs`,
`packages/coding-agent/src/cortex/headless-cli.ts`, `tsconfig.json`, the isolated
kernel Python and `prime-agent-runtime/src` for the corresponding path fields.
The kernel image must already be installed and digest-pinned; do not invent a
digest. State/journals, attempt sockets and model/key files stay outside kernel
workspaces. Do not delete recovery state to obtain a fresh budget.

The host-only `model_config_file` follows
[`CortexModelConfig`](../../agents/atlas/packages/coding-agent/src/cortex/headless-config.ts):
`schema_version` (`1`), `provider`, `model`, `api`, `baseUrl`, `apiKeyFile`,
`reasoning`, `contextWindow`, `maxTokens`, and
`cost.{input,output,cacheRead,cacheWrite}` are required. `api` is
`openai-responses`, `openai-completions` or `anthropic-messages`. Configure only
an authorized provider and verified accounting assumptions; prices are USD per
million tokens, not measured charges. Keep the model file and separate
`apiKeyFile` private (`0600` in `0700` parents), outside state/workspace/sandbox.
HTTPS is required unless `allowLoopbackHttp: true` explicitly permits literal
`127.0.0.1` or `[::1]` HTTP; `localhost` and alternative address spellings do not
qualify. There is no Factory/Prime, environment-key or alternate-model fallback.
The [headless contract](../../agents/atlas/packages/coding-agent/docs/cortex-headless.md)
defines the full bounds and recovery rules.

From the repository root, with `PROOF_ATLAS_CONFIG_FILE` set to the absolute
private config **path**, not its contents:

```sh
: "${PROOF_ATLAS_CONFIG_FILE:?set the absolute private Atlas config path}"
cargo run --locked -p proof-atlas-bin --bin proof-atlas -- \
  --config "$PROOF_ATLAS_CONFIG_FILE" --check
```

`--config` can be omitted when `PROOF_ATLAS_CONFIG_FILE` is set. `--check` validates
Rust-side configuration/path checks, private files, signer/pin, policy and
restricted migrated DB. It does **not** start the TypeScript launcher, initialize
or validate its provider/model configuration, check installed kernel image
presence, prove network/chain/gateway compatibility, or deploy anything.
The blocking RPC client is constructed through `spawn_blocking`. Three scoped
binary startup/shutdown regressions passed, including SIGTERM while waiting for
finality; canonical DB migration and private-file tests also passed.

Only after separate operator authorization for inference and gateway writes,
start the scheduler/publisher by omitting `--check`:

```sh
: "${PROOF_ATLAS_CONFIG_FILE:?set the absolute private Atlas config path}"
cargo run --locked -p proof-atlas-bin --bin proof-atlas -- \
  --config "$PROOF_ATLAS_CONFIG_FILE"
```

Normal service mode can invoke the model and publish rounds. It never starts an
experiment/rental worker, calls the sealer or submits chain weights.

### Separate local experiment worker (`proof-experiment`)

[`bins/proof-experiment/src/main.rs`](../../bins/proof-experiment/src/main.rs)
is the opt-in service that runs `proof_worker::ExperimentWorker` end to end on
**explicitly local** CPU capacity. It is not a Lium adapter: the provider in
[`crates/proof-local-provider`](../../crates/proof-local-provider) claims one of
`slots` durable rows in `proof_local_slot` (migration
[`0029_proof_local_provider.sql`](../../crates/db/migrations/0029_proof_local_provider.sql),
append-only `proof_local_event` history) as the only "rental". Nothing is ever
charged; quotes carry `hourly_total_microusd = 1` / `maximum_total_microusd = 1`
only because the quote schema rejects zero. That is an accounting placeholder,
and `gpu_type = "none-local-cpu"` / `gpu_count = 1` likewise satisfy schema
bounds without any GPU. `image` is `local-docker-cpu`, `image_digest` is the
configured local image ID, `provider_fingerprint` commits to the observed
daemon engine ID plus that image ID, and `offer_id` is derived from it.
Resource ids are `local-<intent uuid>`; `DurableExecutor::bind_local_target`
accepts them because the quote's `image_digest` equals the executor image.

Apply migrations through `0030` with the owner connection first. Provision an
operator-private JSON file (same private-file rules as Atlas); unknown fields
are rejected:

| Field | Meaning |
|-------|---------|
| `schema_version` | exactly `1` |
| `database_url_file` | restricted `base_app` URL file |
| `proof_pin_file` | Proof pin copy (validated) |
| `proof_secret_file` | private Proof mini-secret (32 raw bytes or hex); its public key must equal the pin's `topic_pubkey`, even with `publisher.kind = "none"` |
| `chain_endpoints`, `netuid` | finalized-chain read endpoint for recipe epoch checks |
| `publisher` | `{"kind":"none"}` (evidence stays retained locally, publication always fails, never rewardable) or `{"kind":"gateway","gateway_url":"https://…"}` (see [Evidence publication](#evidence-publication-v2evidenceproof)); `http://` is accepted only for loopback |
| `docker_socket` | absolute trusted daemon socket |
| `image_id` | exact local image ID `sha256:…` used for quotes and execution; not a tag |
| `slots` | 1..=64 concurrent local claims |
| `lifetime_seconds` | 60..=86400 grant lifetime per claim |
| `holdout_store` | absolute existing directory bind-mounted read-only into the observer container; optional `DockerObserver` wiring exists; `collect` stays fail-closed (`UnobservedMeasurements`) without verified FLOP evidence |
| `policy`, `headless` | operator policy text and the same `HeadlessConfig` as Atlas |

```sh
: "${PROOF_EXPERIMENT_CONFIG_FILE:?set the absolute private config path}"
cargo run --locked -p proof-experiment-bin --bin proof-experiment -- \
  --config "$PROOF_EXPERIMENT_CONFIG_FILE" --check
```

`--check` validates private files, pin, headless paths/bounds, the restricted
migrated database (including `proof_local_*` privileges), that the daemon
serves the exact image ID, and materializes the slot rows. It does **not**
start the TypeScript launcher or a model, create containers, claim a slot,
prove chain/gateway reachability, or publish anything. Without `--check` the
worker polls for durable experiments, quotes local slots, runs the headless
agent against the local executor, and cleans up by force-removing containers
labelled `cortex.proof.experiment=<id>` from the pinned image before releasing
the slot; SIGTERM/Ctrl-C stops it in order. Three startup regressions and five
provider regressions run against a disposable Postgres plus the local daemon.

## Miner API

All paths below are relative to the Proof service. The gateway prefix, when
routed by an operator, is `/challenge/proof`. Requests are JSON, limited to
32 KiB. Unknown fields are rejected.

| POST path | JSON body | Result |
|-----------|-----------|--------|
| `/v2/experiments` | `{ "request": CreateExperiment, "authorization": SignedAction }` | Created experiment |
| `/v2/experiments/{id}/view` | `{ "request": { "experiment_id": id }, "authorization": SignedAction }` | Experiment, current exact quote, resources |
| `/v2/experiments/{id}/consent` | `{ "revision": integer, "consent": SignedConsent }` | Approved experiment and one durable rent intent |
| `/v2/experiments/{id}/cancel` | `{ "request": { "experiment_id": id, "revision": integer }, "authorization": SignedAction }` | Cancellation requested, not deletion certified |

`CreateExperiment` contains `id`, `account_id` and `recipe_digest`. The account
must already be bound to the signing miner by a trusted broker. It is an opaque
account UUID, never an API key.

The source-of-truth schemas are
[`consent.rs`](../../crates/proof-autonomy/src/consent.rs) and
[`types.rs`](../../crates/proof-autonomy-pg/src/types.rs).

`SignedAction` binds the 64-hex miner public key, fresh UUID nonce, expiry
(at most 300 seconds ahead), method `POST`, **service-relative path** and the
canonical SHA-256 commitment of `request`, not the outer envelope. Its sr25519
signature uses `cortex-proof-action-v1` over the canonical commitment of the
ordered tuple `(miner_hotkey, nonce, expires_at, method, path, body_digest)`.
The view also consumes a nonce. No unsigned identity header grants read access.

`SignedConsent` signs the canonical commitment of the **entire stored quote**
under `cortex-proof-consent-v1`. The quote binds account, experiment, recipe,
offer, hardware, image digest, provider fingerprint, integer micro-USD prices,
maximum total cost, lifetime and expiry. Never consent to a displayed subset.
Replacing a quote invalidates any prior pending rental intent.

Sign with the repository crypto helpers, which apply domain separation. Do
not sign raw JSON text or invent a different canonicalization.

- Invalid authorization or ownership: **403**.
- Stale revision, replay or consumed quote: **409**.
- Invalid/oversized command: **400**.
- Intake quota: **429**, at most eight nonterminal experiments and 32 creations
  per hour per account. Rejected creates do not consume the nonce.
- Missing/corrupt durable state: **503**.
- Trusted account enrollment, lease acquisition, quote publication, provider
  callbacks, evidence admission and reward signing are **not public routes**.

## Cancellation and provider boundary

Cancellation does not call an LLM. It expires the controller lease, suppresses
unsent rents and leaves issued rents available for reconciliation. It cannot
unsend an already-issued provider request. A late result is retained under its
original account/experiment and stays quarantined.

Resource names do not authorize operations. Only stored account/resource ids,
live controller fences and unexpired grants permit execution. Neither restart
nor takeover extends the original spending deadline. Account revocation blocks
execution, but the controller can still attempt cleanup using the original
account binding.

`proof-worker` refreshes quotes, requires strict adoption before execution and
persists runtime identity and the original deadline before invocation. A second
invocation under the same controller fence is refused; recovery must preserve
the original run UUID, deadline, resume mode and budget journal. Independent
cleanup capacity is not consumed by model execution. These are locally tested
controller guarantees, not an enabled experiment service.

The generic controller enforces the original database runtime deadline even
when an agent ignores its stop signal. Shutdown aborts pending lease acquisition;
suspended operation and heartbeat futures are dropped before database bookkeeping.
Atlas cancellation also prevents a blocking RPC result completed after shutdown
from freezing a new round. These controls do not cancel an already-issued
provider request or establish remote termination.

The strict [`MinerProvider`](../../crates/proof-broker/src/lib.rs) contract requires:

- Fresh exact-offer verification and provider-enforced expiry/price bounds.
- Miner credential resolution outside agents and kernels, no process/operator
  key fallback.
- One rent call per committed intent. Ambiguous creation is inspected by
  authenticated account and provider request id, never retried by pod name.
- A persisted deletion id, then a separate authoritative check of resource
  absence **and stopped billing**. A DELETE acknowledgement, malformed list,
  unauthorized response or generic 404 cannot certify this.

The existing legacy Lium client does not prove those guarantees and is not
substituted. Atomic request-id, expiry, cost and stopped-billing guarantees have
**not been established** for the live integration; this is not proof that the
provider lacks them. Local fake-provider tests are not evidence of
provider-enforced isolation. Miner-administrator control also remains a
scientific measurement risk, even with externally retained evidence.

## Retained science and private runtime

`proof-research` verifies registered signed topics, sealed baselines and recipes,
then admits 3–20 paired observations under a current experiment/resource lease.
Scripts and logs are retained by digest. Every candidate must satisfy the
existing topic gates against both the sealed and reproduced baselines. A model
verdict never substitutes for measurements, contamination checks or cleanup.
Mean and standard error are retained; uncertainty alone does not create credit.
Publication must confirm the exact public allowlist, and cleanup must finish,
before a passing record becomes rewardable. Scientific gate rejections remain
retained without credit. The local Docker executor now archives execution output
and failures, but neither script-reported numbers nor process wall time supply
the missing independent scientific metric/FLOPs observations.

W&B SDK **v0.28.0** sends automatic runtime/environment telemetry outside the
seven-field public allowlist and no setting suppresses it; never enable the stock
SDK as a substitute for an allowlist-enforcing adapter.
[`crates/proof-wandb`](../../crates/proof-wandb) uploads exactly the allowlisted
record over the direct GraphQL artifact path instead, but it is wired into no
binary and has never contacted W&B: server acceptance of a null
`createArtifactManifest.runName` is unverified.

`proof-runtime` uses a private Unix socket outside all agent workspaces. Requests
are at most 64 KiB, responses 128 KiB, with a 30-second transport deadline.
The parent directory must be canonical and mode `0700`; the socket is `0600`.
Never mount this router on public HTTP or give kernels its socket, database
credentials, provider credentials or signing keys.

Experiment operations bind one recipe, resource and controller fence.
`execute`/`kernel` authorize before and after the call and poll ownership every
200 ms. The target execution adapter must stop expired/cancelled work itself:
dropping a future does not certify remote termination. `collect` accepts no
agent measurements. `report` retains bounded, explicitly untrusted narrative.

Experiment and Atlas launchers share `HeadlessProcess`, which supervises the real
`CortexRuntime` with host-only model configuration and a new private socket per
attempt. Resume retains the durable run UUID, original absolute deadline and
budget journal; losing a checkpoint is not permission to restart with a new
budget. Status output is lifecycle/accounting only, never scientific evidence.

Seventeen process-supervision tests passed, covering inherited pipes, leader
exit, TERM followed by KILL after a five-second grace period, future-drop/reaper
behavior, and a `setsid` escape from the process group. Process-group
supervision alone does **not** stop that escape (one regression documents it).
Set `headless.pid_namespace` to an absolute `unshare` executable so the launcher
runs as PID 1 of a private PID namespace (`--user --map-current-user --pid
--fork --kill-child=SIGKILL`): the kernel then kills every descendant when the
launcher exits or is killed, including escaped sessions. This is only PID
containment, not a mount, network or user sandbox; the launcher keeps the same
uid and filesystem view. The full headless-delivery regression runs under it
when `/usr/bin/unshare` exists.

Atlas uses the same private protocol with role `atlas`, round id and frozen
document commitment. Its only operations are:

- `history`: `{offset?, limit}` with limit 1–32, returning admitted contributions
  and previous awards.
- `read_evidence`: the same pagination for summaries; `{evidence_digest}` for
  private signed recipes and observations; or
  `{evidence_digest, artifact_digest, offset, limit}` for retained bytes in
  hex-encoded pages of at most 16 KiB. Reads cannot escape the frozen corpus.
- `submit_decision`: a v2 `AtlasDecision`, validated and signed by the controller.
  Acceptance queues publication; it does not certify payment.

## Finalized rounds and publication

`proof-rounds` freezes contiguous windows ending at
`anchor_block + (round + 1) * 360`. `chain-live` reads finality, then pins epoch,
timestamp and metagraph to the boundary hash. There is no optimistic-tip fallback.
The corpus uses the boundary epoch and timestamp cutoff for retained evidence
and completed lifecycle events. The local executor includes a finalized-chain
`LiveEpoch` adapter. Real RPC-backed, independently measured collection has not
been demonstrated; evidence epochs remain trusted controller inputs.

Frozen documents bind configuration, Proof public key, participants, evidence
and previous history. The first round is zero; a new round requires the prior
decision and an exact publication receipt. Competing schedulers serialize in
Postgres. Controller leases last 1–300 seconds and retain their fencing counters.

Contribution identity commits topic id, baseline metrics commitment and candidate
script digest. New seeds or rentals cannot reset the same contribution's age.
Prior awarded ownership cannot transfer to a later copier. This is not semantic
novelty detection or fraud prevention. Omitted prior contributions retain zero
credit rather than disappearing from history. Changed decay plans require the
exact previous-award digest and rationale, never a reset of age or initial credit.

The controller signs one exact-participant leaf set, retains its SCALE bytes,
and retries those **same bytes**, not fresh randomized signatures. Proof uses
absolute allocations plus the real UID-0 residual, preserving decay through
normalization while retaining Proof/Bounty **8000/2000 bps**.
Existing validator pure-burn restrictions are unchanged.

Research rounds and chain epochs are separate clocks. Multiple rounds may map
to one epoch. The concrete `proof-publication` / `gateway-proof` path implements
`POST /v2/weights/proof/rounds` and `GET /v2/weights/proof/rounds/{round}` for an
immutable full-batch signed publication. It rejects older-round replacement and
reconciles both the exact receipt **and** read-back bytes. HTTP **409 is never
publication success**; the legacy raw-weight client's behavior is not reused.

The gateway receiver is separately opt-in with `BASE_GATEWAY_PROOF_V2=1` and an
explicit `BASE_GATEWAY_PROOF_ANCHOR_BLOCK`. Activation is sticky in durable state,
blocks legacy Proof ingress and prevents serving stale seals; unsetting the flag
is not a rollback. The receiver does not seal automatically. Only the existing
explicit seal path can produce a new seal. These controls are tested locally,
including headless delivery through the production `seal_epoch` helper, but not
deployed receiver/publication, operator admin seal HTTP coordination or chain
payment. Atlas service startup must not be treated as permission to activate
this gateway mode.

### Evidence publication (`/v2/evidence/proof`)

`proof-publication` also carries `EvidencePublication`: the seven-field
`PublicEvidence` allowlist (floats as IEEE-754 bits so bytes round-trip),
SCALE-encoded and signed by the Proof key under the new domain tag
`base-proof-evidence-v1`. Nothing else (resource ids, logs, rationale, holdout
records) is on the wire. `GatewayEvidencePublisher::new(url, proof_pubkey, secret)`
refuses a signer that does not match the pin, then implements
`proof_research::EvidencePublisher`: `POST /v2/evidence/proof` (octet-stream,
≤4 KiB) must answer **200** with `{evidence_digest, digest}`, and
`GET /v2/evidence/proof/{evidence_digest}` must return bytes that verify under
the pinned key and decode to exactly the sent document. Only then does it return
`digest`, which equals `commitment(&PublicEvidence)` — the value
`ResearchStore::publish` compares against its retained summary before marking
`proof_publication.delivered`. Timeouts, non-200 and **409** are unconfirmed;
the store releases its fence and a later attempt retries the same document.

The receiver lives in `gateway-proof` under the same sticky
`BASE_GATEWAY_PROOF_V2=1` activation and Proof key as the round routes.
Migration [`0030_proof_evidence.sql`](../../crates/db/migrations/0030_proof_evidence.sql)
adds append-only `gateway_proof_evidence` (`evidence_digest` primary key,
receipt `digest`, `signature`, exact `wire`). The first accepted bytes for an
evidence digest are final and are what readback serves; a re-POST with the same
content (any valid Proof signature, since sr25519 signatures are randomized)
acknowledges with the same receipt, different content for the same digest is
**409**, an unknown or foreign signature is **401**, oversize is **413**, and
trailing/noncanonical SCALE or a malformed digest path is **400**. Readback of
an unknown digest is **409**, never an empty 200.

Tests: `proof-publication` `tests/evidence.rs` (sign/verify/tamper, receipt vs
readback modes), `gateway-proof` `tests/evidence.rs` (accept, replay, resign,
conflict, 401/400/413, restart readback) and `proof-research`
`tests/gateway_publication.rs` (real `ResearchStore::publish` through the real
receiver on an in-process axum listener, `confirmed_digest` row, `rewardable`).
Not demonstrated: a deployed receiver, TLS to a remote gateway, or any payout
that reads `gateway_proof_evidence`; publication confirmation is a precondition
for reward, not a reward.

## Local verification

With `DATABASE_URL` pointing at a disposable local Postgres:

```sh
SQLX_OFFLINE=true cargo test --locked -p proof-autonomy-pg -p proof-broker \
  -p proof-autonomy-http -p proof-research -p proof-runtime -p proof-rounds \
  -p proof-autonomy -p proof-worker -p proof-executor -p proof-atlas-worker \
  -p proof-publication -p gateway-proof -p proof-atlas-bin -p chain-live \
  -p proof-challenge-bin -- --test-threads=1
```

Tests create isolated schemas and use synthetic signatures. Without
`DATABASE_URL`, database tests skip. The default test command leaves ignored
integrations disabled; it does not authorize paid inference, Lium rentals,
W&B publication, live gateway writes or chain submission.

The real-kernel bridge regression additionally requires
`CORTEX_TEST_RUNTIME_BRIDGE=1`, `PRIME_AGENT_KERNEL_PYTHON`, `PYTHONPATH` for the
fork's Python runtime and a valid `CORTEX_TEST_KERNEL_IMAGE`. It skips otherwise.

Latest verified checkpoints:

- Earlier routine workspace pass: **151 binaries, 1304 passed, 30 ignored**.
  Final workspace checkpoint: **151 executables, 1304 passed, 0 failed,
  31 ignored**. Separately selected ignored tests passed: durability **2**,
  local Docker **14**, synthetic connected `headless_delivery` **1**, and
  live-Astra egress **2**; these are not added to routine totals.
- Final workspace strict Clippy, formatting, doctests and five xtask gates
  passed. `cargo deny` passed with warnings. The task database was stopped.
- Fresh workspace durability tests: **2/2 passed**. Migration 0031 was absent
  from embedded migration sets; `crates/db/build.rs` now emits
  `cargo:rerun-if-changed=migrations` to track migration changes. This fixes the
  diagnosed build issue, not the remaining production durability gaps above.
- Python: **31 tests passed**; proxy stdlib adversarial checks: **3 passed**.
- Rebuilt test proxy/probe against real Astra on **20128**: egress **2/2 passed**.
  The probe checks actual `judge.json` and excludes the nonsecret proxy flag
  from secret detection. This is synthetic science, not a real eval recipe.

Earlier verification checkpoint (2026-09-07; historical counts):

- Full Rust workspace: **138 test binaries**, **1260 passed**, **24 ignored**,
  **0 failed**, with the disposable DB and private runtime bridge enabled.
  Ignored integrations were separately selected as described below, not silently
  counted as exercised by the default run.
- Workspace formatting and strict Clippy passed. All 74 doctest suites passed
  (one actual doctest). The five xtask gates, dependency audit and local link
  checks passed. The dependency audit retained warnings, including duplicate
  versions, wildcard declarations and a metadata parse warning.
- Complete fork check: **993 files**. The selected RLM/runtime regressions:
  **123 passed across 15 files**, including the repaired heartbeat test import
  prerequisite. No live provider was used in this selection.

- Executor work reported 13 tests, including explicitly run ignored integrations.
  Worker controller regressions cover same-fence invocation refusal, original DB
  deadline enforcement and shutdown/lease acquisition; 17 process-supervision
  tests passed, including the PID-namespace containment described above.
- Atlas scheduler: 13 isolated tests passed; canonical migrations were retested.
  The cancellation regression prevents a completed blocking RPC from freezing a
  round after shutdown. Three binary startup/shutdown regressions, including
  SIGTERM during finality wait, passed; canonical DB migration, private-file/raw-key,
  signer and restricted-DB checks passed.
- [`headless_delivery.rs`](../../crates/proof-atlas-worker/tests/headless_delivery.rs)
  passed when explicitly selected: real scheduler/Postgres → `HeadlessProcess` /
  unmodified `CortexRuntime` → Docker Python → private controller decision →
  strict HTTP gateway/exact readback → production `seal_epoch` → real served
  router and independent Python vector **`[0.4, 0.4, 0.2]`**. A deliberately lost
  acknowledgement replays identical signed bytes without rerunning the model.
  Chain, science and inference are synthetic. The regression remains ignored and
  calls the seal helper directly, **not** the public operator admin seal HTTP route.
- One authorized Astra Rust → full runtime → isolated Python → private controller
  synthetic round trip passed in **7.23 s**, after fixing a warning being treated
  as a model error. Bounds were **4 calls**, **2048 output tokens/call** and
  **120 s**. Temporary **100 USD/million-token** rates were assumed accounting
  rates, **not verified tariffs or measured cost**. This created no science.
- That live regression is now `#[ignore]` and requires an explicit
  `CORTEX_TEST_HEADLESS_MODEL_CONFIG`, plus kernel prerequisites, even when
  selected with `--ignored`. Never opt into it as part of routine doc/test gates.

Remaining blockers: verified strict live provider/credential/quote adapters,
experiment-appropriate accounting with controller-verified reproducible evidence
in the real evaluation image, DNS-based egress enforcement and successful-payload
confidentiality/integrity verification, resolution of the remaining v1 SQL
journal production durability gaps, a W&B path verified against the live service,
and operator-authorized deployed receiver/publication/admin-seal verification. Enabling the public v2 routes still
starts none of these workers.

Public `ghcr.io/cortexlm/proof-eval` publication is authorized, not deployment.
Published candidate: `ghcr.io/cortexlm/proof-eval@sha256:9e32451e178a2e04f33be592ea73f7f89acf723631d53291dbab016c4690ee92`,
with tag `3e92bc28-flop-correction-candidate`. Anonymous tag and digest requests
both returned HTTP 200 with a matching hash. This is **not a production repin**;
the published image predates the latest proxy-helper corrections. The real GPU
image has not been run. No deployment, on-chain action or extra rental occurred.
