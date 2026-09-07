# Proof orchestration persistence

This crate implements the durable command boundary for new Atlas experiments.
[`proof-autonomy-http`](../proof-autonomy-http/src/lib.rs) mounts it in
`bins/proof-challenge` only when `PROOF_AUTONOMY_DATABASE_URL_FILE` is configured.
It starts no experiment worker and does not make historical in-memory submissions
durable.

The additive [0020 migration](../db/migrations/0020_proof_autonomy.sql) stores
miner-account bindings, experiments, exact quotes, consent, consumed action
nonces, lifecycle events, controller leases and service intents. Credentials
stay outside Postgres: `credential_ref` is an opaque UUID for the trusted
credential resolver, not an API key or a caller-selected filesystem path.
The [0021 migration](../db/migrations/0021_proof_resources.sql) adds durable
resource bindings, original spending deadlines, deletion ids and append-only
provider/deletion observations.

The canonical shared v2 migration sequence now runs **0020–0028**:
0022/0023 retain research and untrusted reports, 0024 freezes Atlas rounds,
[0025](../db/migrations/0025_proof_gateway.sql) persists gateway publication,
[0026](../db/migrations/0026_proof_execution.sql) retains executions,
[0027](../db/migrations/0027_proof_worker.sql) persists experiment workers and
[0028](../db/migrations/0028_proof_atlas_worker.sql) persists Atlas workers.
Apply them with a separate owner connection, not from service startup.
`db::test_pool` uses the same append-only runtime-event and column-grant contract
as canonical migrations; canonical migration coverage was retested.

## Implemented command path

1. A trusted broker registers a previously verified miner/account binding.
2. `create_experiment` verifies a signed request and inserts the experiment,
   initial event and nonce in one transaction.
3. A controller acquires a database lease and publishes an exact machine quote
   against the experiment revision.
4. `consent` verifies the stored quote and miner signature, consumes the quote
   once, advances the revision and inserts one provisioning intent atomically.
5. `begin_provision` rechecks the live fence, active account, stored consent and
   expiry, then commits `dispatched` before returning the approved quote.
   It does **not** call Lium.
6. `cancel` consumes a signed action without invoking a model, expires the
   controller lease and inserts cleanup intent. Unsent rentals become cancelled;
   dispatched rentals remain uncertain. Neither case claims a pod was deleted.
7. [`proof-broker`](../proof-broker/src/lib.rs) performs exact-offer preflight,
   dispatches once, and records the provider result. A timeout or crash becomes
   reconciliation work, never another rental. Late confirmed resources remain
   quarantined until a live controller adopts them.
8. Every resource grant checks the stored experiment/account/resource, active
   lease, account revocation and original spending deadline. Cleanup remains
   available after revocation/expiry and uses stored targets, never names.
9. Deletion intent precedes the provider call. Only separate verification of
   termination, with no unresolved rental left, permits final cleanup.
10. [`proof-worker`](../proof-worker/src/lib.rs) drives quote refresh, provisioning,
    reconciliation, strict adoption and headless execution, with an independent
    model-free cleanup lane. Its durable run UUID and original deadline precede
    invocation. Repeat invocation under the same fence is refused; recovery uses
    the original identity, deadline, resume state and budget journal.

Step 10 is a locally tested library path, not an enabled experiment service:
there is no strict live provider/credential/quote adapter wired to it.

The v2 routes and signature envelopes are documented in the
[local integration runbook](../../docs/runbooks/proof-autonomy-local.md).
Signed view requests authenticate reads; no unsigned hotkey header grants
access. Intake permits at most eight nonterminal experiments and 32 creations
per hour per registered account. Quota checks serialize with account intake;
cancellation is never quota-blocked.

## Invariants

- Nonce consumption, revision changes, events and intents commit together or
  roll back together. Retrying a successful authorization cannot rent twice.
- Expiry uses the database wall clock, including revalidation after lock waits.
- Experiment row locks serialize revision changes before controller lease locks.
  Fences increase on takeover, and ownership rows are never deleted on release.
- A stale controller cannot publish a quote, dispatch, renew or release its
  replacement's lease. Lease durations are bounded to 1–300 seconds.
- Replacing an approved quote invalidates its still-pending rental intent.
- Takeover changes dispatched work to `reconcile`, never back to `pending`.
- The generic experiment controller enforces the original DB runtime deadline
  even if an agent ignores stop. Shutdown aborts pending lease acquisition and
  drops suspended operation/heartbeat futures before DB bookkeeping. Atlas
  cancellation prevents a late blocking RPC completion from freezing a new round.
- The application role cannot rewrite or delete quotes, consent, nonces or
  lifecycle/provider/runtime events. Column-scoped updates permit resource status
  and deletion ids, not rebinding a resource or extending its spending deadline.
  Runtime identity/deadlines remain durable across takeover.

Database fencing does not prevent an already-issued provider request from
finishing after cancellation or lease loss. The strict provider interface
requires absolute authorization expiry, bounded spending and authoritative
request-id reconciliation. **No live Lium adapter currently implements this
contract.** The historical `prism-lium` client is not silently substituted.
Atomic request-id, expiry, cost and stopped-billing guarantees have only been
simulated locally, not established at Lium. This is not evidence that the
provider lacks those guarantees. Miner-administrator control of rented hardware
remains a measurement risk.

