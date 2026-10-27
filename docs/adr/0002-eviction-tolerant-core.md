# ADR 0002: eviction-tolerant training core

- Status: draft for branch `v3-core-eviction`, 2026-10-26 (batch v3-core-eviction, Principal
  SN100 D13). Forge decides promotion to `v3`. The library is new code behind explicit
  interfaces: nothing in the v2 challenge imports it, and no production run, pin, master or
  emission profile changes.
- Order: founder, 2026-10-26: the network runs mostly on Spot 8x B300 hosts that can be evicted
  at any time, with no lost work and no reinstall by hand (programme §2.3-2.4). Design
  checklist: lot brief `B-eviction-tolerant-core.md`.
- Labels: **[V]** verified in code, by a test or a run, or read in a primary source, on the date
  above; **[A]** reported by a cited secondary document or by this batch's research and review
  agents and not re-read here; **[H]** hypothesis; **[?]** unknown. Repository paths are
  relative to `hypertrain/`.

## Context

### What happens today when a participant disappears (hypertrain v2, this branch)

| Phase | Today | Where |
| --- | --- | --- |
| Joined, challenge not yet fetched | After `seed + 100` rounds the challenge answers 409 `CHALLENGE_EXPIRED` and nothing changes. The admission stays `APPLIED` for good: it holds one of the 32 pending places (`PENDING_CAPACITY`), its hotkey is spent, and its coldkey stays blocked by the generic rule below [V] | `src/hypertrain/challenge/admission.py:236-240`, `:166-171`, `:158-166` |
| During a trial, past its deadline | The trial stays `OPEN` and holds one of the run's 2 trial places (`TRIAL_CAPACITY`), so two vanished miners block every newcomer. That lasts until someone requests that admission's challenge [V code, A review reproduction]. That request suspends the miner (`SUSPENDED`, reason `CHALLENGE_EXPIRED`) and marks the trial `EXPIRED` [V]. `join` refuses any coldkey with a non-`ACTIVE` admission [V]. `resume` needs `resolve_pending`, which needs a pending dispute, and an expiry has none: no route found back [V code, H no other route] | `admission.py:241-246`, `:216-228`, `:162-166`, `:1029-1062` |
| During island training | `torchrun --max-restarts=0`, no elastic rendezvous; nothing is published [V]. `schedule_audits_v2` reads every roster member's commit, and a missing one raises 409 for the whole round [V]. Nothing excludes a no-show, so the round never applies and `w+2` never opens: the run stalls. The miner itself is not penalised [V] | `src/hypertrain/miner/island_launch.py:402-415`; `src/hypertrain/challenge/store.py:6384-6388`, `:2151-2161`; `src/hypertrain/challenge/finality_v2.py:41-57` |
| After commit, before the delta | `verified_inputs_v2` needs the delta of every non-excluded member, so aggregate and finalize return 409 [V]. Missing leaves end in audit exhaustion, which excludes the miner (`INFRASTRUCTURE`) and marks the whole round `EXCLUDED`: nobody in it is paid [V] | `store.py:7896-7906`, `:6650-6672` |
| During an audit | The miner is not needed: the audit replays master-staged inputs [A research of `store.py:6897-7336`] | |
| During a dispute | The miner is the first expected party, with a turn deadline [V]. A silent miner loses with `PARTY_TIMEOUT` [V], an accepted rollback exclusion, so it loses the round's reward [V]. Its contest lock is slashed only for `FRAUD` [V]: an evicted honest miner loses the round, not its stake | `src/hypertrain/challenge/disputes_v2.py:332-334`, `:759-774`, `:833-838`; `src/hypertrain/aggregator/rollback_v2.py:116`; `store.py:9243-9252` |
| Finalize and `get_weights` | Finalize is coordinator-only and splits the round reward by tokens over the included works [V]. `get_weights` answers once per epoch, and that first answer is final [V]. A stalled round never finalizes, so its reward is never issued [H, from the stall above] | `store.py:8333-8345`; `src/hypertrain/ledger/ledger.py:490-498`; `src/hypertrain/challenge/app.py:918-939` |
| Reassignment | None. The sample cursor advances by the whole roster, so a vanished miner's samples are skipped, not reassigned [V] | `store.py:5181-5190` |
| Checkpoints | Written every J steps (`src/hypertrain/trainer/island.py:1144-1147`), on rank 0 only [V]. They serve audits and disputes, never a resume: a restart retrains from step 0 unless `published/` exists [A research of `island_launch.py:586-639`] | `src/hypertrain/miner/island_worker.py:188-197` |
| Spot notice | A shell watchdog polls IMDSv2 and stops the run [V]; no leave message exists | `scripts/b300/run_host.sh:96-105` |

