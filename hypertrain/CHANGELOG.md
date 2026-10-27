# Changelog

## [0.1.0] - unreleased

### Added
- Signed protocol, JSON schemas and ledger (delayed rewards, burn/clawback).
- Challenge service, trainer (deterministic rounds, island layouts), auditor, aggregator, miner client.
- drand-beacon sampled replay audits with Merkle commitments.
- CPU simulations: DiLoCo state-policy study, parity grid, mechanism tables.
- GPU determinism Phase A on RTX 5090 hosts (bitwise-identical across 3 hosts).
- `kimi-k3-pro-tiny` test architecture (MLA, shared-expert MoE, aux-loss-free routing bias),
  permanently registered; see `docs/adr/0006` at the monorepo root.
- Read-only model registry `hypertrain.models.registry` (arch -> module, per-arch rank bound and
  reduction orders), covered by `implementation_hash`. `ModelSpec.arch` names a registry entry
  (omitted = `decoder`; existing manifest bytes and run_ids unchanged); v2 manifests carry
  `kimi-k3-pro-tiny` on a one-GPU island, v1 manifests refuse it (D18, ADR 0006).
- R2 data plane (`docs/adr/0003` at the monorepo root): `storage.r2.R2Client` gains multipart,
  ListParts, conditional complete, bound presigned URLs, an opt-in `Retry` and a configurable
  SigV4 `R2Config.region`; `storage.cas`
  stores content-addressed objects in parts of whole 4 MiB blocks (8 MiB by default) with
  relay-identical block lists, resumable uploads and block-verified downloads; `storage.offer`
  hands credential-free hosts presigned UploadPart URLs bound to each part's sha256.
  `data.store.sigv4_presign` now signs the URL query and extra headers.
- Generic CUDA executor (`HYPERTRAIN_CUDA_EXECUTOR`, `hypertrain.gpu_ops.cuda_executor`): the
  challenge sends references, audit replays and referee runs to any SSH + Docker GPU host, with
  the container started by the manifest's image digest, nothing persisted on the host and a
  `role-dispatch.json` provenance the existing CUDA audit checks accept. Mutually exclusive with
  `HYPERTRAIN_ROLE_DISPATCH`; the Vast dispatcher is unchanged. See `docs/adr/0008` at the
  monorepo root and `docs/cuda-executor.md`.
- Generic CUDA executor compute cache (`compute_cache`, `hosts[].compute_cache_dir`): every
  container gets one bind mount of a persistent CUDA compute cache, one directory per image
  digest created by the executor, so only the first reference per image pays the cold CUDA start
  of a fresh `--rm` container (S13, through a wrapper: 57-64 s to 12-19 s; not yet run with this
  implementation). A host that cannot make the directory runs the job without the cache.
  `compute_cache: false` restores the previous command; `role-dispatch.json` and the journal
  record whether the mount was requested. See `docs/adr/0012` at the monorepo root and
  `docs/cuda-executor.md`.
- Executor results through R2 (`HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS`,
  `hypertrain.gpu_ops.artifact_transfer`, opt-in): the container hashes the result tar, declares the
  hashes in R2 and uploads it to presigned part URLs bound to them; the challenge verifies every
  block and the whole sha256 before the unchanged checks. The host holds URLs only. See
  `docs/adr/0013` at the monorepo root and `docs/cuda-executor.md`. `R2ObjectStore.fetch_to` takes
  an optional `check` callback.
- Docs, CI, image and release workflows, Dockerfiles.
- Eviction-tolerant core `hypertrain.elastic` (v3, not wired into the v2 challenge): leases with
  fencing tokens, saved before they are granted so that a restart never reissues one, and a
  progress deadline, joins admitted by the run's authority, deterministic shard reassignment
  (a shard a busy host never works goes to a member that holds nothing), content-addressed
  checkpoint chains checked back to the round's base, idempotent commit and leave retries
  (answered for two rounds, so the retry memory stays bounded), a required audit before any credit and
  `withdraw` of a link found invalid later, the AWS IMDSv2 spot notice, a signed leave and a
  drain that flushes only when it fits before the deadline; see `docs/adr/0002` at the monorepo
  root.
