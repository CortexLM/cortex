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
- `GET /v2/runs/{run_id}/join/{admission_id}/challenge` needs the owner's signature
  (`X-Hotkey`, `X-Trial-Signature` over `hypertrain/trial-challenge/2|run|admission`, as the
  `/job` route does with its own domain), so a third party can no longer open a rival's trial
  (RTA-04). The admin export carries the signed challenge as `challenge_envelope` and
  `trial_export` no longer calls the route; the miner signs the GET. A client built before this
  change gets 403. ADR 0010.
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
  accepted resolution: a referee verdict of fraud against the miner burns the bond and the
  coldkey's unvested rewards and bans the admission. `Admission.seize_bond(admission_id,
  reason, evidence_hash, now)` burns the bond for a no-show (`reason="NO_SHOW"`), a confirmed
  round-stage audit mismatch (`"AUDIT_MISMATCH"`: also suspends with one strike) or a convicted
  miner (`"TRAINING_FRAUD"`); `Admission.release_bond` and the status key `bond` serve the
  owner. `EscrowV2.slash` takes `burn_pending` (default: today's behaviour). ADR 0010.
- `RotateRequest` needs the new hotkey's signature (`new_hot_sig`, own domain
  `hypertrain/rotate/2|`; `miner.admission.sign_rotation_consent`): one coldkey signature can no
  longer burn a registered hotkey that never joined (RTA-01). A new rotation is charged to
  `join_burst` per coldkey per `work_screen_epoch_rounds` (`ROTATION_QUOTA`); a banned admission
  cannot rotate. A client that signs the three-field body gets 409. ADR 0010.
- CUDA network qualification records each device's compute capability with its SM count and
  accepts a device only when the pair is a row of `network_qualification.SM_COUNTS` (RTX 5090
  12.0 = 170, B300 10.3 = 148) and the count equals the signed manifest's
  `reference_spec.sm_count`. A device off the manifest count is refused with
  `qualification_sm_profile_mismatch`, a pair outside the table with
  `qualification_sm_profile_unqualified`; the challenge refuses both at bootstrap. A
  `result.json` written before capabilities were recorded still qualifies 170-SM devices only.
  See `docs/adr/0007-sm-counts-per-arch.md` at the monorepo root.

### Fixed
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

### Not yet
- Quality parity with centralized training is not claimed.