Two premises of the lot brief do not match this branch [V]. There is no 297 s constant: a trial
lasts 100 beacon rounds of 3 s (`src/hypertrain/protocol/messages_v2.py:562-566`), and 297 s is
the time left when the challenge is fetched one round after its seed [H]. There is no dedicated
`CHALLENGE_EXPIRED` coldkey block: it is the suspension plus the generic coldkey rule above.

### Failure model

- AWS Spot gives a two-minute notice at `GET /latest/meta-data/spot/instance-action`
  (`{"action", "time"}`, 404 when none), "on a best effort basis". An instance can be
  interrupted before the notice appears, and a hibernate notice leaves no time at all. AWS
  recommends polling every 5 s. IMDSv2 needs a session from `PUT /latest/api/token`, whose hop
  limit of 1 blocks it inside a bridged container [V, AWS docs read 2026-10-26].
- Other providers: GCP Spot 0 s by default or 120 s, Azure Spot 30 s, CoreWeave up to 7 min,
  Nebius a SIGTERM 60 s before the stop [V, provider docs read 2026-10-26].
- Faults kept: notice then kill; hard kill without notice; partial network loss (cut from both
  the coordinator and the store, or from the store only); whole-host loss (8 GPUs and their
  local NVMe together); correlated regional loss; capacity drought. Measured Spot traces are
  bursty and zone-correlated (Bamboo, NSDI'23) [V].
- Rates: the AWS Spot advisor feed lists `p6-b300.48xlarge` only in us-west-2 at "<5%" per
  trailing month, and the feed is dated 2026-03-20 [V]. Pluralis-8B absorbed 607 departures
  with about 60 nodes over 40 days, about 0.01 departure per node-hour (computed from arXiv
  2607.13332) [V]. Our own B300 rate is unknown [?] and must be measured (lot D).
- Invariants:
  - I1: no accepted work is lost.
  - I2: no step is credited twice.
  - I3: every accepted result is verifiable: a link replays from a stored parent state.
  - I4: wasted GPU time is bounded: one checkpoint interval per departure, plus the drain.
  - I5: an honest departure is never penalised.

## Decision

Package `hypertrain.elastic` (`src/hypertrain/elastic/`), integer milliseconds on an injected
`Clock` throughout.

1. **Membership and trust** (`membership.py`, `coordinator.py`). A member is one host slot of a
   miner; an 8-GPU host is one unit of membership. A lease is live while `now < expires_ms` on
   the coordinator's clock only, so host clock skew never decides an expiry; a heartbeat renews
   it (simulator: 5 s heartbeat, 20 or 60 s TTL).
   - Every grant takes a token from one counter that only grows. A rejoin under the same id
     *replaces* the holder: the old token is fenced, and its shards move to the new token. The
     replacement restarts the member's progress window once per link, not at every rejoin.
   - The counter's mark is saved before the lease is returned (`tokens.py`: the `TokenLog`
     protocol, with `LocalTokenLog`, an atomic local file, and `MemoryTokenLog` for tests; the
     production log is a row of the coordinator's database, where ADR 0003 keeps lease state,
     written by the wiring to the same single-writer contract and not built here), so a
     restarted coordinator continues above every token ever delivered, and a token whose save
     failed is not reissued by the process that spent it (after a restart the number can come
     again only if its save never became durable, and then it was never delivered). A signed
     leave therefore names one lease for ever and cannot release a later one, and a host of the
     old process is fenced [V
     `test_a_signed_leave_cannot_be_replayed_after_a_restart`]. The log fails rather than
     guesses:
     - a missing or unreadable log is an error; a new run makes its own with
       `LocalTokenLog.create`, which refuses a path that holds a log, and an operator who
       restores a lost one removes the old file and gives the new one a mark above every token
       ever delivered, with a margin: the library keeps no other record of it, and a mark at
       `MAX_TOKEN` can never grant;
     - the file is locked for as long as a coordinator holds it, so a second coordinator, in
       this process or another, fails to start (`flock`, local filesystems with hard links
       only) [V `test_a_second_coordinator_does_not_start_on_a_token_log_that_is_held`,
       `test_a_coordinator_in_another_process_is_refused_at_once_by_a_held_log`]. Only the
       process that took the lock saves: a forked copy shares the descriptor, and so the lock,
       and a lock file that was removed or replaced excludes nobody, so both refuse [V
       `test_a_forked_copy_of_a_coordinator_cannot_save`,
       `test_a_log_whose_lock_file_is_gone_refuses_to_save`];
     - a save refuses a token that does not follow the mark the log last saw, which catches a
       second writer, or a store rolled back, while this coordinator runs. It detects a write
       that has happened; it is not mutual exclusion, which is the lock's work. A log restored
       from an older copy before a restart reopens the hole.

     The library persists nothing else: a restarted coordinator has forgotten its heads,
     credits, revocations and member owners, and nothing in the API restores them yet; the
     wiring rebuilds them, and every host joins again.
   - An id belongs to one owner, and a revoked id never rejoins, within one process: the
     wiring persists owners and revocations.
   - `join` admits a host key only if the injected `admit(member, owner, signer)` authority
     agrees. In production that is a server key the owner's coldkey granted for this run
     (`protocol.server_keys.SignedGrant`).
   - Tokens fence; they are not credentials. The caller must authenticate every heartbeat and
     commit as signed by the member's registered key, as the v2 envelopes are. The leave is
     verified inside the library because it is kept as evidence.