- GPU-less eviction simulator `python -m hypertrain.elastic.sim` (`table`, `aggregate`, `soak`):
  8-GPU hosts on a fake clock under seeded notice, hard-kill, host-loss, partition,
  store-partition, regional-loss and drought schedules; every run must end in the uninterrupted
  run's state hash.
- Required `reference_spec.env.pinned` (`{cpu, gpu, present, absent}`): every manifest names a CPU
  class (`avx2`, `avx512`) and a GPU class (`none`, `sm_103`, `sm_120`) from the closed, append-only
  tables of `hypertrain.protocol.env_pins` and carries the process environment they imply
  (`ATEN_CPU_CAPABILITY`, `MKL_CBWR`, `ONEDNN_MAX_CPU_ISA`, `OMP_DYNAMIC`, `HT_ATTN_BACKEND`, and
  `HT_COMPILE`, `HT_FP8`, `HT_MOE_GROUPED`, `OMP_THREAD_LIMIT` pinned unset). A CPU-only run says
  `"gpu": "none"`. A manifest without the field, with `null`, or with only one of the two parts is
  refused wherever it is parsed (`create_run` and `create_run_v2` answer 422 naming the field;
  a stored run that no longer parses answers 409 `RUN_MANIFEST_RETIRED`). The tables are in
  `docs/protocol.md` section 7 and the decision in `docs/adr/0011` (part 3) at the monorepo root.
  The island and repair workers and the in-process entry points (`train_round`, `train_island`,
  `train_unit_order`) refuse a process whose environment, or ATen capability, diverges from the
  pin, naming the variable; the island worker also refuses a device outside the GPU class and
  records what it observed in `summary.json["env"]`. `TrainConfig.env_pin` is required.
  `python -m hypertrain.trainer.env_probe --manifest FILE` reports the environment a manifest pins
  and checks that this host can honor it (`protocol.env_pins.require_honorable`: a child started
  under the pinned environment, cached per process and backend).
  `launch_island`, `launch_window` and `_launch_unbounded` start island ranks under the pin and
  raise `EnvPinRefused` (an `IslandFailure`) before charging capacity when this host cannot honor
  it. The rollback repair and history workers get the same pin.
  `NetworkMiner.join` and `probe_trial` refuse a host that cannot honor the pin before any request,
  and `bootstrap_service_capacity_v2` always asks whether the challenge host can (409
  `SERVICE_PROFILE_ENV_PIN`).
  The miner's dispute follower answers every publication from its published trace instead of
  replaying it in the daemon (the in-process replay of an unpinned CPU publication is gone).
  Breaking: every `run_id`, `reference_hash` and `training_hash` changes; existing runs are
  recreated. `kimi_tiny.train_config` and `qualify` take the `EnvPin` the caller's process honors.
  The miner declares its probed environment in bodies it signs: `JoinRequest.env` and
  `WorkScreenV2.env` (`EnvObservation`: pinned variables as seen, ATen's resolved capability,
  device class) are required, and the server answers 409 `ENV_PIN_DIVERGES: NAME, NAME` at join,
  in the proof route before the reference is paid and in `Admission.proof`, before anything is
  written (no strike, no suspension, the trial stays open). `sign_join` takes `env=`;
  `IslandArtifacts.env` is what the ranks observed; the worker writes `summary.json["env"]`.
  The CPU service profile's `implementation_hash` covers the manifest's whole pin body and the files
  `protocol/env_pins.py` and `trainer/determinism.py` (twelve files; `_service_implementation_v2`
  takes the `EnvPin`), so it differs per pin and an installed profile no longer matches.
  The capacity workers (genesis and outer step) are started under the manifest's pin
  (`ChallengeStore._capacity_env_v2`), the genesis attestation must show that same environment, and
  `python -m hypertrain.aggregator.capacity_worker` refuses a process that does not honor the pin.

