# Network v2 operator guide

Network v2 uses owner-signed `RunManifestV2`, `ht/2` messages and `/v2` routes.
V1 runs and signing domains stay separate; existing state directories are not
upgraded. Use this guide for the multi-rank client, not the single-rank v1
`join`/`run` workflow in [miner.md](miner.md). General layouts use
`N = DP * EP`, pipeline size 1; OpenDecision uses dense DP=N, EP=1, optional
ZeRO-1. Logical rank support is not hardware qualification.

## Current proof boundary

| Surface | Implemented/proven scope | Limit |
| --- | --- | --- |
| OpenDecision MLM, decision, distill | Each objective passed a CPU N2/DP2/EP1/ZeRO-1, H2/J1, four-client, two-round carry gate: independent audits, exact m/v/step/EF, capped tapes, test reward finality and checkpoint | Tiny functional evidence, not quality/calibration, throughput, profitability or CUDA. Original signed admission artifacts retained; final pytest production journals/checkpoints were removed by retention. Production outcomes are observed gates, not fresh artifact re-verification |
| Backend authority | One immutable owner-reviewed backend selection shared by admission, audit, trace geometry, referee and rollback; restart/cache contradictions reject. FIRST4 attempt008 passed bounded same-driver two-host CUDA bitwise qualification | Selection/dispatch and public CLI CPU execution are not CUDA attestation. Attempt008 qualifies only its frozen small decoder/source/image/profile; workload_allowed=false, current production client admission remains separate |
| Codec | One shared metadata parser validates complete payload before decode, exact expected shapes and raw-byte cap | Expected shapes must come from trusted manifest/state. Legacy `decompress(payload)` has no geometry cap; byte preflight does not bound an earlier storage fetch or legitimate tensor footprint |
| Scale | Allocator supports up to 1024 candidate metadata entries; legacy service path permits at most 16 without a capacity profile. Bounded capacity intake/opening fail-closed proof is APPROVE: authenticated profile opening rejects before cache, state loads, tensor allocation or publication | Allocator1024 is not service capacity. Tiny32 profile metadata/intake exist; heavy32 remains denied pending OS limits, consumer guards, bounded reads and execution charging |
| Rollback | CPU two-round exclusion, fresh replay, CAS/restart and checkpoint lineage; authenticated predecessor history reconstruction | History bound 16 rounds; larger history fails closed. No CUDA rollback proof |
| Relays | Local HTTP/ASGI lifecycle evidence; independent r2 local kind import/source109, TLS/CNI, upload/fixture-master acceptance, restart/replica/regional fallback, retention/GC and namespace denial PASS with owned cleanup. Product gateway namespace AND pod TCP8443 delta passed one four-overlay render regression | Original r2 outer caller exit1 retained; isolated READY phase repair passed two native-event cases, not another cluster run. APAC runtime, L0 finality and full D3/funded service integration unproven; no WAN or three-live-region independence claim |
| External services | Teacher-label and R2 client/config paths exist | Live OpenRouter/R2 checks deferred pending credentials. No live teacher or R2 success inferred from CPU fixtures |

Root ledger **20/23; three tasks remain open: allocation80, CPU verification and final push/CI**. Workspace evidence sources
(not shipped runtime authority): `K8S-TRANSPORT-R2-RESULT-VERIFY.md`,
`K8S-CALLER-READY-PHASE-FIX-VERIFY.md`,
`K8S-RELAY-PRODUCT-INTEGRATION-VERIFY.md`, `MINER-PUBLIC-CLI-VERIFY.md` in
`.omo/evidence/hypertrain-network/`; FIRST4 is
`qualification-usd50-20261009T181257Z/NEXT30-008-RESULT-VERIFY.md` there.
Public CLI proof is in-process CPU run-v2, not installed-CLI subprocess or CUDA.
Operator, consumer, wrapper and antiabuse contracts are complete within their
verified CPU scope. Capacity controller safeguards were previously reviewed under
`CAPACITY-CPU-FALLBACK-VERIFY.md`; current source requalification is pending after
type/runtime changes. Genuine80 execution remains unrun; fresh CPU approval is
absent and root execution admission remains pending. Production116 gates remain unproven.
Internal runtime-custody or `NOTSTOREACCEPTED` descriptors are private retention,
not accepted shadow-execution, backend/economic qualification or eligibility.
Accepted shadow-execution requires the original validated execution context,
actual backend qualification and economic authority; private retention cannot
bypass those gates or establish funded production116 acceptance.