2. **Shard assignment** (`assignment.py`, `coordinator.py`). A round is a set of work units
   (shard, base state, inner steps). `plan(seed, pending, free)` gives each pending shard, in
   ascending order, to the member with the highest rendezvous score
   `sha256("ht-elastic-assign-v1|" || seed || shard || member)` among members with free
   capacity (default 1 shard per host). It is a pure function of an ordered event log, so an
   auditor can recompute it. Only pending shards are planned: a departure moves only the
   departed member's shards, and a live holder is never displaced. **Progress deadline**: a
   member holding work loses its lease (exit `STALLED`, not penalised) when it makes no
   accepted link within `progress_ms`. The window is counted per member, from its last
   accepted link or from when it started holding work, and a new shard does not reopen it. Its
   shards are reassigned and the member may join again. A sweep lapses every late member before
   it plans the freed shards. A host that heartbeats but cannot upload, or restarts in a loop,
   therefore never stalls a round; a crash loop loses its shard within about two windows.
3. **Checkpoints** (`checkpoint.py`, `store.py`).
   - What: the full state a shard resumes from. For hypertrain that is the `pack_state` blob of
     theta, m and v in fp32 plus the step (`src/hypertrain/auditor/replay.py:142-149`), so
     about 12 bytes per parameter: 12 GB for 1 B parameters, 96 GB for 8 B [V format, H
     sizes].
   - Format: a content-addressed state object, and a content-addressed manifest
     `ht-elastic-checkpoint-v1\n || JCS{version, run_id, policy, round, shard, base, parent,
     step, state, producer, lease}`. A manifest is written only after its state exists.
   - Where: the `CheckpointStore` protocol (`put`, `get`, `exists`). Local filesystem now;
     next, the R2 store of v3-dataplane (`hypertrain.storage.cas.R2ObjectStore`, unmerged
     branch [A], ADR 0003). Nothing in the store is mutable: the chain head lives in the
     coordinator, so R2 never needs a compare-and-swap or more than one write per key.
     `exists` means "a put of this key completed"; only `get` proves the bytes, so the store's
     root belongs to the store alone.
   - Who reads: the coordinator (manifests only), the next holder and the auditor.
   - Cadence: every `ckpt_every` inner steps and at the end of the unit, plus a flush on
     notice.
   - The local store fsyncs the bytes under a unique temporary name, renames, then fsyncs the
     directory. It also fsyncs an object it finds intact, and the root after creating a fan-out
     directory. `data.store.LocalFSStore` does neither and stays as it is for v2.
4. **Resume proof.** A resumer walks the chain from the head back to the round's base: every
   link must bind the same run, policy digest, round, shard and base, with a step within the
   unit and strictly decreasing towards the base. Every state hash is checked on read; a torn
   object is `CheckpointError("CORRUPT_OBJECT")`. The coordinator accepts a new head only if
   its parent is the head it holds, its step is higher, its state exists, and its producer and
   lease are the committer's (compare-and-set). The hash link is the proof that a replacement
   resumed from the right state, and each link (parent step to step) is one replayable audit
   segment.
5. **Detection and the signed leave** (`notice.py`, `drain.py`).
   - `AwsSpotNotice` reads IMDSv2 and renews an expired session once, never through an HTTP
     proxy. Any other answer, and an unreachable or silent IMDS, raises `NoticeError`; the
     caller keeps working under its lease. `FakeNoticeSource` replays a schedule.
   - The miner calls `DrainHandler.check()` between inner steps and keeps heartbeating while it
     drains. If the time left covers the last measured flush plus a margin, it flushes a
     checkpoint at the current step, then sends the leave; otherwise it leaves at once.
   - The leave goes out even when the flush fails. A failed leave is retried alone, with the
     notice and the flush decision already taken.
   - The leave is `JCS(LeaveNotice{version, run_id, member, signer, lease, reason,
     provider_time, heads})` under domain `hypertrain/elastic-leave/1|`, signed by the host key
     the member registered at join (`protocol.server_keys.check_sig`). `heads` (at most 1,024)
     and `provider_time` are evidence: the verified notice is kept on the member's `NOTICE`
     departure and never trusted over the coordinator's own heads.
   - A host without a notice is detected by lease expiry or by the progress deadline.