### Changed
- A challenge poll after the deadline no longer suspends a miner whose proof was accepted
  (RTA-05), and the exact accepted proof and screen may be re-posted after their deadline so
  that a finalization that failed can run. See `docs/adr/0010-admission-hardening.md` at the
  monorepo root.
- An unproven challenge past its deadline is closed `EXPIRED` by the master at the next beacon
  push and no longer suspends the miner (RTA-03); a later request answers 409
  `CHALLENGE_EXPIRED`. The two trial places can no longer be held for good by identities that
  never prove. ADR 0010.
- The challenge is asked by `POST /v2/runs/{run_id}/join/{admission_id}/challenge` whose body is
  a `ChallengeRequest` envelope signed by the admission's hotkey (new message type, expiry-bound),
  replacing the unsigned `GET`: a third party can no longer open a rival's trial (RTA-04), and no
  signature travels in a header, since the master proxy forwards none (D51). The admin export
  carries the signed challenge as `challenge_envelope` and `trial_export` no longer calls the
  route; the miner signs its request. A client built before this change gets 405. ADR 0010.
- The miner's job read is `POST /v2/runs/{run_id}/join/{admission_id}/job` whose body is a
  `JobRequest` envelope signed by the admission's hotkey, replacing `GET` with the headers
  `X-Hotkey` and `X-Trial-Signature`, which the master proxy does not forward (D51 to D54). The
  service refuses an owner request (challenge or job) that expires more than 100 rounds ahead of
  its current round. A client built before this change gets 405. ADR 0010.
- At the start of every `join`, an `APPLIED` row that is past its challenge window
  (`seed + challenge_rounds`) and holds no stake is suspended as `APPLIED_EXPIRED`: 32 unfunded
  joins can no longer hold every pending place for good (RTA-02). Its coldkey may apply again;
  its hotkey stays spent; a funded row is never touched. ADR 0010.
- Optional admission policy field `max_unfunded_per_prefix` (1 to 32): `join` refuses a new
  application with `PREFIX_PENDING_CAPACITY` when its /24 or /64 already holds that many unfunded
  `APPLIED` rows (RTA-02). Absent by default and omitted from the digest, so no existing policy
  digest or run id changes. ADR 0010.
- The admission bond can be released and slashed (RTA-06). `EscrowV2` takes an optional
  `admission_settlement` and settles a `LOCK_ADMISSION` through it, by lock kind; the settlement
  is derived from the admission's records. `Admission.settle_fraud(dispute_id)` runs with every
  accepted resolution: a referee verdict of fraud against the miner burns the bond, bans the
  admission and, while it still holds a residual lock, burns the coldkey's unvested rewards.
  `Admission.seize_bond(admission_id, reason, evidence_hash, now)` is the internal hook for a
  no-show (`reason="NO_SHOW"`), a confirmed round-stage audit mismatch (`"AUDIT_MISMATCH"`: also
  suspends with one strike) or a convicted miner (`"TRAINING_FRAUD"`, the only one called in this
  tree: the audit gives a no-show no sanction, ADR 0011); `Admission.release_bond` and the
  status key `bond` serve the owner. `EscrowV2.slash` takes `burn_pending` (default: today's
  behaviour). ADR 0010.
- `RotateRequest` needs the new hotkey's signature (`new_hot_sig`, own domain
  `hypertrain/rotate/2|`; `miner.admission.sign_rotation_consent`): one coldkey signature can no
  longer burn a registered hotkey that never joined (RTA-01). A new rotation is charged to
  `join_burst` per coldkey per `work_screen_epoch_rounds` (`ROTATION_QUOTA`); a banned admission
  cannot rotate. A client that signs the three-field body gets 409. ADR 0010.
