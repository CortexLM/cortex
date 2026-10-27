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

### Changed
- CUDA network qualification records each device's compute capability with its SM count and
  accepts a device only when the pair is a row of `network_qualification.SM_COUNTS` (RTX 5090
  12.0 = 170, B300 10.3 = 148) and the count equals the signed manifest's
  `reference_spec.sm_count`. A device off the manifest count is refused with
  `qualification_sm_profile_mismatch`, a pair outside the table with
  `qualification_sm_profile_unqualified`; the challenge refuses both at bootstrap. A
  `result.json` written before capabilities were recorded still qualifies 170-SM devices only.
  See `docs/adr/0007-sm-counts-per-arch.md` at the monorepo root.

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
  tables; `Admission.settle_fraud` uses it (Lens F4 iii).
- Escrow: the funding status of an admission (`EscrowV2.locked`, behind the unauthenticated
  `GET .../admission/{hotkey}`) read every lock of the ledger and ran one unindexed sum per lock under the
  store lock: 1.08 s at 5,000 open locks, 5.4 s at 10,000, 96 s at 40,000 on the real code, enough to
  make every v2 operation answer 503 once the beacon lagged 6 s. It now reads the owner's open locks
  through a partial index (the same value, bit for bit). New admission locks must hold a tenth of the
  funding floor and are capped at 2,048 per coldkey and 4,096 in the ledger (Lens F7a).
- The beacon route no longer stops at one escrow origin that cannot mature. A shadow reward whose
  hotkey rotated after acceptance (`signer is not authorized for message role`) or whose job object
  went missing (`ObjectNotFound`) rolled back the whole round's maturity and lease expiry and made
  `POST /v1/admin/beacon` answer 409 or 500 at every later round. Each origin now matures in its own
  savepoint; one that fails for another reason than vesting or an open dispute stays `reward_pending`,
  is journaled as `records_v2 kind='maturity-failed'` and is retried at every round. A conservation
  failure or a SQLite error still aborts the call (Lens F6, SHD-D3).

### Not yet
- Quality parity with centralized training is not claimed.