6. **Coordinator and settlement** (`coordinator.py`).
   - Every mutating entry point (`open_round`, `join`, `heartbeat`, `leave`, `tick`, `commit`)
     and `settle` first lapses expired leases and stalled holders. `holder` and `links` are pure
     reads.
   - A shard has at most one holder, a (member, token) pair.
   - Retries are idempotent. A commit of a link its member had accepted is answered again
     without new credit, before any lease check and across round boundaries, and a leave
     already accepted under the same lease is answered with its departure. A retry after a lost
     answer is therefore safe.
   - Credit is the steps each accepted link adds, paid to the owner of the member that produced
     it, so an evicted miner keeps all it had accepted.
   - Only a link that fails the audit penalises its producer, whose member is revoked.
     Departures (`NOTICE`, `LAPSE`, `STALLED`, `REPLACED`) are never penalised.
   - A round opens only once every shard of the previous one is complete. `open_round` checks
     each unit with the manifest's own types before changing any state, and returns the closed
     round's final settlement, departures up to that call included.
7. **Missed trial or eviction; admission and settlement (proposal P1, opt-in v3 flag).** No v2
   path changes in this batch. P1 adds a field to the v3 run policy, so it is bound into the
   run's policy digest (`WorkUnit.policy`); changing it means a new run, and v2 manifests cannot
   carry it:

   ```text
   elastic: ElasticPolicyV3 {
     version: 1, ttl_ms, progress_ms, ckpt_every_steps, capacity (shards per host),
     spares, notice_margin_ms, assign_seed: "drand-round-of-open"
   }
   ```

   Under P1:
   - A round is a set of S shards held under leases, not a fixed roster that must all commit.
     Roster sizing: the run policy fixes S, and any M ≥ 1 live members make progress. At the
     default capacity of 1, ⌈S/M⌉ waves are needed and members beyond S are hot spares. With
     capacity c > 1, as few as ⌈S/c⌉ members may hold every shard while the rest stay idle, so
     c > 1 pays only if a miner trains its shards in parallel. The quorum is "every shard
     complete, whoever completes it", so a no-show never stalls a round.
   - An eviction is a verified leave under a live lease, a lapse (hard kill, host loss,
     partition: indistinguishable from honest failure) or a stall. None is penalised, and
     accepted links are credited. A held shard that never progressed earns nothing. Only a link
     that fails the audit is penalised.
   - A trial becomes one work unit under the same lease and progress deadline, replacing v2's
     single 100-round (about 297 to 300 s) window. A trial interrupted by an eviction ends
     `INTERRUPTED`, not `EXPIRED`: its trial place is released, there is no suspension and no
     coldkey block, and the miner may ask for a new trial within a retry budget [founder]. An
     `APPLIED` row whose challenge was never fetched expires with that window instead of
     holding a pending place and blocking its coldkey for the rest of the run.
   - The audit replays links, which maps to today's anchored replay; disputes bisect inside a
     link (step, layer, op) as today.
   - A dispute turn belongs to the owner, not to the host. The owner's replacement host answers
     it from the store (published artifacts and the link chain), so the turn deadline must
     exceed the replacement delay. An honest miner evicted mid-turn otherwise loses the round by
     `PARTY_TIMEOUT`, as today. Whether a verified leave extends the open turn by one
     replacement window is a founder decision [H proposal].
   - Partial participation is credited per accepted step. How the v2 per-miner cap (Q/4,
     hence at least 4 verified miners) applies is a founder decision.
   - When P1 is wired into a challenge path, the modules that decide acceptance (`coordinator`,
     `checkpoint`, `membership`, `assignment`) join that path's `implementation_hash` set.
     That is a v3 protocol-version bump; v2 runs, golden roots and the v2 hash do not move.
8. **Not changed** [V]: `src/hypertrain/protocol/`, the golden leaf roots, the ten
   `implementation_hash` files and the store's hashed methods, the v2 admission, round, audit,
   dispute and escrow code, and the existing CI shards (`tests/elastic/` runs in its own job).

## Redundancy options

Production example [H]: N = 16 hosts of 8 GPUs, λ = 0.01 departure per host-hour
(Pluralis-like), checkpoint interval I = 10 min, synchronous checkpoint C = 30 s, replacement
delay P = 10 min, restore R = 1 min.