- A signed elastic leave names the coordinator epoch, drawn at random at every start and never
  kept in the token log: a log restored from an older copy, or a copy or hard link of it, can
  re-issue a token but no longer lets an old leave release the new lease (EB-01). `LeaveNotice`
  gains the required field `epoch`; `Coordinator.leave` answers `STALE_EPOCH`. ADR 0010.
- CUDA network qualification records each device's compute capability with its SM count and
  accepts a device only when the pair is a row of `network_qualification.SM_COUNTS` (RTX 5090
  12.0 = 170, B300 10.3 = 148) and the count equals the signed manifest's
  `reference_spec.sm_count`. A device off the manifest count is refused with
  `qualification_sm_profile_mismatch`, a pair outside the table with
  `qualification_sm_profile_unqualified`; the challenge refuses both at bootstrap. A
  `result.json` written before capabilities were recorded still qualifies 170-SM devices only.
  See `docs/adr/0007-sm-counts-per-arch.md` at the monorepo root.
- Relay upload quota (ADR 0014 Part 6, TRN-02): a relay counts the uploads it still holds
  (a live reservation, or custody with a pin neither released nor past its horizon) instead of
  every record it ever accepted, and has no default count cap (it was 64 for the life of the
  run, after which every miner assigned to it was refused and `/readyz` turned false).
  `relay.json` gains an optional `max_uploads`, omitted from the file when unset. `accept`
  refuses with `relay journal capacity exhausted` before the journal passes 12 MiB (the
  metadata limit is 16 MiB), and `/readyz` no longer depends on the count. `/readyz` also reports
  `journal_bytes`, `journal_budget`, `journal_fill` and `alarm` (above 80 % of the budget, with a
  warning in the log); the alarm never makes a replica unready.
- Relay release path (ADR 0014 Part 6, TRN-02): the master serves a public, read-only,
  coordinator-signed `RelaySettlementView` at `GET /v2/runs/{run_id}/relay-settlements/{grant_hash}`
  (new relay message type; exactly the grant hash, finality hash and beacon, closed dispute root and
  beacon, and vesting beacon), and a relay answers `CustodyRelease` from it. Until now `runtime()` never
  configured a settlement source, so a production relay refused every release. No credential enters the
  relay.
- Relay custody rotation (ADR 0014 Part 6, TRN-02, D53/D54): `collect()` also prunes the records
  whose custody is spent (released and past the deletion bound, or lapsed) with their nonces,
  never while a live record, an upload of the same bytes in flight or a deletion reservation
  still needs the object, and a task of the relay runs it every 5 minutes. A pin whose signed
  horizon plus the epoch grace is past, with no release and no dispute referencing it, is lapsed
  and deleted like a released one if its upload never reached a receipt; a receipted upload stays
  until the master releases it, however old, because a master that is away past the horizon cannot
  extend or release (D54, D72). Until now nothing removed a record and `collect()` had no caller.
- Unattended relay custody on the master (ADR 0014 Part 6, TRN-02, D53/D54): a task of the challenge
  app (`HYPERTRAIN_RELAY_LIFECYCLE_INTERVAL`, default 120 s, `0` off) releases every grant that is
  settled or closed without a reward and past the epoch grace, and extends the custody of a grant
  whose round has an unresolved dispute before its horizon, repeatedly, through `relay_control_v2`. No operator call is needed any
  more; the admin routes remain. `ChallengeStore.release_due_v2` is one pass.

### Fixed
- Escrow: the events the service writes (reward finalize, maturity, the payment of `get_weights`,
  the settlement of a dispute) live in a key space of their own, `srv:` plus an HMAC under a key
  derived from the coordinator secret, so a signed client event can no longer occupy an id the
  service needs and freeze finalize, the beacon route, the weights answer or a dispute (Lens F1).
  `EscrowV2.release_settlement` is the service's release; `release` is the owner's. A ledger built
  without the key is a read-only verifier. See `docs/adr/0014` at the monorepo root.
