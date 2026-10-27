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
- Opt-in `reference_spec.env.pinned` (`{present: {NAME: VALUE}, absent: [NAME]}`) over the closed,
  append-only table of `hypertrain.protocol.env_pins`: process-environment variables that change
  CPU round bytes (`ATEN_CPU_CAPABILITY`, `MKL_CBWR`, `ONEDNN_MAX_CPU_ISA`, `HT_ATTN_BACKEND`,
  `OMP_DYNAMIC`, and `HT_COMPILE`, `HT_FP8`, `HT_MOE_GROUPED`, `OMP_THREAD_LIMIT` pinned unset).
  A manifest without the field keeps its bytes and `run_id`; the table is in `docs/protocol.md`
  section 7 and the decision in `docs/adr/0011` (part 3) at the monorepo root. The island and
  repair workers and the in-process entry points (`train_round`, `train_island`,
  `train_unit_order`) refuse a process whose environment, or ATen capability, diverges from the
  pin, naming the variable; a pinned rank records `env_pin` in `summary.json`.
  `python -m hypertrain.trainer.env_probe --manifest FILE` reports the environment a manifest pins
  and checks that this host can honor it (`protocol.env_pins.require_honorable`: a child started
  under the pinned environment, cached per process).
  `launch_island`, `launch_window` and `_launch_unbounded` start island ranks under the pins and
  raise `EnvPinRefused` (an `IslandFailure`) before charging capacity when this host cannot honor
  them. The rollback repair and history workers get the same pins.
  `NetworkMiner.join` and `probe_trial` refuse a host that cannot honor the pin before any request,
  and `bootstrap_service_capacity_v2` answers 409 `SERVICE_PROFILE_ENV_PIN` when the challenge host
  cannot.
  The miner's dispute follower answers a pinned CPU publication from its published trace instead
  of replaying it in the daemon (behavior change for pinned manifests only).

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
  tree by `settle_fraud`; ADR 0011 Part 8 calls the other two from the exit sweeper);
  `Admission.release_bond` and the status key `bond` serve the owner. `EscrowV2.slash` takes
  `burn_pending` (default: today's behaviour). ADR 0010.
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
- `Admission.seize_bond(reason="NO_SHOW")` adds one strike and burns the bond. The first no-show
  puts the admission back on a fresh probation (state reason `NO_SHOW`, new trials, no
  suspension); a second one for other evidence inside `NO_SHOW_STRIKE_WINDOW_SCREENS` (4) work
  screens suspends it, as `"AUDIT_MISMATCH"` does at once (D63). A banned admission stays banned,
  unvested rewards are spared and the coldkey stays bound as in probation. The exit reaper of the
  beacon route calls it. ADR 0010 Part 6, ADR 0011 Part 8 decision 12.
- A roster member left out for its silence is sanctioned in the same beacon push: its bond is
  burned and it gets one strike (a probation the first time, a suspension the second time inside
  the window, D63), through the new row `exit-sanction` (`PENDING`, `APPLIED` or `WAIVED`, id
  `<w>:<hotkey>`) that a sweeper applies and retries, so that a failing sanction never keeps the
  member in the round. The blame is the miner's only when the window it missed was served and
  somebody used it: the master records `beacon-pause` (once per pause) when its wall clock was
  more than two rounds ahead of the last round it held, and a silence in a round where nobody
  committed, or a missing delta in a round nobody delivered to, is waived. A NO_SHOW that a lazy
  writer recorded is adopted on the next push. `ChallengeStore.sanction_hook` (None: the run's
  `Admission`) is the seam a test binds. A suspension has no route back. See
  `docs/adr/0011-audit-integrity.md` (Part 8) at the monorepo root.

### Fixed
- A member convicted of training fraud between the aggregate and the finalize of a round is no
  longer paid for it, nor is any other admission of its coldkey (D64): `finalize` leaves out an
  input whose coldkey has a `BANNED` admission (read through the admission's hotkey history, a
  rotation changes the hotkey), the coordinator-signed `Finalize.included` lists the others, their
  tokens split the whole round budget, and the ledger refuses a reward for a convicted coldkey
  (`BANNED_WORK`). A round whose inputs all belong to convicted coldkeys settles with an empty
  `Finalize.included` and answers `CLOSED_UNPAID`: nothing is minted, and the ledger records the
  round as withdrawn when the next one is finalized, so a convicted round no longer blocks the
  reward order of the rounds after it. The service-admitted path compares `included` with the
  roster less the convicted coldkeys instead of the whole roster.
  `scripts/ops/live_operator.py finalize` lists the same set. A member that is only suspended or
  has a burned bond is still paid for the round it was verified in. See
  `docs/adr/0011-audit-integrity.md` (Part 8, decision 23).
- The dispute event poll is a request signed in its body, as the other owner routes: `POST
  /v2/runs/{run_id}/disputes` takes the party's `WatchRequest{party, cursor, wait_seconds}` envelope
  and `GET` with `X-Dispute-Signature` answers 405 (D51: the master proxy forwards no signature
  header). The miner's watch and follow clients sign it; a client built before this change gets 405.
  See `docs/network-v2.md`.
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
- A silent roster member is left out at the bound of its wait, no longer when somebody next calls
  the round: the first beacon push at or after `d_commit` records `excluded` `NO_SHOW` (stage
  `commit`) for every member with no commit, and the first at or after `d_upload` for every audited
  member with no delta. The records are those of the lazy writers, which stay as a backstop. A round
  the rest cannot carry keeps its membership facts and gets a `round-exit` marker. The reaper skips a
  service run, and a failure of it, or of the dispute reaper, is journaled as `exit-failed` and
  stops neither the beacon push nor the other reaper. No one is sanctioned yet. See
  `docs/adr/0011-audit-integrity.md` (Part 8) at the monorepo root.
- A round the caps or the finality rules refuse is a 409, no longer a bare 500: opening a round
  with a roster the Q/4 caps cannot carry (fewer than four eligible members) or round w+2 without
  round w+1 answered `Internal Server Error`. `InsufficientEligibleWeight` now answers 409
  `ROUND_INFEASIBLE: <reason>` and `FinalityError` and `TapeError` 409 `ROUND_BLOCKED: <reason>`
  on every route of the challenge app (the two rollback routes keep their bare message for a
  `TapeError`). The store still raises the bare exceptions; nothing is written by a refused call.
  A fault of the master's own artifacts is not a request's fault and stays a 500: a stored tape
  whose signature or version fails, a replay that differs from the tape it replays, and a repair
  worker, signing authority or output artifact that fails, or a fresh independent worker that
  disagrees with the tape the coordinator signed (`TapeIntegrityError`, a subclass of `TapeError`) answer 500 `TAPE_INTEGRITY: <reason>` on every route, the two rollback routes
  included.
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
- A miner can no longer contest a verdict that upholds its work: `POST .../dispute` answers 409
  `NOTHING_TO_CONTEST` when the audit lease of the contesting miner in that round says MATCH. A
  funded contest of its own MATCH suspended the contestant (`PENDING_DISPUTE`), pinned its round
  and, once the server closed it, left the contestant on a fresh probation that is not live; the
  contest bond came back, so leaving the round that way was free. A MISMATCH is contested as
  before. The store's `contest_of` callback refuses, so
  `disputes_v2.py` is unchanged. See `docs/adr/0011-audit-integrity.md` (Part 8, decision 21) at
  the monorepo root.
- A round that cannot reach a tape can be closed as a retired round instead of blocking the run
  and the bond of every honest member for ever: the store records a null tape (the model does not
  move), `finality` `RETIRED` and the evidence, carries every roster member's anchor `w - 1` as
  its anchor `w` under new hashes (the tensor files are linked, nothing is loaded), cancels the
  open audit leases, and `audits`, `aggregate`, `finalize` and `rollback` of that round answer 409
  `ROUND_RETIRED`; no late audit result can replace a carried anchor. The causes (`INFEASIBLE`,
  `AUDIT_WINDOW_MISSED`) are re-derived from the facts, the escrow is not called (the next reward
  `finalize` records the round as withdrawn). `schedule_audits_v2`, `aggregate_v2`, `finalize_v2`
  and `rollback_v2` are owned functions: `implementation_hash` changes. See
  `docs/adr/0011-audit-integrity.md` (Part 8, decisions 7 to 11) at the monorepo root and
  `docs/network-v2.md`.
- `scripts/ops/live_operator.py open` opens the round that follows a retired one: when round
  `w - 1` has no `finalize`, `prev_final_hash` is the evidence hash of its `retired` record instead
  of the tool stopping with "missing accepted finalize" (a round that is neither is still refused).
- The relay custody of the uploads of a retired round can be settled and released: the settlement
  answered 409 "pinned by unresolved finality" for ever because it knew only a `SETTLED` round.
  Its finality hash is now the evidence hash of the retirement and its vesting ends
  `E_vest_rounds` after the retirement beacon; an open dispute still pins it.

### Not yet
- Quality parity with centralized training is not claimed.