These are scoped proofs, not an all-network release approval. Harness, CLI and
capacity work may change; publication must reconcile their final evidence before
removing a pending label. Ordinary checkpoint verification independently replays
outer aggregation; inner training/carry evidence comes from service audits and
anchor comparisons, not the checkpoint alone.

## Install and pin the run

Run from the repository root, Linux x86_64, Python 3.12. Commands below are
source-checked recipes; they were not executed during this documentation pass.

```bash
uv sync --frozen --extra trainer
uv run --frozen hypertrain-miner --help
uv run --frozen hypertrain-miner run-v2 --help
uv run --frozen hypertrain-miner watch --help
```

For OpenDecision also install the pinned `od` extra. The checkout's trainer extra
uses CPU Torch; CUDA requires the exact reviewed image/runtime, not merely
changing `device`. Obtain the owner hotkey, full wrapper run ID, image digest,
signed policies and verified dataset from the operator. Protect both hotkey and
coldkey seed files: regular files, mode 0600, owned by the running user; 32 raw
bytes or 64 hex characters. Never put seeds in the TOML or publish them.

Minimal `miner.toml` template; replace the capitalized values with actual pins:

```toml
api = "https://MASTER/challenge/hypertrain"
keyfile = "miner.key"
workdir = "work"
state_source = "states"
image_digest = "sha256:IMAGE_DIGEST"
run_id = "RUN_ID"
owner_hotkey = "OWNER_SS58"
device = "cpu"
```

`api` is the service base, without `/v2/runs/...`. `NetworkMiner` requires pinned
`run_id` and `owner_hotkey`; it verifies the signed wrapper before work.
`state_source` and `image_digest` remain required shared config fields; v2 round
inputs are fetched through the job/object endpoints. Relative paths resolve
against the TOML directory. Environment `HYPERTRAIN_MINER_<FIELD>` overrides
matching TOML fields; unknown TOML keys reject. Other existing fields:
`data_dir`, `poll_seconds` (2.0), `chunk_bytes` (67108864),
`allow_file_upload` (false). Do not assume these v1-oriented tuning fields alter
the v2 server's deadlines or relay chunk policy.

## Join, prove work, fund, graduate

Set `REQUEST_ID` to a fresh 64-hex request ID, `EXPIRY_BEACON` to a future
verified beacon round. Public join needs both keys:

```bash
uv run --frozen hypertrain-miner join-v2 --config miner.toml \
  --coldkey cold.key --request-id "$REQUEST_ID" --expiry "$EXPIRY_BEACON"
uv run --frozen hypertrain-miner status-v2 --config miner.toml
```

Applications/proofs are bound to the exact signed manifest, admission, nonce,
image, layout and epoch. A decoder/reset/other-objective admission cache cannot
authorize a carry run. Hardware hints are advisory; a fresh full-round work screen
and independent same-layout replay establish work, not physical GPU ownership.
Trial `WorkProof.challenge_hash` is the canonical `JoinChallenge` body digest,
which includes its nonce; `WorkScreenV2` is reserved by admission and nonce.
`WorkProof` reservations include admission and `challenge_hash`, not delta bytes
alone. Retry the exact stored signed operation: changing expiry or signature does
not create a new semantic attempt (`protocol/envelope_v2.py:replay_key`).