- Escrow: genesis, bootstrap, reward and shadow-reward origin ids are the HMAC of their preimage under
  the same id key instead of a sha256 of public values, so a third party who knows the run, the round,
  the hotkey and the finalize hash cannot compute one. Origins issued earlier keep their ids. Operator
  tools that build a genesis or bootstrap pass `escrow_id_key(coordinator secret)`. `finalize_v2` builds
  the reward's origin list through the ledger, so `implementation_hash` changes (service-admission
  records must be re-bootstrapped).
- Reward finalize judges the stored commit, verdicts and audit challenge at the beacon where this
  master accepted each of them, not at the finalize beacon. Honest envelopes expire at or before
  `d_final`, so any delayed finalize (an open contest, a beacon pause, an operator that waited) used
  to fail for ever with `envelope expired` and freeze the next rounds (Lens F3, mechanism M2). A
  ledger without the acceptance callback refuses to finalize. `live_operator finalize` seals its
  `Finalize` until `max(d_final, now) + 1000`.
- Disputes: a contest bond is settled by the dispute that took it, found through the lock's own
  signed dispute id, and a dispute or reward origin is found by its own id. A bond whose name was an
  older dispute's id made the new dispute unresolvable (its settlement resolved on the other row) and a
  lock named like a reward origin got that origin's settlement. `POST /dispute` refuses a bond named
  like an existing dispute or origin (`CONTEST_LOCK_ID_COLLISION`) (Lens F4).
- Admission locks are typed too: a lock named like a closed dispute or a settled reward origin was
  released with that object's settlement over `POST /escrow/release`. `EscrowV2.lock_status` asks a
  resolver of admission locks, or nothing (`UNACCEPTED_SETTLEMENT`), and never the dispute or origin
  tables (Lens F4 iii). The bond release of `Admission.release_bond` is a server event.
- Escrow: the funding status of an admission (`EscrowV2.locked`, behind the unauthenticated
  `GET .../admission/{hotkey}`) read every lock of the ledger and ran one unindexed sum per lock under the
  store lock: 1.08 s at 5,000 open locks, 5.4 s at 10,000, 96 s at 40,000 on the real code, enough to
  make every v2 operation answer 503 once the beacon lagged 6 s. It now reads the owner's open locks
  through a partial index (the same value, bit for bit). New admission locks must hold a tenth of the
  funding floor and are capped at 2,048 per coldkey and 4,096 in the ledger (Lens F7a).
- Escrow: an event hashed and stored the whole ledger state through the pure-Python canonical encoder,
  one interpreter step per character, twice, and `verify()` did it again for every stored snapshot:
  124 ms per event and 909 ms to verify 21 events at 5,000 unit rows (restart cost grew with the square
  of the ledger). They use the standard encoder, whose bytes are the same (a test compares them on
  arbitrary strings, and every stored event hash still verifies): 17 ms and 59 ms. A transfer to oneself
  is refused (`SELF_TRANSFER`, an old one replays) and a transfer adds a recipient row only for the
  origins it spends (Lens F7b).
- The beacon route no longer stops at one escrow origin that cannot mature. A shadow reward whose
  hotkey rotated after acceptance (`signer is not authorized for message role`) or whose job object
  went missing (`ObjectNotFound`) rolled back the whole round's maturity and lease expiry and made
  `POST /v1/admin/beacon` answer 409 or 500 at every later round. Each origin now matures in its own
  savepoint; one that fails for another reason than vesting or an open dispute stays `reward_pending`,
  is journaled as `records_v2 kind='maturity-failed'` and is retried at every round. A conservation
  failure or a SQLite error still aborts the call (Lens F6, SHD-D3).