| Option | GPU-hour overhead | Example | Tolerates | Same final state as no eviction |
| --- | --- | --- | --- | --- |
| Checkpoint + reassignment (chosen default) | C/I + λ·I/2 (hard kills) + λ·(N−1)·P (fleet idle at the round barrier while an orphan waits) | 5 % + 0.08 % + 2.5 % ≈ 7.6 % (about 3 % with asynchronous upload) | any rate; delay grows with P | yes |
| + k hot spare hosts | k/N + C/I + λ·(N−1)·R | k = 1: 6.25 % + 5 % + 0.25 % ≈ 11.5 % | k departures per replacement window with no barrier wait | yes |
| Speculative duplicate (each shard on 2 hosts) | +100 % + C/I | ≈ 105 % | 1 departure per shard with no delay; doubles as a bitwise cross-check | yes |
| k-of-n shards (n − k extra shards, first k count) | (n − k)/k | 2 of 16: 12.5 % | n − k late shards per round | no: which shards count depends on timing |
| Peer replicas of checkpoints | network only | 0 GPU % | object store outage | yes, but peers are evictable too |

Drain budget [H]: at an assumed 10 Gbit/s to R2, a 12 GB state flushes in about 10 s and a
96 GB state in about 77 s. Add one poll (5 s) and one inner step, and the 96 GB flush only
just fits a two-minute AWS notice and never a 30 s one. The interval checkpoint then bounds the
loss, so I must be chosen from the real state size, R2 throughput (unmeasured [?]) and the
measured eviction rate.

The simulator measures the same trade-offs on a toy configuration [V sim]: 4 shards of 60
steps x 10 s, 2 rounds, 100 seeds each, ideal compute 10.67 GPU-h. Regenerate with
`sim aggregate --seeds 100 --hosts H` or `--ckpt-every K --scenarios ...`.

| Configuration | Scenario | GPU-h | Overhead vs ideal | Makespan min | Wasted steps |
| --- | --- | --- | --- | --- | --- |
| 4 hosts (no spare), checkpoint every 6 steps | none | 11.51 | +8 % | 21.6 | 0.0 |
| same | loss | 13.38 | +25 % | 26.3 | 2.9 |
| same | churn | 15.28 | +43 % | 31.2 | 12.2 |
| 5 hosts (1 hot spare) | loss | 14.27 | +34 % | 22.3 | 2.4 |
| same | churn | 16.02 | +50 % | 26.2 | 11.2 |
| 6 hosts (2 hot spares) | churn | 18.36 | +72 % | 24.8 | 9.6 |
| no spare, checkpoint every step | churn | 18.48 | +73 % | 37.3 | 2.0 |
| no spare, every 3 steps | churn | 15.58 | +46 % | 31.8 | 6.3 |
| no spare, every 12 steps | churn | 15.72 | +47 % | 32.0 | 22.9 |
| no spare, every 30 steps | churn | 17.51 | +64 % | 35.4 | 48.0 |

Decision: checkpoint + reassignment by default. Hot spares are a run-policy parameter, chosen
from the measured rate: they buy makespan, not GPU-hours. Speculative duplicates are reserved
for verification needs. k-of-n is rejected for bitwise-verifiable pretraining.

## Evidence

### Simulator

`python -m hypertrain.elastic.sim` drives the real coordinator, checkpoint manager, drain
handler and fake notice source on one fake clock. The workload is a toy deterministic one: a
sha256 chain per shard and round, and an outer step over the shard finals. It runs these
seeded schedules:

- `notice`: a notice, then a kill 120 s later;
- `short-notice`: a notice 1-3 s before the kill;
- `kill`: a hard kill, then a restart on the same host;
- `loss`: the host is gone and a replacement arrives after 300 s;
- `partition`: the host is cut off from the coordinator and the store;
- `store-partition`: the host is cut off from the store only;
- `region-notice` and `region-loss`: every host of one region at once;
- `drought`: a regional loss, then no capacity for 30 min;
- `churn`: 2-8 mixed faults.

The reference miner loop joins, heartbeats (also while it drains), restores, trains,
checkpoints every `ckpt_every` steps, commits and retries a lost answer.

Checks of the core's observable behaviour. They catch the membership, assignment,
progress-deadline and retry mutants of the list below. The store, notice and signature,
admission, compare-and-set and chain-splice mutants are caught only by their unit tests: the
simulator uses an in-memory store, a fake notice source and admitted, honest hosts. The checks:

- no shard leaves a live holder;
- no lease above its capacity;
- a dead host's lease is released within the TTL;
- no holder is stalled without a fault (partition, kill, loss or eviction) in its window;
- no live work is refused or moved (`refused`, `displaced`);
- every accepted link is in the final chain;
- each step is credited once;
- no forged link is accepted;
- the final state equals the uninterrupted run's;
- no honest owner is penalised.

Two checks are properties of the reference miner's cadence and hold for any correct core:
every loss stays within one checkpoint interval and is accounted for, and a drain on a
reachable store lasts at most one flush.

200 seeds per scenario, default configuration (4 hosts x 8 GPUs, 4 shards, 60 steps x 10 s,
2 rounds, checkpoint every 6 steps, lease 20 s, notice 120 s) [V, run 2026-10-26]. Command:
`sim aggregate --seeds 200`, then `--cheater 1 --scenarios none churn`.