For each challenge, obtain the coordinator-signed envelope from
`GET /v2/runs/{run_id}/join/{admission_id}/challenge`. The operator must supply the
matching staged `IslandJobV1` and all files in its `object_paths` beside the job.
The CLI does not download/stage an admission job automatically. Given those
existing files:

```bash
uv run --frozen hypertrain-miner probe-v2 --config miner.toml \
  --job trial/job.json --challenge challenge.json > proof.json
uv run --frozen hypertrain-miner proof-v2 --config miner.toml --proof proof.json
```

`probe-v2` executes the screen and uploads original rank-0 state/EF/delta/leaves;
stdout contains the signed proof/screen pair. The operator independently prepares
and finalizes references through authenticated admin routes; miners cannot
self-finalize or turn a successful probe into ACTIVE status.

Current admission policy fixes E=4 and requires 12 clean finalized participations
plus canary blocks 1,2,3. The first four are shadow-only. Later PROBATION influence
also requires funded eligibility; ACTIVE requires current screening, sufficient
matured units locked, no pending dispute/outbox and the conservative economic
bound. Actual audit floor is 100% anchored-full replay in this phase, despite
lower nominal probation/base rates. Calendar age or a legacy bond flag is not
graduation. Join quotas, one concurrent non-ACTIVE identity per coldkey, funded
locks and known-owner caps bound exposure; they do not reveal every Sybil owner.

Prefix quotas use a stable coordinator-private HMAC namespace pinned atomically
in the store. Reopening the same store with the same coordinator does not reset
current prefix buckets. A populated legacy store without the namespace pin, or
a coordinator namespace mismatch, fails with HTTP 503 without resetting quotas.
No automatic migration is provided; separate operator resolution is required.

Funding uses signed `EscrowLock` via `POST /v2/runs/{run_id}/escrow/lock`, not a
CLI `--bond` flag. Test genesis has pinned nonfinancial origins; production mode
has separate reward authority/issuance bounds and no test-genesis conversion.
Rewards are issued only through accepted finality, uniquely tracked by origin,
then mature after vesting/dispute conditions. Transfers preserve origin units;
fraud burns collectible exposure rather than redistributing it. Shadow work does
not influence global theta; authorized production bootstrap issuance is a
separate policy, not proof of market-backed collateral.

## Train one live round and watch disputes

Only run after the operator has opened an eligible, funded round:

```bash
uv run --frozen hypertrain-miner run-v2 --config miner.toml --round 0
```

This follows accept, authenticated inputs, exact-N launcher, one island commit,
original artifacts, regional custody, master acceptance and delta submission.
Tokens derive from assigned samples times sequence length, not self-reported GPU
count. `UPLOADED` is not audit success, settlement or payment. Unlike v1 `run`,
`run-v2` has no `--rounds`/`--start` option. Preserve the work directory; do not
claim the CLI is a complete unattended production scheduler.

The decoder operation driver has an executable original four-message
AcceptV2/CommitV2/WorkProof/DeltaManifestV2 consumer. Capture records ordered
original requests, bounded responses and only the successful-response accepted
prefix. Canonical publication-path and same-Event probe-cancellation repairs are
review-approved; they are not current missing implementations. These scoped
`D2-DECODER-DRIVER-VERIFY.md`, `D2-CAPTURE-VERIFY.md` and
`SERVICE-ROLE-DISPATCH-VERIFY.md` approvals do not prove actual paid CUDA or
complete owner-accepted trial/audit/dispute/finality execution.

Production `WorkProof.challenge_hash` is the accepted canonical `CommitV2` body
digest. The upload-grant `commit_hash` query uses that same body digest, not the
delta hash or signed envelope hash. Identical delta bytes across live rounds are
valid with distinct explicit commit/challenge subjects; an omitted selector is
accepted only for a unique exact match, otherwise ambiguity rejects. See
`miner/core.py:NetworkMiner.run_round` and
`protocol/envelope_v2.py:replay_key`; signatures/expiry changes cannot bypass
their semantic reservations.