- A reward follows its coldkey through a hotkey rotation (D66). The shadow reward of an admission that
  rotated no longer sticks: its custody is revalidated against the hotkey that signed it, which the
  admission held then, and the ledger resolves a retired hotkey of an admission to its coldkey when it
  validates stored evidence (the intake gate stays strict). `get_weights` pays a live reward to the
  admission's current hotkey, and pays nothing to a banned coldkey (its units stay free and the
  epoch mass burns).
- The dispute long poll can no longer deadlock the store. `DisputesV2` has one lock order, the
  store lock then a leaf condition: `events()` reads the event table outside the condition and parks
  on a counter, so a mutator that notifies inside its transaction never waits for a poller that
  waits for the store lock. A debug assertion reports an inverted acquisition. See
  `docs/adr/0011-audit-integrity.md` at the monorepo root.
- Generic CUDA executor: bytes a host writes can no longer leave `Executor.launch` as
  `RecursionError`, `MemoryError` or `OverflowError`. A probe that does not parse or exceeds a bound,
  a result whose metadata nests deeper than 8 levels and a tar whose extended header asks for more
  than 1 MiB quarantine the host and the next one serves the job; any other bad result fails the
  attempt, and a `trace.json` that the audit reader would refuse is no longer published. See
  `docs/cuda-executor.md` and `docs/adr/0011-audit-integrity.md` at the monorepo root.
- A dispute that nobody can decide no longer freezes finality and round opening for its whole
  absolute horizon. On the next beacon push the server closes it as `INFRASTRUCTURE` without a
  loser (the verdict and its exclusion stand, the contest bond returns in full, nothing is
  slashed) when it is past the horizon, paused anywhere but on its final op, or abandoned
  `T.dispute_per_level` rounds after its `turn_deadline`; the contested miner's suspension is
  lifted to a fresh probation. A failing closure is journaled as `dispute-exit-failed`. See
  `docs/adr/0011-audit-integrity.md` at the monorepo root.
- A roster member that never commits no longer stalls its round: audits are scheduled for the
  members that committed, the absent one is left out (`NO_SHOW`, no sanction), and a member that
  passed its audit but never uploads its delta is left out when the round is aggregated; a round
  the remaining members cannot carry is refused with 409 `ROUND_INFEASIBLE`. The new helpers
  join the owned set of `implementation_hash`, which changes. See
  `docs/adr/0011-audit-integrity.md` at the monorepo root.
- A round the caps or the finality rules refuse is a 409, no longer a bare 500: opening a round
  with a roster the Q/4 caps cannot carry (fewer than four eligible members) or round w+2 without
  round w+1 answered `Internal Server Error`. `InsufficientEligibleWeight` now answers 409
  `ROUND_INFEASIBLE: <reason>` and `FinalityError` and `TapeError` 409 `ROUND_BLOCKED: <reason>`
  on every route of the challenge app (the two rollback routes keep their bare message for a
  `TapeError`). The store still raises the bare exceptions; nothing is written by a refused call.
  See `docs/adr/0011-audit-integrity.md` at the monorepo root.
- After a round that finality closed without a reward (an accepted rollback, or an exclusion
  before the round was applied) no later round could be reward-finalized: the ledger accepted
  only the round after the last one it had paid, nothing wrote the row of a closed round, and
  `finalize` answered 409 `FINALIZE_ORDER` for ever (real HTTP, two-round rollback then a fully
  audited round 2). The next reward `finalize` now records each such round once, as a
  `ROUND_WITHDRAWN` event of the escrow journal in its own transaction, then pays: no table, no
  snapshot change, nothing minted or moved, and a run already stuck this way is released by its
  next `finalize`. A round that is pending, settled, awaiting its rollback or has an open audit
  or dispute still blocks. See `docs/adr/0011-audit-integrity.md` at the monorepo root.
- The ledger no longer pays a round that finality closed without a reward: its reward evidence is
  refused with `ROUND_CLOSED_UNPAID` (409), before and after the round was recorded as withdrawn.
  `finalize` already stopped a repaired or an unapplied round upstream; this is the ledger's own
  rule, so a future caller cannot mint it.