## Tests

Uses the existing SQLx **0.8.6** dependency, runtime-bound queries and transactions.
Integration tests use a real disposable Postgres and the `base_app` role, with a
separate migrated schema per test:

```sh
# DATABASE_URL must refer to a disposable local test database, never production.
SQLX_OFFLINE=true cargo test --locked -p proof-autonomy-pg -p proof-broker -p proof-autonomy-http -p db \
  --features db/testing -- --test-threads=1
```

Without `DATABASE_URL`, database tests skip explicitly. Local coverage includes
concurrent consent and controller acquisition, nonce replay, stale revisions,
revoked accounts, signature/scope changes, expiration while waiting for locks,
rollback after late failure, restart readback, uncertain dispatch takeover and
append-only permissions, late resources after cancellation, active grant
invalidation, provider refusal/uncertainty, HTTP authorization, quotas and
verified cleanup. Test quotes and signatures use synthetic fixtures;
no paid inference, real rental or publication occurs.

## Connected components and remaining integration

Local companion crates now retain scientific evidence (`proof-research`),
connect private experiment/Atlas IPC (`proof-runtime`, `proof-rounds`), and
persist frozen rounds, decision history and signed-byte publication outboxes.
Tests reach a real gateway seal and independent served-vector recomputation with
synthetic science and chain inputs.

`proof-executor` runs paired actual CPU scripts in digest-pinned local Docker and
retains stdout/stderr, exit status, wall time and failures. No independent
metric/FLOPs observer exists: `collect` always fails closed with
`UnobservedMeasurements`. These runs do not create successful science.
`proof-worker` and `proof-atlas-worker` share the real `HeadlessProcess` /
`CortexRuntime` launcher, host-only model config, attempt-scoped socket and
original budget journal. The model endpoint requires HTTPS unless literal
loopback HTTP is explicitly authorized with `allowLoopbackHttp: true`; private
`apiKeyFile` authentication has no Factory/environment fallback.
Seventeen process-supervision tests passed, including inherited pipes, leader
exit, TERM→KILL after five seconds, future-drop/reaper behavior and a `setsid`
escape. The optional `headless.pid_namespace` (`unshare --pid --kill-child`)
kills escaped descendants; it is PID containment only.

The separate `proof-atlas` binary is opt-in via `PROOF_ATLAS_CONFIG_FILE` /
`--config`; its private raw/hex signer must match the Proof pin. Its `--check`
validates Rust-side files/signer/policy and restricted DB, not TS model/provider
initialization, image presence, network/chain/gateway compatibility or deployment.
Normal mode drives only Atlas scheduling/publication, never rentals, sealer calls
or chain submission. See the
[runnable operator contract](../../docs/runbooks/proof-autonomy-local.md#separate-atlas-operator-contract).

`proof-publication` / `gateway-proof` now provide a concrete strict
`POST /v2/weights/proof/rounds` receiver and exact receipt **and** byte readback
through `GET /v2/weights/proof/rounds/{round}`. Full signed batches are immutable;
409 is not success. Sticky gateway activation blocks legacy Proof ingress and
stale seals, with the existing explicit seal path only. No deployed receiver or
live publication has been established.

Scoped reports cover 13 executor tests including explicitly run ignored
integrations, the worker same-fence refusal fix, and 13 isolated Atlas scheduler
tests. Canonical DB migration/private-file checks and three binary
startup/shutdown regressions passed, including SIGTERM while waiting for finality.
The separate Atlas cancellation regression prevents post-shutdown round freezing.

The explicitly ignored
[`headless_delivery.rs`](../proof-atlas-worker/tests/headless_delivery.rs)
passed the real scheduler/Postgres → `HeadlessProcess` / unmodified `CortexRuntime`
→ Docker Python → private controller decision → strict HTTP gateway/exact
readback → production `seal_epoch` → real served router and independent Python
vector `[0.4, 0.4, 0.2]` path. A deliberately lost acknowledgement replays identical
signed bytes without model rerun. Chain, science and inference are synthetic;
the test calls the seal helper directly, not the operator admin seal HTTP route.

One authorized Astra Rust → full runtime → isolated Python → private
controller synthetic test passed in 7.23 s, bounded to 4 calls, 2048 output
tokens/call and 120 s. Temporary 100 USD/million-token rates were assumed
accounting rates, not verified tariffs or measured cost. That live test is now
ignored and additionally requires explicit `CORTEX_TEST_HEADLESS_MODEL_CONFIG`.
Full-workspace validation including workers passed: 1260 tests across 138
binaries, with 24 ignored by default. Explicit executor/headless selections and
123 RLM/runtime regressions also passed. See the
[validation checkpoint](../../docs/runbooks/proof-autonomy-local.md#local-verification)
for checks, audit warnings and the boundary between real and synthetic inputs.

Remaining blockers are verified live provider/credential/quote adapters,
independent scientific metrics/FLOPs and chain-derived collection provenance,
safe public evidence publication, stronger process containment, and
operator-authorized deployed round-delivery/admin-seal verification.
W&B SDK v0.28.0 automatic runtime/environment
telemetry violates the seven-field allowlist; no safe adapter was created.
This does not establish production autonomy, GPU reproduction,
provider-enforced delegation or automatic payment. V1 and the Proof/Bounty
8000/2000 bps allocation remain unchanged.
