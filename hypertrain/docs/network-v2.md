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
inputs are fetched through the job/object endpoints. Re-requesting the same
round job (a miner restarting after a crash) returns the identical staged job;
staged inputs that differ on the server answer 409, never an overwrite.
Relative paths resolve
against the TOML directory. Environment `HYPERTRAIN_MINER_<FIELD>` overrides
matching TOML fields; unknown TOML keys reject. Other existing fields:
`data_dir`, `poll_seconds` (2.0), `chunk_bytes` (67108864),
`allow_file_upload` (false). Do not assume these v1-oriented tuning fields alter
the v2 server's deadlines or relay chunk policy.

## CPU smoke

`bash scripts/mine-smoke-v2.sh` runs this miner path offline on CPU: it starts the
real challenge app (tiny test-ledger fixture run, throwaway fixture keys) on
127.0.0.1 plus a self-signed TLS relay on localhost, then runs `join-v2`,
`escrow-lock`, four `trial_export`/`probe-v2`/`proof-v2` trials and
`run-v2 --round 0` as separate `uv run --frozen hypertrain-miner` processes. A round
needs a four-identity eligible roster, so three peer identities are graduated
in-process by the operator role (`"mode": "in-process"` in the evidence). PASS
requires every stage at rc 0 and the round-0 delta recorded; measured ~132 s,
budget `MINE_SMOKE_V2_BUDGET_SECONDS` (default 300). Evidence (stage, rc, ms) goes
to `$MINE_SMOKE_V2_EVIDENCE` or a fresh 0600 temp file; `MINE_SMOKE_V2_BREAK=<stage>`
points one of the four miner stages at a closed port to check failure detection (any
other name exits 2 before running). It proves the CLI
chain, not CUDA, audits, settlement or production funding.

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

The challenge is issued by the owner's signed request: `POST
/v2/runs/{run_id}/join/{admission_id}/challenge` with, as its body, an `ht/2` envelope of the
message `ChallengeRequest{admission_id}` signed by the admission's current hotkey
(`envelope_v2.seal(hotkey, "ChallengeRequest", run_id, {"admission_id": ...}, expiry)`). The proof
is in the body, never in a header: the master proxy is the only miner entry path and forwards
only `content-type`, `accept` and `authorization`. Opening a trial takes one of the run's two
trial places, so nobody else may ask for it: unknown run 404, unknown admission 409, a
deregistered or revoked owner 403, and any body that is not an unexpired `ChallengeRequest`
of this run, for this admission, signed by the current hotkey (empty, not JSON, another
message, run, admission or signer, a bad signature, an expired envelope, an expiry more than
100 rounds ahead of the service's current round) 403 `hotkey is not current accepted identity`.
The admin token is no substitute, and the earlier unsigned `GET` answers 405. The envelope's
expiry bounds a replay of a capture (the miner client signs 20 rounds ahead, a minute; the
service refuses more than 100, five minutes); the request is idempotent, so asking again returns
the stored challenge, and an asking never suspends anyone. Once a challenge is issued, the operator exports
the matching staged `IslandJobV1`, its inputs and the signed challenge (the export carries it as
`challenge_envelope`; the CLI does not call the route) into a new or empty folder:

```bash
uv run --frozen python -m hypertrain.challenge.trial_export \
  --api "$API" --run-id "$RUN_ID" --admission-id "$ADMISSION_ID" \
  --admin-token-file /path/to/admin-token --out trial
```

Copy `trial/` to the miner, then run:

```bash
uv run --frozen hypertrain-miner probe-v2 --config miner.toml \
  --job trial/job.json --challenge trial/challenge.json > proof.json