- `finalize` pays the inputs recorded with the applied tape instead of re-deriving the live set,
  so a sanction that lands between the aggregate and the finalize (a suspension, a burned bond)
  no longer answers 403 for ever. `finalize_v2` is an owned function: `implementation_hash`
  changes. See `docs/adr/0011-audit-integrity.md` (Part 8, decision 23) at the monorepo root.
- An audited roster member whose admission no longer qualifies it when the round is aggregated
  (hotkey rotated, banned, suspended, back on probation, screen ended, bond gone) no longer
  makes `aggregate` answer 403 for the whole round for ever: it is left out (`excluded`, reason
  `INELIGIBLE`, with a `cause`, no sanction) and the others are paid. A roster that the rest
  cannot carry answers 409 `ROUND_INFEASIBLE` and writes nothing. The new helpers join the owned
  set of `implementation_hash`, which changes. See `docs/adr/0011-audit-integrity.md` (Part 8,
  decision 22) at the monorepo root.
- A round opens only with roster members whose screen covers it: `open` answers 403 "roster
  screen ends before the round's audit window" for an entry whose `screen_until` is before the
  round's `d_final` (equal is accepted), and `scripts/ops/live_operator.py open` refuses the same
  entry by name. `open_round_v2` is an owned function: `implementation_hash` changes. See
  `docs/adr/0011-audit-integrity.md` (Part 8, decision 21) at the monorepo root.
- A rotation is refused with 409 `ROTATION_ROUND_OPEN` while a round that lists the admission is
  not closed (a round without a finality record counts as open). Rotated after the aggregate, a
  member made `finalize` answer 403 for ever (the reward looks the owner up by hotkey) and the
  way back is refused (`NEW_HOTKEY_HISTORY`); rotated before it, the member left the round for
  free. A retry of an applied rotation is still answered. `Admission.rounds_listing` and `open_rounds` serve `rotate` and
  `lock_settlement`. Rotate between rounds. See `docs/adr/0011-audit-integrity.md` (Part 8,
  decision 21) at the monorepo root.