| Scenario | Done, final = ref | Violations | Exits notice/lapse/stalled/replaced/revoked | Penalised runs | Wasted steps mean / max | Max loss (bound 6) | Drain s | Makespan min | GPU-h (vs none) |
|---|---|---|---|---|---|---|---|---|---|
| none | 200 | 0 | 0/0/0/0/0 | 0 | 0.0 / 0 | 0 | 0.0 | 21.6 | 11.51 (+0 %) |
| notice | 200 | 0 | 198/0/0/0/0 | 0 | 0.0 / 0 | 0 | 4.0 | 27.0 | 13.82 (+20 %) |
| short-notice | 200 | 0 | 82/116/0/0/0 | 0 | 2.8 / 6 | 6 | 0.0 | 26.2 | 13.35 (+16 %) |
| kill | 200 | 0 | 0/198/0/0/0 | 0 | 2.8 / 6 | 6 | 0.0 | 22.7 | 12.04 (+5 %) |
| loss | 200 | 0 | 0/196/0/0/0 | 0 | 3.0 / 6 | 6 | 0.0 | 26.3 | 13.37 (+16 %) |
| partition | 200 | 0 | 0/196/0/0/0 | 0 | 4.2 / 6 | 6 | 0.0 | 23.0 | 12.25 (+6 %) |
| store-partition | 200 | 0 | 0/0/376/0/0 | 0 | 6.8 / 12 | 6 | 0.0 | 26.7 | 14.23 (+24 %) |
| region-notice | 200 | 0 | 398/0/0/0/0 | 0 | 0.0 / 0 | 0 | 8.2 | 26.8 | 13.10 (+14 %) |
| region-loss | 200 | 0 | 0/394/0/0/0 | 0 | 5.6 / 12 | 6 | 0.0 | 26.1 | 12.68 (+10 %) |
| drought | 200 | 0 | 0/390/0/0/0 | 0 | 5.3 / 12 | 6 | 0.0 | 33.0 | 12.20 (+6 %) |
| churn | 200 | 0 | 205/482/244/0/0 | 0 | 11.7 / 37 | 6 | 2.8 | 31.1 | 15.22 (+32 %) |
| none + cheater (slot 1) | 200 | 0 | 0/0/0/0/200 | 200, only `miner-01` | 6.0 / 6 | 6 | 0.0 | 42.0 | |
| churn + cheater (slot 1) | 200 | 0 | 159/377/174/0/200 | 200, only `miner-01` | 14.4 / 41 | 6 | 2.4 | 42.7 | |

Reading:

- A notice drain wastes no step. A hard departure wastes at most one checkpoint interval, so
  two lost hosts waste at most 12 steps.
- A store-only partition stalls the holder about twice per run, and each time its shard moves
  on.
- The GPU-hour cost of a notice is mostly the evicted host billed until its kill, plus the
  shard waiting for its replacement (no spare).
- The cheater's forged link is refused in every run and only the cheater is penalised. The
  makespan doubles because its slot gets no replacement.

The soak (`soak`) ran 8,000 seeds x 11 scenarios with 0 violations on the final code [V],
over two disjoint seed ranges. Its configurations draw at random:

- hosts, regions, shards, steps and rounds, and a capacity of 1 or 2;
- checkpoint intervals and write times of 5, 15 or 30 s;
- TTLs of 20 or 60 s and restart times;
- host lease guards that are wrong;
- 10-30 % lost acknowledgements.

### Mutation check

`scripts/check_elastic_mutants.py` applies 53 single-snippet mutants and requires each named
test to fail (pytest exit 1; a collection error does not count). The child writes no bytecode
and SIGTERM restores the file. Result: 53/53 killed [V, 2026-10-26]. Among them:

| Mutant | Caught by (`tests/elastic/`) |
| --- | --- |
| checkpoint write not atomic (written under its key) | `test_store.py::test_put_killed_before_fsync_publishes_nothing` (child killed by SIGKILL at the data fsync) |
| lease never expires | `test_membership.py::test_lease_lapses_without_a_heartbeat` |
| a holder that never progresses keeps its shard | `test_coordinator.py::test_a_holder_that_stops_progressing_loses_its_shard_unpenalised` |
| every rejoin restarts the progress window (crash loop) | `test_coordinator.py::test_a_host_that_rejoins_in_a_loop_still_loses_a_shard_it_never_advances` |
| a late member is handed shards in the sweep that lapses it | `test_coordinator.py::test_late_members_are_all_lapsed_before_their_shards_move` |
| an unadmitted key joins and replaces the live holder | `test_coordinator.py::test_only_an_admitted_key_joins_or_replaces_a_member` |
| double assignment: capacity not consumed | `test_assignment.py::test_plan_gives_each_pending_shard_to_one_member_within_its_capacity` (hypothesis) |
| double assignment: a held shard planned again | `test_coordinator.py::test_a_departure_moves_only_the_departed_members_shards` |
| head not compare-and-set | `test_coordinator.py::test_a_commit_must_extend_the_accepted_head` |
| a retry after its lease ended is refused | `test_coordinator.py::test_a_retried_commit_is_acknowledged_after_its_lease_and_round_ended` |
| retried commit credited twice | `test_coordinator.py::test_a_retried_commit_is_acknowledged_again_without_new_credit` |
| accepted work dropped at a departure | `test_coordinator.py::test_a_lapsed_lease_releases_its_shard_and_head_without_a_tick` |
| resume trusts a spliced chain | `test_checkpoint.py::test_resume_refuses_a_chain_spliced_from_another_shard` |
| forged leave signature accepted | `test_coordinator.py::test_a_forged_leave_signature_is_refused` |
| a leave from another run accepted | `test_coordinator.py::test_a_leave_signed_for_another_run_is_refused` |
| drain flushes past the deadline | `test_drain.py::test_a_late_notice_leaves_at_once_without_flushing` |
| a valid link refused once (work silently wasted) | `test_sim.py::test_every_scenario_ends_in_the_uninterrupted_state[none-0]` |
| a lapse is penalised | `test_sim.py::test_lapses_and_notices_never_penalise_an_honest_miner` |
| replacement resumes from the base, not the head | `test_sim.py::test_resuming_the_same_heads_again_and_again_changes_nothing` |

### Running it

From `hypertrain/`:

```bash
uv run --frozen pytest -q tests/elastic                   # CI job `elastic`, about 15 s
uv run --frozen python scripts/check_elastic_mutants.py   # about 45 s
uv run --frozen python -m hypertrain.elastic.sim table    # one run per scenario
uv run --frozen python -m hypertrain.elastic.sim aggregate --seeds 200   # the evidence table
uv run --frozen python -m hypertrain.elastic.sim soak --seeds 5000
# long soak, 16 processes over disjoint seed ranges:
for i in $(seq 0 15); do
  uv run --frozen python -m hypertrain.elastic.sim soak --seeds 2500 --start $((i * 2500)) &
done; wait
```

CI: job `elastic` of `hypertrain-ci.yml`. It installs no torch, runs the suite under a 5-minute
step timeout and then the mutation check, on a 4-vCPU runner.

## GPU test (lot D) and estimates

- Run the real trainer with elastic checkpoints on 2 x `p6-b300.48xlarge` Spot (D11: Jakarta,
  about 185 USD for 2 x 4 h, on the ledger through Pilote [A programme §6]).
- Inject real notices with the AWS FIS action `aws:ec2:send-spot-instance-interruptions`,
  which sends a rebalance recommendation and then the two-minute notice [V, FIS docs read
  2026-10-26], and inject hard kills too.
- Measure: notice detection latency, flush time to R2 for the real state size, restore on the
  other host, and wasted GPU-time. The final state must equal the uninterrupted run bit for bit
  (the B300 requalification of ADR 0004 comes first).
- Wire the drain into the miner, replacing the shell watchdog. The image needs an IMDSv2 hop
  limit of 2 or host networking, or the token PUT times out.
- Estimates [H]:
  - simulator and core: done in this batch (1 session);
  - P1 protocol (messages, store path, settlement): 3-4 sessions, about 2 days;
  - miner client (drain, resume from island checkpoints): 2-3 sessions, about 1.5 days;
  - auditor (link replay, sampling): 2 sessions, about 1 day;
  - docs: 1 session;
  - GPU test: 1 session plus one day of Spot time.

  The miner client and the auditor can run in parallel once P1's messages are fixed, so this
  is about 6 working days after founder approval of P1.

## Founder decisions

1. Protocol-version bump P1 (elastic v3 rounds, opt-in per run): founder and Principal SN100.
2. Reward rules for partial participation:
   - credit per accepted step to the producer's owner;
   - whether a revoked member keeps credit for its earlier audited links (the code keeps it);
   - how the v2 per-miner cap (Q/4, at least 4 verified miners) applies;
   - whether idle hot spares are paid.
3. Penalties: none for notices, lapses, stalls or replacements (proposed). Any slash or burn for
   a failed audit has on-chain effects and is founder-only.
4. Budgets:
   - a lapse or stall budget against a miner that joins, holds shards and vanishes repeatedly
     (harm today bounded by one TTL or one progress deadline per join);
   - the retry budget for `INTERRUPTED` trials.

## Consequences

- The whole elastic loop is proven on CPU without a GPU or a provider account. The real state
  size, R2 throughput and eviction rate are not [?].