`run-v2 --job path/job.json` is a local staged-job launch returning artifact paths;
it does not submit a live round. Both forms use the same launcher. Failure,
deadline or rank disagreement prevents a partial validated publication.

Persist the exact original job JSON beside its `published/` directory. Watch
independently of training:

```bash
uv run --frozen hypertrain-miner watch --config miner.toml \
  --job work/round/job.json --timeout 30
```

This command performs one bounded long-poll pass, then exits with a processed
count; repeat under an operator supervisor. Timeout range is 0..30 seconds.
STEP/LAYER/OP answers and requested `StateServe` proofs come from validated
published traces/checkpoints. The coordinator signature covers the event's
`Turn.contest.w`; the watcher requires that round to match the original validated
publication job, alongside run and signer checks. Wrong-round events raise
`WATCH_PUBLICATION_CONTEXT` before event-driven trace/state responses, signing,
outbox writes, acknowledgments, sends or cursor advancement; they are not skipped
and acknowledged. Pending events and fetched batches are checked before delivery.
Durable cursor/outbox retries identical signed
replies; one watch-directory lock prevents concurrent watchers. Do not share a
watch directory across identities or discard it on restart. The CLI has no
`--level` or dispute-opening subcommand: signed `DisputeV2`, `BisectV2`,
`StateServe`, `ResolutionV2` and recovery routes own those transitions.
Independent referees use their own accepted current execution deadline while
preserving historical publication descriptors. Losing-party checks, funded
contests and fixed absolute horizons prevent arbitrary transcript resets.

## Weights, finality and custody

Flat aggregation retains each miner's original delta: norm preclip, previous
outer-update CClip, then deterministic integer allocation. Q=16777216; final
weights sum Q, each miner <=4194304, total probation <=4194304, plus signed
known-owner group bounds. No post-cap renormalization. Four eligible ACTIVE
miners can carry exact quarter weights; infeasible cohorts reject. Similarity
alone is not fraud evidence. Audits replay each miner's own assignment.

At most two speculative rounds may be applied; opening w+2 waits for w to settle
or be excluded and repaired. Retain original input objects, signed tapes,
accepted envelopes, carry anchors and full bounded predecessor history. Reward
`get_weights` is not a model-checkpoint download endpoint.

Regional relays are transport/cache only, not regional arithmetic aggregators.
An upload opportunity acknowledgment is not custody. Completion failure requires
an exact complete receipt or full original chunk coverage, authenticated healthy
witnesses and original grant/assignment lineage. The master fetches/hash-checks
original bytes before acceptance. Rescue preserves custody without backdating
late acceptance or charging the miner for proven storage failure.
Retention release waits for finality, disputes and vesting; signed extensions,
overlapping old/new keys and drain preserve existing retrieval authority.

Relay configuration is `RelayConfig` in `src/hypertrain/relay/app.py`:
`run_id`, `master_public`, `master_url`, signed `registry`/`network_manifest`,
`relay_id`, `region`, `active_key`, `signing_files`, `observer_public_keys`, plus
`s3_endpoint`/`s3_bucket`/`s3_prefix`/`s3_region` or `local_backing`.
`RELAY_CONFIG` selects its JSON; `RELAY_SECRETS` selects mounted signing files,
`drain.token` and S3 `object.json`. Do not mount coordinator/admin keys into relays.
Authoritative external backing is separate from disposable pod cache.

Operator manifests are `deploy/k8s/base` and overlays `eu`, `us`, `apac`, `local`.
Render before considering deployment:

```bash
kubectl kustomize deploy/k8s/overlays/eu
```