uv run --frozen hypertrain-miner proof-v2 --config miner.toml --proof proof.json
```

A joined miner can skip the export: `probe-v2 --config miner.toml > proof.json`
without `--job`/`--challenge` reads its admission ID (`status-v2`), POSTs its signed challenge
request (which issues the trial when none is open), then
`POST /v2/runs/{run_id}/join/{admission_id}/job`, whose body is an `ht/2` envelope of the
message `JobRequest{admission_id}` signed by the admission's current hotkey, like the challenge
request (a header carries no signature; `GET` answers 405). The route returns the admin
export's JSON only to the currently registered hotkey that owns the admission:
unknown run 404, unknown admission 409 `UNKNOWN_ADMISSION`, a deregistered or
revoked owner 403 like the challenge route, any body that is not the owner's unexpired
`JobRequest` (a `ChallengeRequest` is not one) 403 (the admin token is no substitute), then the
export's own refusals (409 `NO_CHALLENGE`, 409 `REFERENCE_TRIAL_CLOSED` once the trial is
finalized, 503 on a beacon pause). The miner checks the job, objects and signed challenge as
`trial_export` does, streams each input (one uncompressed, sha256-verified
object; about 4 bytes per parameter for a `reset` start state, more under
`carry`) into a private temporary folder under its workdir, probes, and deletes
the folder, also on failure: a re-run downloads and probes again.

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

An application that holds no stake and can no longer progress does not keep its place.
An `APPLIED` row is dead once more than `challenge_rounds` rounds have passed since its
seed (`join` sets it one round after the join): `challenge` then refuses it for ever. At
the start of every `join` the service suspends each dead row that has no locked unit and no
lock waiting in the outbox, with reason `APPLIED_EXPIRED`. The row, its transitions and its
spent hotkey stay; the pending place and the coldkey are released, so the miner may apply
again with a new hotkey. Only `APPLIED_EXPIRED` rows are ignored by the one-non-`ACTIVE`-row
rule; every other non-`ACTIVE` row still blocks its coldkey. A funded row is never touched.

The policy may also set `max_unfunded_per_prefix` (1 to 32; absent by default and then
omitted from the digest, so every earlier policy digest and run id is unchanged). `join`
then answers `PREFIX_PENDING_CAPACITY` when the /24 (IPv4) or /64 (IPv6) of the caller already
holds that many unfunded `APPLIED` rows; funded rows and later states do not count, so an
operator who funds right after joining is never refused. The prefix is kept as a tag keyed by
the coordinator's secret, not bound to the quota epoch, in the table `admission_origin`
(created on open; a database from before it has no tags and counts nothing). A run that sets
the field has a new policy digest and a new run id, and an image without the field refuses
its policy (`extra='forbid'`), so the challenge image ships first. Enable it only once the
challenge container sees real client addresses: behind a proxy every client may share one
prefix, and the cap becomes a global cap on unfunded newcomers. A first value to try is
`join_burst` (4).

Rotation. `POST /v2/runs/{run_id}/admission/{hotkey}/rotate` moves an admission to a new
hotkey. The coldkey of the admission seals the `RotateRequest` envelope, and the new hotkey
consents in the body: `new_hot_sig`, its signature of
`hypertrain/rotate/2|RotateRequest|<sha256 of JCS{coldkey,hotkey,new_hotkey,operation_id}>|<exp>|<run>`
(`miner.admission.sign_rotation_consent`, then `sign_rotation` with the same expiry). The
coldkey alone is refused (`ROTATION_CONSENT`, or 409 for the earlier three-field body), so no
one can rebind an admission onto a registered hotkey that never agreed and spend that hotkey
for good. A new rotation costs one unit of a budget of `join_burst` per coldkey and per
`work_screen_epoch_rounds` window (`ROTATION_QUOTA`; a retry of the same `operation_id` costs
nothing and answers the same admission, whatever its fresh signatures); a `BANNED` admission
cannot rotate (`TERMINAL_BANNED`); the new hotkey must be unspent (`NEW_HOTKEY_HISTORY`).
Locks, strikes and a pending dispute follow the admission. A rotation is refused with 409
`ROTATION_ROUND_OPEN` while a round that lists the admission is not closed (no audit or
dispute pending, and finalized, or excluded with its rollback complete; a round with no
finality record counts as open), because the roster names the hotkey and the old one is
refused at intake the moment the admission moves: rotate between rounds. A retry of a rotation that was applied is still
answered. There is no emergency rotation of a hotkey that a round lists.

A screen stays current for the signed policy's `work_screen_epoch_rounds` beacon
rounds after its accepted proof. A newer proof needs a new challenge. A challenge is
refused (`CHALLENGE_EXPIRED`) once more than `challenge_rounds` rounds have passed since
the miner's join or its last trial finalization. Only a suspend and resume reopens it,
and resume restarts probation at zero clean trials. No admin route calls them: `suspend`
runs when a dispute opens, and `resume` only after a nonfraud dispute settlement. The
server's own closure of a dispute nobody can decide resumes the miner by itself; every
other nonfraud settlement needs the miner's signed recovery call (see the dispute
paragraphs below). So a miner that waits while others run their trials stays eligible
until its last proof plus `work_screen_epoch_rounds`, for the rest of the run.

The renewal window does not grow with `work_screen_epoch_rounds`. A trial finalization
sets the next seed one round later, and the next trial must fit in the `challenge_rounds`
rounds after that seed: always 100, which the policy schema fixes, about 5 min at 3 s
rounds. The challenge deadline is seed + 100 whenever the challenge is requested, and the
proof must be accepted by then. A challenge left unproven past its deadline is closed
`EXPIRED` by the master at the next beacon push (or at the next challenge request, which
frees the place first). Nobody is suspended: the miner keeps its state, its clean trials and
its screen, and a later request answers 409 `CHALLENGE_EXPIRED` until a re-seed exists. A
beacon pusher that was down or slowed (an operator changed the cadence during a trial, the S13
incident) is the same case: its catch-up carries the beacon past the deadline, and the miner is
refused, not suspended. The
deadline round itself still accepts a proof. A trial whose proof was accepted is never expired
by the clock, and its finalization may follow the deadline: the miner may re-post the exact
accepted proof and screen after their deadline to run a finalization that failed (the proof
route commits the proof and the finalization separately). A different proof after the
deadline is refused. So a miner that is down for
longer than that window minus one trial's duration cannot renew its screen, whatever
`work_screen_epoch_rounds` is. At most two trials are open at once across the run (the
schema fixes `max_trial_replays` at 2). In a run longer than `work_screen_epoch_rounds`
(about 5 h at 6000), every ACTIVE miner must therefore chain its trials, each one proven
inside its window, to stay eligible. Otherwise only the suspend and resume above bring it
back, to PROBATION at zero clean trials; an expired trial changes none of that, it only frees
its place.

`manifest_v2 example` writes 6000 (about 5 h). In a serial 40-trial campaign
(3 × 12 ACTIVE + 4 PROBATION), each trial spans at most about `challenge_rounds` + 1
rounds when finalization and joins are prompt; joinbase-6 measured about 64. The first
screen must then cover 36 more trials and round 0 up to its `d_final`: 4106 rounds with
the joinbase-6 offsets, 5180 with the `od_large_manifest.py` offsets. The former 1000
(about 50 min) could not reach round 0. The cost: one screen now vouches for a miner's
hardware for about 5 h instead of 50 min. The same value is the IP-prefix quota epoch,
`now // work_screen_epoch_rounds`. The epoch is part of the prefix HMAC, so every prefix
starts a full bucket each epoch, and joins delete the `ip:%` quota rows of earlier epochs.
At 6000 both happen every 6000 rounds instead of every 1000. The value is per run: it is
part of the admission policy hash, hence of the run ID.