- Synchronous checkpoints cost C/I of GPU time. The simulator waits for each write; an
  asynchronous upload would overlap compute but bound the loss by two intervals instead of one
  [H].
- `gpu_ops.network_qualification.sources()`
  (`src/hypertrain/gpu_ops/network_qualification.py:229`), `scripts/verify_image.sh` and
  `scripts/relay_kind_smoke.py` hash every file under `src/hypertrain`. Their frozen source
  manifests are made stale by these new files, as by any source change: regenerate them with
  the next image [V].
- Known limits [V]:
  - a put killed before its rename leaves its temporary file (`.<sha>.<uuid>.tmp`) under the
    store root until the host's owner removes it, and a token log made or saved by a process
    killed at the wrong moment leaves `.<name>.<uuid>.tmp` next to the log, a copy of an old
    mark that nothing reads;
  - no test injects a power loss, so the directory fsyncs, also those of the token log, are
    covered by review only;
  - a member killed repeatedly before it completes a link loses its shard within about two
    progress windows, by design and without penalty;
  - the coordinator's heads, credits, links, owners and revocations live in memory: only the
    token mark survives a restart, and rebuilding the rest is the wiring batch's work, for
    which the API has no restore call yet;
  - the progress deadline is a run parameter and must exceed the slowest honest link, which the
    simulator checks for its own configurations.

## Rejected alternatives

1. Synchronous all-reduce across hosts with over-provisioned peers (Templar): it needs
   over-provisioning and was never shown under preemption [A Scout benchmark].
2. Asynchronous training without a round barrier (prime-rl, INTELLECT-2, with a `max_staleness`
   bound) [A Scout benchmark; prime-rl `max_staleness` default 8, Apache-2.0, A research read
   2026-10-26]. It is the most
   eviction-tolerant pattern, but the updates applied, and their staleness, depend on arrival
   timing. The result is then not bitwise reproducible against an uninterrupted run, which
   hypertrain's audit and golden roots require. It stays the candidate for the RL workloads of
   lot E, where the verifier checks rollouts, not a replayed state.
3. k-of-n shards: which shards count depends on timing, so the result is not bitwise
   reproducible.
4. The chain head as a mutable object in R2 (`current.json`): it needs conditional writes and
   meets the one-write-per-second limit per key; the coordinator's compare-and-set replaces it.
5. Penalising lapses or stalls: a hard kill or a lost route is indistinguishable from honest
   failure, and the harm is bounded by one TTL or one progress deadline.
6. Training until the deadline, then flushing: the notice is best effort and may come late, and
   leaving at detection frees the shard for a spare about 100 s earlier.
7. Host-side wall-clock leases: clock skew would decide expiry. The coordinator's clock plus
   fencing tokens replace them.
8. Reusing `data.store.LocalFSStore` for checkpoints: no fsync, and it never rewrites a torn
   object under its key.

## Reuse and provenance

No third-party code was copied. Design ideas only, all permissively licensed:

- heartbeat, timeout and "deathrattle" from INTELLECT-1 / prime-diloco (Apache-2.0);
- expiring membership records and checkpoint catch-up from Agora (Apache-2.0) and hivemind
  (MIT);
- IMDS polling from aws-node-termination-handler (Apache-2.0).

## Sources

- Code facts: this branch at `ac4c4dec` and later, read 2026-10-26. The research and review
  agents' reproductions are kept in the batch report.
- Scout benchmark `livrables/scout/2026-10-29_benchmark-entrainement-decentralise.md` and its
  `_work/` notes (read 2026-10-26).
- AWS, read 2026-10-26:
  - <https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/spot-instance-termination-notices.html>
  - <https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/prepare-for-interruptions.html>
  - <https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configuring-instance-metadata-service.html>
  - <https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/rebalance-recommendations.html>
  - <https://aws.amazon.com/ec2/spot/instance-advisor/>
  - <https://spot-bid-advisor.s3.amazonaws.com/spot-advisor-data.json>
  - <https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>
- Other providers, read 2026-10-26:
  - GCP <https://docs.cloud.google.com/compute/docs/instances/spot>
  - Azure <https://learn.microsoft.com/en-us/azure/virtual-machines/linux/scheduled-events>
  - CoreWeave <https://docs.coreweave.com/platform/capacity-plans/spot-node-pools>
  - Nebius <https://docs.nebius.com/compute/virtual-machines/preemptible>
- Precedents, read 2026-10-26:
  - Pluralis Agora, arXiv 2607.13332
  - INTELLECT-1, arXiv 2412.01152
  - <https://github.com/PrimeIntellect-ai/prime-diloco> (`src/zeroband/comms.py`)
  - <https://github.com/PrimeIntellect-ai/prime-rl>
  - Bamboo, NSDI'23 <https://www.usenix.org/system/files/nsdi23-thorpe.pdf>