Deployment expects operator-created `relay-contract` ConfigMap and
`relay-runtime-v1` Secret; validate the pinned image against current source,
signed contracts, TLS, policy enforcement, backing credentials and cluster
resources first. The independent local r2 test imported the closed historical
source109 image and passed actual CNI/TLS, upload/fixture-master acceptance,
restart, replica/regional fallback, retention/GC and namespace denial; owned
cleanup and all native unit terminals are verified. Its outer caller exited1
on early READY without ownership, despite runtime PASS and a legitimate VERIFIED
cleanup barrier. The isolated caller phase fix passed two native-event cases;
it does not rewrite that failed caller or certify current product caller bytes.
The product shared-base policy now permits only gateway namespace AND pod
TCP8443 (Service443 targetPort8443), retaining443/default deny; one actual
four-overlay render regression passed. APAC actual runtime, L0 finality and
full D3/restored funded service integration remain unproven.
Deployment is
operator-managed Kustomize, not a custom reconciliation controller.

## Executable local D2 preparation

The preparation tool exists; do not recreate keys/configs manually or substitute
live source for its frozen archive. From the repository root, supply an
operator-reviewed freeze and an unused output directory through `REVIEWED_FREEZE`
and `PREPARATION_OUTPUT`; set `CPU_GENESIS_UNIX` to the approved synthetic genesis
time. These variables are inputs, not shipped authority. The base recipe is:

```bash
flock /tmp/hypertrain-network-cpu.lock nice -n 19 taskset -c 0-3 \
  env OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 PYTHONDONTWRITEBYTECODE=1 \
  uv run --frozen python scripts/network_gpu_prepare.py \
  --freeze "$REVIEWED_FREEZE" \
  --output "$PREPARATION_OUTPUT" \
  --cpu-genesis-unix "$CPU_GENESIS_UNIX"
```

This is a retained executable recipe, not permission to overwrite or rerun its
existing output. Preparation rejects an existing output, altered freeze or live
`hypertrain` modules loaded before frozen imports. New execution needs an unused
output and a freshly reviewed freeze. The archive binds 162 staged source files;
canonical qualification and operation-wrapper maps remain distinct from that
staging map. Installed dependencies are not claimed to be bundled in the archive.

Output includes `prepared.json` with
`status="LOCAL_PREPARED_NOT_LAUNCHABLE"`; `public/` contains signed manifest/full
TestGenesis, dataset/tokenizer/proofs, full initialized state/optimizer/EF/v0,
CA/server certificate chains, public configs and hash index
`public/index.json`. `private/` directories are0700; key/seed/token/TOML
files0600, excluded from the public index. Synthetic deterministic identities are
not production credentials. `cpu-service/` is a conserved CPU genesis store;
`production-service/` is empty. Join requests exist as signed inputs, but no
join/trial is accepted and no ACTIVE history is created. CPU genesis initialization
is not a training trajectory, qualification or hidden extra H execution.

Candidate preparation adds the approved input pair to the same tool. Set
`CANDIDATE_OUTPUT` to a distinct unused directory:

```bash
flock /tmp/hypertrain-network-cpu.lock nice -n 19 taskset -c 0-3 \
  env OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 PYTHONDONTWRITEBYTECODE=1 \
  uv run --frozen python scripts/network_gpu_prepare.py \
  --freeze "$REVIEWED_FREEZE" \
  --output "$CANDIDATE_OUTPUT" \
  --cpu-genesis-unix "$CPU_GENESIS_UNIX" \
  --candidate-driver 580.95.05 --candidate-driver 580.159.03
```

The recipe's `580.95.05`/`580.159.03` values are qualification **INPUT only**,
not observed drivers or paid GO. Actual FIRST4 attempt008 subsequently ran on
two hosts BOTH580.95.05 and passed same-driver bitwise qualification; this is
not evidence for the proposed cross-driver pair. Candidate changes only the
profile across copied frozen sources under new run/maps/source receipt; qualified
false and spend_authorized false remain explicit. Normal execution is
4 qualification +96 shadow +16 live =116,58 per host. Ten additional fault-trace
equivalents are contingent, giving a hard ceiling of126,63 per host, including
qualification. Unused fault capacity is not fault-proof completion or permission
for free retries. cu130/CUDA13.0/image contract does not change by supplying candidates.