- `GET /v2/runs/{run}/relay-observers/{grant}/{observer}` hands back the pending challenge until
  the observer answers it or it lapses (it used to replace it on every call, so any caller
  could invalidate the observer's response), and no longer reads the accepted reference bytes
  to do so. Its beacon pin carries the round: an exchange that straddled a beacon round used to
  end in HTTP 500 for ever (`docs/adr/0014`, Part 8).
- `PUT /v2/runs/{run}/objects/{sha256}` checks the object signature before it touches the store,
  and the store work of that route, of `GET .../objects/{sha256}` and of `POST /v2/relay-receipts`
  runs in the worker pool instead of on the event loop. A request that waits for the store lock
  (a long operation, or a stranger who only knows a registered hotkey, or an anonymous receipt)
  used to stop the whole process, `/version` included. A bad signature now answers 403 before
  404 (unknown run) or 403 (hotkey not admitted); nothing else changes (`docs/adr/0014`, Part 8).
- The dispute watch poll is `POST /v2/runs/{run}/disputes/watch` with a signed ht/2 body
  (`DisputeWatchRequest {party, cursor, wait}`, expiry at most 20 rounds ahead of the master's
  round) instead of `GET .../disputes` with an `X-Dispute-Signature` header. The header route is
  removed with no shim: the master proxy drops custom headers, the signature never expired, and
  nothing outside this repository runs on it. `HttpDisputeTransport` takes the party's key, the
  run id and a beacon reader; the miner no longer puts the static signature on its HTTP client.
  Every refused poll answers 403, as the header route did (`docs/adr/0014`, Part 8).
- Dispute watch polls run on their own thread pool and are bounded: two at a time per party and
  128 in all, a further poll answers 429 at once (`watch --follow` already backs off on 429). A
  wait used to occupy a thread of the pool every other route shares, so 32 waiting watchers
  stopped the master from answering anything else. The role lookup of the watch and ack routes
  now holds the store lock like every other use of the shared connection (`docs/adr/0014`,
  Part 8).
- The miner's trial job read is `POST /v2/runs/{run}/join/{admission}/job` with a signed ht/2
  body (`TrialJobRequest {admission_id}`, signed by the admission's hotkey, expiry at most 20
  rounds ahead of the master's round) instead of a `GET` with `X-Hotkey` and `X-Trial-Signature`
  headers. The header route is removed (405) with no shim, for the reasons of the dispute watch
  above; a refused request answers 403 as before (`docs/adr/0014`, Part 8).
- Object uploads are `POST /v2/runs/{run}/objects/{sha256}` instead of `PUT` with `X-Hotkey` and
  `X-Object-Signature` headers (removed, 405, no shim, for the reasons above). The body is the
  uploader's signed `ObjectUploadRequest {sha256, size}` envelope behind a 4-byte length, then the
  object: one stateless request, no multipart dependency, and the signed length gives the pinned
  per-object limit an early 413. The envelope expires at most 20 rounds past the master's round;
  the miner reads that round at most every 15 seconds and signs again for each retry. The master
  verifies the signature, then the uploader's admission, before it reads the object
  (`docs/adr/0014`, Part 8).
- The relay keeps an upload that reached a receipt until the master releases it, however old, and
  the master releases a round closed without a reward as well as a settled one (ADR 0014 Part 6.4,
  D54, D72): a master that is away past the custody horizon can neither extend nor release, and a
  dispute may have opened after the last extension. An upload the master holds no receipt for (a
  partial upload, or a receipt it refused with a 4xx, recorded as `ForwardRefused`) still lapses.
- `GET /readyz` of the relay answers 503 `{"ready": false, ...}` when its storage fails (it was a
  bare 500, or 503/400 with another body), with the journal fill fields `null` when the journal
  cannot be read.
- The dispute watch holds at most 64 of its 128 slots for parties that are not pinned roles, so that
  registered hotkeys cannot lock the auditors, referees and coordinator out; the retired
  `GET /v2/runs/{run}/disputes` answers 405 (it was 404).
- Escrow `STATE_RANGE` covers every numeric column of the state (`mature_at`, `reward_round`,
  `escrow_finalized.w`, `ordinal`, `trial_epoch`), not only `units` and `issued`; `-2**63` no
  longer escapes as an SQLite overflow.
- `scripts/launch_cuda_run.sh` accepts the object-upload retry of the signed POST (`_retrying("POST
  object`) as well as the older PUT in the miner bundle, through one check used twice; its bundle
  checks are tested against `miner/core.py`.
- A miner no longer dies on one lost read of a trial or job input. `probe-v2` and
  `run-v2` retry an object GET that meets a 404 like one that meets a 5xx (five retries, 1 to
  16 s, each wait jittered, as every retry of the miner now is), fail with the same error when it
  stays, and append one `kind: retry` line per object GET retry to `journal.jsonl` of the miner
  directory. The s100 TEST driver gives the exhausted `trial input object unavailable` 404 another
  pass. See `docs/network-v2.md`.
- A miner whose trial window lapsed (409 `CHALLENGE_EXPIRED`, state kept) can be given a new one:
  `POST /v2/runs/{run_id}/admin/join/{admission_id}/reseed` (admin bearer; `live_operator.py
  reseed`) writes `seed_beacon = now + 1` and nothing else, refused while its trial is open, while
  the window lives, or for a suspended, banned or disputed admission. `probe-v2` waits for it, asking
  again with the same hotkey every 10 s for 10 minutes, journals the `resume`, and exits with
  status 3 naming the route if nobody reseeds. No constant, policy field or message changes.
  See ADR 0010 decision 2 point 5 at the monorepo root.

### Not yet
- Quality parity with centralized training is not claimed.