`manifest_v2 example --ledger-mode production` writes the same 6000. It is sized for the
TEST admission campaign above, and with it an ACTIVE miner that stops trialing is
ineligible about 5 h after its last proof, for the rest of the run. Decide how an idle
miner re-screens (a larger value or a code change) before signing a production admission
policy.

Prefix quotas use a stable coordinator-private HMAC namespace pinned atomically
in the store. Reopening the same store with the same coordinator does not reset
current prefix buckets. A populated legacy store without the namespace pin, or
a coordinator namespace mismatch, fails with HTTP 503 without resetting quotas.
No automatic migration is provided; separate operator resolution is required.

Funding uses signed `EscrowLock` via `POST /v2/runs/{run_id}/escrow/lock`, not a
CLI `--bond` flag. Test genesis has pinned nonfinancial origins; production mode
has separate reward authority/issuance bounds and no test-genesis conversion.

The bond is the `LOCK_ADMISSION` of the admission. It has its own settlement, derived from
the admission's records on every call (ADR 0010, Part 6), and `GET /v2/runs/{run_id}/admission/{hotkey}`
lists it under `bond`: per residual lock its units, whether it is `releasable`, its
`release_beacon` and the `finality_hash` and `closed_dispute_root` to sign. The owner takes it
back with the existing signed `EscrowRelease` (`POST .../escrow/release`) carrying those two
hashes. It is released only when the admission is not `BANNED` and has no pending dispute or
lock in the outbox, every round whose roster lists it is closed, none of its hotkeys has
unvested rewards, it is not live (`ACTIVE`, or `PROBATION` with four clean trials, until its
screen ends), and the last reward origin of its hotkeys has matured. The bond is burned by the
service and never through a route, in three cases: a referee verdict of fraud against the miner
(it bans the admission, `LOCK_BANNED` follows, and, while the admission still holds a residual lock,
burns the coldkey's unvested rewards with it), a round-stage audit mismatch that the caller
confirmed as the miner's (`reason="AUDIT_MISMATCH"`), and a roster member excluded for its silence
(`reason="NO_SHOW"`). In the last two the bond alone burns, one strike is added and the coldkey
stays bound as in probation; an audit mismatch suspends the admission, a first no-show puts it back
on a fresh probation (new trials, no suspension) and a second one inside four work screens suspends
it like a mismatch. An uncontested `MISMATCH`, an infrastructure
closure and a verdict against the auditor leave the bond intact. The status route reads the run's
rounds once per call for an admission that holds a bond.

Production funding is one coordinator-signed `ProductionBootstrap`, posted as an
`ht/2` envelope to `POST /v2/runs/{run_id}/admin/bootstrap` (admin bearer). The
run's `EconomicsPolicyV2.bootstrap_allocation_hash` pins its owner/units/maturity
list (`genesis_allocation_hash` over `[owner, units, mature_at]`); only
`ledger_mode="production"` may set it, and an unset field is omitted, so earlier
policy digests and run IDs are unchanged. Origins are
`ht-origin-bootstrap-v1|run|allocation_hash|index`. The ledger accepts it once per
run: the same `operation_id` and body replay the stored receipt, the same
`operation_id` with other content answers `OPERATION_CONFLICT`, and any second
bootstrap answers `PRODUCTION_BOOTSTRAP_FORBIDDEN`. It is refused when the inner
authority signature is not `reward_authority` (pinned to `coord_pubkey`), when
the envelope signer is not the coordinator, when the allocation differs from the
pin, when bootstrap plus issued and reserved shadow units would exceed
`max_total_issuance` (`TOTAL_ISSUANCE_BUDGET`; later rewards share the same cap),
when the run has no CUDA qualification (production refuses the CPU backend) and
when the latest beacon round is not BLS-verified (`UNVERIFIED_BEACON`).
Bootstrap units mature on the beacon loop and can be locked or transferred;
like genesis units they are collateral and never pay through `get_weights`.

Production eligibility also needs the run's `economic-bound` record. `POST
/v2/runs/{run_id}/admin/economic-bound` (admin bearer, no body) writes it once,
coordinator-signed, only when the pinned policy satisfies
`q_floor*s_lower*beta*(R_collectible+S_min) >= G_max` in exact ppm integers
(`challenge/trust_v2.py:economic_bound`); otherwise it answers 409 and writes
nothing. The record binds the economics policy hash; later calls return it
unchanged, and a stored record for another policy answers 409.

A test-ledger run published on the production challenge steers real emission:
as soon as its first live round finalizes and its reward matures, those units
enter `get_weights` like any production reward
(`tests/challenge/test_v2_weights_bridge.py::test_v2_matured_reward_reaches_get_weights`).
`ledger_mode="test"` does not keep a run out of on-chain weights; do not host a
test-ledger run on the production challenge unless that emission is intended.
Rewards are issued only through accepted finality, uniquely tracked by origin,
then mature after vesting/dispute conditions. Transfers preserve origin units;
fraud burns collectible exposure rather than redistributing it. Shadow work does
not influence global theta; authorized production bootstrap issuance is a
separate policy, not proof of market-backed collateral.

Matured live rewards become weight in `GET /internal/v1/get_weights`. Each epoch answer first
pays v1 entitlements, then fills the rest of its 10^6 mass with `available` units of live
reward origins whose `mature_at` is at or before the drand round of `epoch_at`. Those units
move to `paid` in the same transaction that stores the answer, so a repeated epoch request
returns the stored bytes and pays nothing again; units over the mass pay in later epochs.
Locked, disputed, pending, burned, transferred, shadow-bootstrap and genesis units are never
paid. A hotkey missing from the metagraph is dropped after payment and its units burn to UID0
(see `docs/findings/v2-weights-bridge.md`).

## Train one live round and watch disputes

Only run after the operator has opened an eligible, funded round:

```bash
uv run --frozen hypertrain-miner run-v2 --config miner.toml --round 0
```

This follows accept, authenticated inputs, exact-N launcher, one island commit,
original artifacts, regional custody, master acceptance and delta submission.
Tokens derive from assigned samples times sequence length, not self-reported GPU
count. Rerunning a round that is already `UPLOADED` exits 0 with `UPLOADED`: the
master returns its first acceptance for the same receipt without a new relay
retrieval, and answers 409 `RELAY_RECEIPT_CONFLICT` for a different receipt.
The delta submission must name the master acceptance minted for its own miner, round and grant:
the acceptance's `w` and `hotkey` must be the signer's round and hotkey and the receipt it names
must carry the delta's `grant_hash`, otherwise the master answers 422. A commit whose leaf count,
token count or assignment size differs from the manifest is also 422 with the reason.
`UPLOADED` is not audit success, settlement or payment. Unlike v1 `run`,
`run-v2` has no `--rounds`/`--start` option. Preserve the work directory; do not
claim the CLI is a complete unattended production scheduler.

A dispute party reads its events with `POST /v2/runs/{run_id}/disputes`. The body is the party's own
signed envelope `WatchRequest{party, cursor, wait_seconds}` (whole seconds, 0 to 30): the master
proxy forwards no signature header, so nothing is signed in a header. The envelope expires at most
100 rounds after the service's current round (`hypertrain-miner watch` signs 20 ahead), the signer
must be `party`, and `party` must be a pinned auditor, referee or coordinator or a subnet member;
anything else answers 403. `GET /v2/runs/{run_id}/disputes` with `X-Dispute-Signature` is gone (405):
a client built before this change must be upgraded.

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

A contest needs a verdict to contest. A miner cannot contest a verdict that upholds its work:
`POST .../dispute` answers 409 `NOTHING_TO_CONTEST`, and nothing is written, when the audit
lease of the contesting miner in the contest's round says MATCH. Only the miner itself can open
the contest of its verdict, so a MATCH contest could only suspend the contestant, pin its round
and, once the server closes the dispute, leave it on a fresh probation that is not live: a free
way out of the round, since the contest bond comes back. A MISMATCH, a false one included, is
contested as before.

A dispute that nobody can decide is closed by the server on the next verified beacon
round, not convicted. The resolution is `INFRASTRUCTURE` with no loser: the verdict and
the audit's own exclusion of the contested miner stand, the contest bond is released in
full, and nothing is slashed, struck or banned. Three cases close:

- a dispute past its absolute horizon, strictly after the last round `resolution()` still
  accepts, so a referee's signed resolution in the horizon round itself still decides it;
- a transcript paused anywhere but on its single final op, which the referee cannot score;
- an unpaused turn whose expected party has not moved `T.dispute_per_level` rounds after its
  `turn_deadline`, acknowledged or not. The operator's witnessed `timeout()` keeps exactly
  that window.

A paused final op stays with the referee until the horizon. The evidence of a closure names
its cause: `bounded-infrastructure-exclusion` (horizon), `unadjudicable-paused-transcript`
or `abandoned-turn`. Closing a dispute recounts the round's unresolved disputes to zero; the
coordinator's `finalize` for that round then settles it, and round `w+2` opens only after
that. The beacon round is the clock, so after a long beacon outage the first push closes
every expired dispute at once and the `timeout()` window is skipped for them. A closure that
fails is rolled back, journaled as a `dispute-exit-failed` record (stage and error class
only) and retried at the next beacon.

The same closure lifts the contested miner's `PENDING_DISPUTE` suspension to `PROBATION`
with zero clean trials and its strikes untouched: a fresh probation is the cost of a
contest, and an infrastructure fault never leaves an honest miner suspended. If that lift
fails it is journaled with stage `admission` and the closure stands. Any other nonfraud
settlement (a referee `MATCH`, a witnessed `timeout()` or a failed lift) is lifted by the
miner with `POST /v2/runs/<run_id>/admission/recover`: a `Receipt` envelope signed by the
miner's hotkey (a subnet member), with body `{"w": <contest round>, "commit_hash":
"<dispute_id>", "received_round": <latest beacon round>}` and an expiry of at least the
latest round. The dispute must be the exact unresolved pending one of that miner and its
settlement nonfraud, otherwise the route answers 403 "recovery reference differs from exact
current pending dispute". The result is the same fresh probation.

## Weights, finality and custody

Flat aggregation retains each miner's original delta: norm preclip, previous
outer-update CClip, then deterministic integer allocation. Q=16777216; final
weights sum Q, each miner <=4194304, total probation <=4194304, plus signed
known-owner group bounds. No post-cap renormalization. Four eligible ACTIVE
miners can carry exact quarter weights; infeasible cohorts reject. A round of
four therefore cannot lose a member. Similarity alone is not fraud evidence.
Audits replay each miner's own assignment.

A miner whose audit could not run (no auditor finished it in two windows of two
leases each) is left out of that round (`excluded`, reason `INFRASTRUCTURE`) and
the round is not excluded: the other miners aggregate and settle as after an
UNVERIFIED verdict. The round cannot be applied with fewer than four verified
inputs, and an excluded miner has no anchor for the round, so it cannot be on the
next roster. While an audit is pending the round stays `PENDING`: opening w+2,
aggregation, rollback and relay settlement wait for it.

At most two speculative rounds may be applied; opening w+2 waits for w to settle
or be excluded and repaired. Retain original input objects, signed tapes,
accepted envelopes, carry anchors and full bounded predecessor history. Reward
`get_weights` is not a model-checkpoint download endpoint.

A round that finality closes without a reward (excluded, or repaired by an accepted rollback)
is never reward-finalized and never paid, and it does not stop the rounds after it. Reward
`finalize` accepts round w only if w is the first round that is neither reward-finalized nor
withdrawn: when a lower round is closed without a reward, the same `finalize` first records it
once in the escrow journal (event `ROUND_WITHDRAWN`, with the hash that closed it), in the same
transaction, then pays w. The record moves no unit and sanctions nobody. The ledger refuses the
evidence of such a round itself with 409 `ROUND_CLOSED_UNPAID` (`finalize` already refuses a
repaired or an unapplied round before the ledger is asked). A refused `finalize` records
nothing, and a run that rolled back before this record existed is not stuck: its next
`finalize` records the closed rounds. A round that is pending, settled, awaiting its rollback,
or that has an open audit or dispute is not withdrawn and still answers the next round 409
`FINALIZE_ORDER`.

A roster member that has not committed when the commit window closes is left out of its
round by the first beacon push at or after `d_commit`: its `excluded` record gets reason
`NO_SHOW` (stage `commit`), the audits of the other members are created when `audits` is
called, and the round is aggregated and finalized without it. A member that committed and
passed its audit but uploaded no delta by `d_upload` is left out the same way by the first
push at or after `d_upload` (stage `delta`). `audits` and `aggregate` write the same records
for a member the beacon route did not see (a challenge restarted before the run was served
again). Its reward share goes to the members that remain, split by tokens as for any
exclusion. A member left out before its audit has no anchor for that round and cannot be
rostered again in this run; one left out at the delta stage keeps its anchor. The remaining
members must still carry the allocation: at least four, more when probation or owner-group
caps bind, counting `NO_SHOW`, `UNVERIFIED` and `INFRASTRUCTURE` exclusions of the round
together. Otherwise `audits` or `aggregate` answers 409 `ROUND_INFEASIBLE`, no member is
left out, a `round-exit` record names the reason (`INFEASIBLE`) and the run stays blocked
until an operator decision; there is no void round. Call `audits` once `d_audit` has passed
and before `absolute` minus one round, `absolute` being the smaller of `d_final` and
`d_audit` plus twice `lease_rounds`; a later call answers 503 for every roster and the round
cannot be audited.

The minimum roster is arithmetic: the caps of the aggregation policy are protocol constants and
a round needs four members, so a roster of R members tolerates R - 4 left out. Four tolerate
none, five tolerate one, six tolerate two. `scripts/ops/live_operator.py open --tolerate K`
(default 1) refuses a roster of fewer than 4 + K hotkeys and names the arithmetic; `--tolerate 0`
opens four on purpose. The store itself keeps the protocol minimum of four.

A member that committed and never posted its leaves has no audit: its queued job is dropped at
the first `absolute` (`excluded`, reason `NO_SHOW`, stage `leaves`), without a second window, and
the round goes on without it. It is not sanctioned: only the master lacks the leaves, and it
cannot prove that the miner never posted them.

A member left out for its silence is sanctioned in the same push, unless the master cannot
show that it could have been heard. The push books the blame (`exit-sanction`, id
`<w>:<hotkey>`) and applies it: the admission is suspended (reason `NO_SHOW`) with one
strike and its bond is burned, as for a confirmed audit mismatch. That holds for a round the
rest cannot carry too: its silent members are sanctioned although none is left out. The
blame is the miner's only if the window it missed was served and somebody used it. It is the
infrastructure's, the row is `WAIVED` and nothing happens to the admission, when the master
was refusing its callers while the window was open (`beacon-pause`: the wall clock was more
than two rounds ahead of the last round the master held), when nobody committed (commit
stage), when nobody delivered a delta, or when the master holds a relay receipt or its own
acceptance of the member's upload (delta stage: the relay or the storage lost it). A
sanction that fails is journaled (`exit-failed`) and retried on the next push; the member is
out of its round all the same. There is no route back from this suspension: a miner whose
host is evicted before `d_commit` loses its identity like a free identity does (ADR 0011
Part 8, decision 12 and Decisions for Forge 1).

A round that cannot reach a tape can be retired (ADR 0011 part 8, decisions 7 to 11) instead of
blocking the run for good. Two causes, each re-derived by the master from the facts: the members
that can still deliver (committed, not left out, and with their delta once `d_upload` has passed)
cannot carry one complete update (`INFEASIBLE`, from `d_commit`), or nobody scheduled audits and
`audits` answers 503 from `absolute` minus one round (`AUDIT_WINDOW_MISSED`). It also waits until
every earlier round is closed and is refused for a service-admitted run (`ROUND_RETIRE_REFUSED:
<reason>`, nothing written; no route asks for it). The master closes the
round as a null round: `applied` is a record of kind `null` whose output state is its input state
(the model, the outer momentum and the centre do not move, the data window is consumed), `finality`
is `RETIRED` (closed), and `retired` holds the evidence `{v: ht-round-retired-v1, run_id, w,
cause, roster_hash, survivors, theta_hash}`, whose sha256 is the evidence hash, with
`retired_beacon`. Every roster member, the ones that never committed too, gets for round `w` the
anchor it held for `w - 1` under new hashes (`ht-carried-proof/1`, `ht-carried-anchor/1`; the two
tensor files are linked and only the metadata is rewritten, `state_root` and `ef_hash` stay), so
round `w + 1` restarts from the optimizer state and the error feedback round `w` started from.
The open audit leases of the round end `exhausted` with result `ROUND_RETIRED` and their guards
are revoked; a finished verdict stays as evidence. From then on `audits`, `aggregate`, `finalize`
and `rollback` of that round answer 409 `ROUND_RETIRED`, and no late audit result can replace a
carried anchor. Nothing is minted for it and the escrow is not called at retirement: the next
reward `finalize` records the round as withdrawn, as for any round closed without a reward.
The relay custody of its uploads is settled by the retirement: the finality hash is the evidence
hash, the finality beacon is `retired_beacon` and vesting ends `E_vest_rounds` after it (an open
dispute against the round still pins the custody). The bond of every roster member is released
as soon as the round is closed.
Round `w + 1` is opened by the operator with `prev_final_hash` set to the evidence hash
(`scripts/ops/live_operator.py open` reads `retired/w` when round `w` has no `finalize`, and
still refuses by name a round that has neither). It needs four eligible members that hold an anchor for `w`, which are the members of the retired roster
(a member that was never rostered has none: 409 `missing accepted anchor`); fewer answer 409
`ROUND_INFEASIBLE`. A retirement costs every roster member a round of work and reward.

A request that the allocation caps or the finality rules refuse is a 409 whose detail starts
with a code, on every route, and writes nothing. `ROUND_INFEASIBLE: <reason>`: the members
cannot carry one complete update (opening a round with fewer than four eligible members, a
rollback that would leave fewer, an aggregation over an input set or delta it cannot weigh).
`ROUND_BLOCKED: <reason>`: finality or the tape refuses the call (opening round w+2 before
round w+1 exists or while w is unresolved, applying a round whose predecessor is not applied,
settling a round that cannot settle, a tape that fails its own checks). The two rollback routes
answer a tape error with its bare message, as before. A fault of the master's own artifacts is
never a conflict: a stored tape that fails its signature or version, a replay that differs from the
tape it replays, a repair worker, signing authority or output artifact that fails, or a fresh
independent worker that disagrees with the tape the coordinator signed, answers a 500
`TAPE_INTEGRITY: <reason>` on every route, the rollback routes included.
`finalize` pays the inputs recorded with the applied tape, whatever the members' standing is
when it runs, except a coldkey convicted since: `Finalize.included` lists the inputs less every
admission of a coldkey that has a banned one, and the ledger refuses (`BANNED_WORK`) a reward for
it. When no input is left, the Finalize lists nobody, the round settles with no reward (the answer
is `{"status": "CLOSED_UNPAID"}`) and the ledger passes over it when the next round is finalized. A member
suspended, struck or with a burned bond between the aggregate and the finalize is still paid for
the round it was verified in (the finalize used to re-derive the live set and answer 403 for
ever); a member convicted of training fraud in that window is paid nothing, and its share goes to
the others by tokens, so the whole round budget is still minted.

An audited member whose admission no longer qualifies it when the round is aggregated is left
out of the round too: `excluded`, reason `INELIGIBLE`, stage `aggregate`, with a `cause`
(`ROTATED`: its hotkey no longer names an admission; `BANNED`; `SUSPENDED`, also while a contest
is pending; `NOT_LIVE`: back on a fresh probation; `SCREEN_LAPSED`; `UNFUNDED`: its bond is gone,
burned for instance). The admission is read at the `aggregate` call, so a member that is fit
then is kept. Nobody is sanctioned for it: the cause is the member's own state or the effect of
a sanction already applied. The remaining members must still carry the allocation; with four
rostered, one unfit member makes `aggregate` answer 409 `ROUND_INFEASIBLE` and writes nothing,
and the round aggregates as soon as the member is eligible again. Call `aggregate` before
`d_final`: the later the call, the more screens have ended.

A round opens only with members whose screen covers it. `open` answers 403 "roster screen ends
before the round's audit window" for an entry whose `screen_until` is before the round's
`d_final`; an entry whose screen ends exactly at `d_final` is accepted, since a member is eligible
while `now <= screen_until`. A screen cannot be renewed by waiting, so a lapse inside the window
used to be a free way out of a round. `scripts/ops/live_operator.py open` refuses the same entry
by name before it sends anything.

Regional relays are transport/cache only, not regional arithmetic aggregators.
An upload opportunity acknowledgment is not custody. Completion failure requires
an exact complete receipt or full original chunk coverage, authenticated healthy
witnesses and original grant/assignment lineage. The master fetches/hash-checks
original bytes before acceptance. Rescue preserves custody without backdating
late acceptance or charging the miner for proven storage failure.
Multi-chunk custody without a receipt disables the relay key at once only when
the master holds the original and it proves every chunk hash. Otherwise the key
is disabled only after rescued bytes hash to `delta_hash`: the grant could not
tie those chunk hashes to the commit, so valid chunks that concatenate to the
wrong delta answer 422 and leave the relay enabled. A rescue the relay cannot
serve at all disables the key too: an HTTP error (including a backend 503), a
signed response other than `SERVED`, a bad response signature or binding, a
timeout, or streamed bytes that miss the signed, miner-committed chunk hash. Two
pinned auditors already proved the failure and the relay cannot produce its own
signed custody. The fallback answers `rescued: false, disabled: true`; the
`relay-disabled` id is `relay_id:assignment_epoch:key_id` and earlier evidence wins.
While a grant is live, `upload-grant` returns the same signed grant for the same
chunk list and 409 for another list, unless the master holds the original delta
and it proves the new list: then the honest list gets a new grant.
That grant carries `replaces`, the digest of the live grant it supersedes (an
optional `UploadGrant` field, omitted otherwise, so older grant digests are
unchanged). The relay then drops its unreceipted reservation for that grant;
it refuses the replacement when that upload has a receipt or belongs to another
miner, round or relay, and a wrong `replaces` leaves the old reservation blocking.
Upgrade order: deploy relays before the master. A master that emits `replaces`
in an `UploadGrant` (commit e3e5343 onward) is rejected by a relay built before
that commit (extra fields are forbidden). Roll relays first, then the master.
A relay whose granted chunks do not assemble to `delta_hash` answers 400
`AssemblyMismatch`, records it on the upload and skips it in later recovery.
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