Prepared public remaining-authority fields are NOT_PRESENT for observed qualification,
accepted trials/admission, current production contexts, financial/rate admission,
runtime/service-controller admission and resolved launch contract. Original
validators reject `RUNTIME_AUTHORIZATION_REQUIRED`,
`SERVICE_CONTROLLER_NOT_ADMITTED` and `QUALIFICATION_SOURCE` in their respective
unadmitted scopes. Local synthetic source/index receipts do not satisfy those
runtime predicates.

Attempt008's actual same-driver qualification and verified paid cleanup are
separate rescued evidence, not promotion of those prepared fields. The funded
116-operation continuation and live roster/service capacity remain unproven.

`build_runtime`/`configure_dispatch` and the dynamic verified-beacon context
factory are implemented, but need exact current owner receipts, verified image,
observed role/host qualification, backend-before-run bootstrap, accepted subjects
and shared cancellation/clock installation. No manufactured context plan, source
qualification row or synthetic ACTIVE may bypass them. Four same-instance
qualification executions must precede the112 normal continuation operations;
up to ten further fault equivalents remain contingent within the126 ceiling,
admitted staging/runtime/rescue/cleanup budget and original deadline.

Continuation accepts only the owner-signed reviewed record, with its rescued
same-instance qualification, exact source/profile/image subjects and all signer
identities validated before promotion. There is no raw-plan fallback. With the
operator-supplied `REVIEW_JSON`, `OWNER_SS58`, verified `BEACON` and unused
`RUN_DIRECTORY`, the existing entrypoint is:

```bash
uv run --frozen python experiments/gpu_network_v2/orchestrate.py continue \
  "$REVIEW_JSON" --owner "$OWNER_SS58" --beacon "$BEACON" --run-dir "$RUN_DIRECTORY"
```

The joined signed CLI intake/flat-store qualification/identity preflight is
approved at metadata scope only. It does not establish production bootstrap,
live TLS, actual CUDA qualification, public ACTIVE12 or current decoder service
integration. This command is not a launch authorization.

`D2-QUOTE-VERIFY.md` corrects historical evidence to two advertised595.84 offers,
54488349/machine153074 and54730345/machine24815. One-hour/80GB/30GB quoted
2.02+1.80 plus one5USD buffer is **8.82USD conditional**, not verified current
total cost: prior experiment debit/pending charges are unknown. That historical
pair does not price the580 candidates. Before execution, refresh authenticated
account, complete instances/volumes/charges and exact offers; attribute liabilities
and obtain explicit bounded GO. No such admission or provider action is credited
to preparation.

Later relevant store/source changes invalidate that reviewed execution-source
closure. The publication inventory identifies eight stale D2 source entries;
the historical candidate freeze does not cover the new bytes. Regenerate/review
effective staged archive/map, canonical qualification
map, operation-wrapper map and owner receipt subjects before launch. Preserve
old hashes/artifacts as history; do not silently patch the frozen tree or infer a
new image build from package/runtime repairs.

## Configuration and remaining activation gates

Policies and wire fields: `src/hypertrain/protocol/messages_v2.py`.
CLI/config: `src/hypertrain/miner/{cli,core}.py`. Service setup:
[operator.md](operator.md). Teacher configuration: [teacher.md](teacher.md).
Dataset R2 credentials/publication: [r2.md](r2.md); relay object credentials are
the separate `S3Credentials` contract in `src/hypertrain/data/store.py`.

The portable D3 capability is **restored-finalized-admission** for the original
OD MLM run
`587fc10ec7aeee92e235d39c9332c763e16cde05e5e13a8562204c700adce325`.
Its validator independently checks original owner manifest/TestGenesis, signed
trial challenges/proofs/screens/finalities, saved artifacts, conserved funding
and current eligibility. Original dual-signed `JoinRequest` and signed
`EscrowLock` envelope bytes are missing: this restores finalized admission
authority, not complete historical join/lock intake. It neither fabricates those
signatures nor includes later production checkpoints. Source:
[`network_authority_snapshot.py`](../scripts/network_authority_snapshot.py)
`Bundle`, `_authority`, `bootstrap`. Root must separately admit the exact bundle
and image hashes; portable authority APPROVE is not D3 execution PASS.

Activation status at this documentation correction:

| Gate | Current scope and remaining work |
| --- | --- |
| D2 GPU | FIRST4 attempt008 same-driver580.95.05 two-host bitwise CUDA qualification PASS, four rounds/eight workers; workload_allowed=false. Public CLI CPU evidence separate. Cross-driver qualification, funded116 continuation, current production admission and complete live GPU graph unproven |
| D3 Kubernetes | Independent local r2 import/source109/TLS/CNI/upload/fixture-master/failover/GC/deny runtime PASS and owned cleanup verified. Historical outer caller exit1 retained; isolated READY phase fix two native cases PASS. Product policy four-overlay rendered regression one PASS. APAC runtime, L0 finality and full D3/restored funded integration remain unproven |
| Capacity | Bounded opening/job/audit/referee/rollback/input/aggregate/final metadata guards, owned accounting, terminal collector and native CPU fallback were previously reviewed. Current source requalification is pending after type/runtime changes invalidated the old112 pins. Genuine80 H2 execution remains unrun; eligible MATCH/apply/CAS and CPU verification remain pending. Fresh CPU approval is absent and root execution admission remains pending; heavy32 DENIED |
| Remaining staging/runtime | Executable paths now exist; fresh admitted source, observed host, owner runtime/service receipts and production context subjects are still required. Metadata/local readiness is not execution admission |

These statuses follow `NETWORK-COMPLETION-MAP.md`,
`D2-DECODER-DRIVER-VERIFY.md`, `D2-CAPTURE-VERIFY.md`,
`D2-LAUNCH-PREP-VERIFY.md`, `D2-CANDIDATE-INPUT-VERIFY.md`, `D2-QUOTE-VERIFY.md`,
`CAPACITY-FINAL-INTEGRITY-VERIFY.md`, `D3-AUTHORITY-VERIFY.md`,
`D3-BUILD-ADMISSION.json` and `D3-POSTSTART-REAL-VERIFY.md` in the network evidence
directory, superseded for local runtime scope by `K8S-TRANSPORT-R2-RESULT-VERIFY.md`,
`K8S-CALLER-READY-PHASE-FIX-VERIFY.md` and
`K8S-RELAY-PRODUCT-INTEGRATION-VERIFY.md`. FIRST4 scope follows
`qualification-usd50-20261009T181257Z/NEXT30-008-RESULT-VERIFY.md`;
`MINER-PUBLIC-CLI-VERIFY.md` supplies distinct CPU-only interface evidence.

Keep heavy32 `SERVICE_RUNTIME_NOT_ENFORCED` denial until independent runtime
memory/CPU/deadline enforcement, bounded consumer reads, original-roster guards,
durable attempt charging and explicit source-bound requalification pass. Codec
approval and allocator limits cannot grant execution. FIRST4 CUDA008 and local
r2 Kubernetes transport now have scoped PASS evidence, not full network/service
integration. APAC runtime, L0 finality, full D3 and funded116 remain unproven;
live OpenRouter/R2 credentials remain deferred. Historical Phase A/B GPU results
retain only their original scope. Root's ledger is20/23 with allocation80, CPU
verification and final push/CI open; no80 execution or publication occurred in
this documentation update. Earlier D3
HostConfig false/null rejections, NEEDS-FIX
reports, frozen local image and historical595.84 quote remain chronology, not
current missing kernel/path implementation or present availability claims.
Bounded opening denial approval is specifically
`challenge/store.py:_service_boundary_v2` / `open_round_v2`, as reviewed in
`CAPACITY-INTAKE-VERIFY.md`; it does not qualify heavy execution.
