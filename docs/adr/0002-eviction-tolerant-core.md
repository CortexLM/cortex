# ADR 0002: eviction-tolerant training core

- Status: draft for branch `v3-core-eviction`, 2026-10-26 (batch v3-core-eviction, Principal
  SN100 D13). Forge decides promotion to `v3`. The library is new code behind explicit
  interfaces: nothing in the v2 challenge imports it, and no production run, pin, master or
  emission profile changes.
- Order: founder, 2026-10-26: the network runs mostly on Spot 8x B300 hosts that can be evicted
  at any time, with no lost work and no reinstall by hand (programme §2.3-2.4). Design
  checklist: lot brief `B-eviction-tolerant-core.md`.
- Labels: **[V]** verified in code, by a test or a run on the date above, **[A]** reported by a
  cited document, **[H]** hypothesis, **[?]** unknown. Paths are relative to `hypertrain/`.

## Context

### What happens today when a participant disappears (hypertrain v2, this branch)

| Phase | Today | Where |
| --- | --- | --- |
| Joined, challenge not yet fetched | After `seed + 100` rounds the challenge answers 409 `CHALLENGE_EXPIRED` and nothing changes: the admission stays `APPLIED`, the hotkey is spent and the coldkey stays blocked by the generic rule below [V] | `src/hypertrain/challenge/admission.py:236-240`, `:158-166` |
| During a trial, past its deadline | The next challenge request suspends the miner (`SUSPENDED`, reason `CHALLENGE_EXPIRED`) and marks the trial `EXPIRED` [V]. `join` refuses any coldkey that has a non-`ACTIVE` admission [V]. `resume` needs `resolve_pending`, which needs a pending dispute; none exists for an expiry, so no route found back [V code, H no other route] | `admission.py:216-228`, `:162-166`, `:1029-1062` |
| During island training | `torchrun --max-restarts=0`, no elastic rendezvous; nothing is published [V]. `schedule_audits_v2` reads the commit of every roster member and a missing one raises 409 for the whole round [V]; nothing excludes a no-show, so the round never applies and `w+2` never opens: the run stalls. The miner itself is not penalised [V] | `src/hypertrain/miner/island_launch.py:402-415`; `src/hypertrain/challenge/store.py:6384-6388`, `:2151-2161`; `challenge/finality_v2.py:41-57` |
| After commit, before the delta | `verified_inputs_v2` needs the delta of every non-excluded member: aggregate and finalize return 409 [V]. Missing leaves end in audit exhaustion, which excludes the miner (`INFRASTRUCTURE`) and marks the whole round `EXCLUDED`, so nobody in it is paid [V] | `store.py:7896-7906`, `:6650-6672` |
| During audit or finalize | The miner is not needed: the audit replays master-staged inputs and finalize is coordinator-only [A research of `store.py:6897-7336`, `:8285-8361`] | |
| Reassignment | None. The sample cursor advances by the whole roster, so a vanished miner's samples are skipped, not reassigned [V] | `store.py:5181-5190` |
| Checkpoints | Rank 0 writes one every J steps [V], used for audits and disputes, never to resume: a restart retrains from step 0 unless `published/` exists [A research of `island_launch.py:586-639`] | `src/hypertrain/miner/island_worker.py:188-197` |
| Spot notice | A shell watchdog polls IMDSv2 and stops the run [V]; no leave message exists | `scripts/b300/run_host.sh:96-105` |

Two premises of the lot brief do not match this branch [V]. There is no 297 s constant: a trial
lasts 100 beacon rounds of 3 s (`src/hypertrain/protocol/messages_v2.py:562-566`), and 297 s is
the time left when the challenge is fetched one round after its seed [H]. There is no dedicated
`CHALLENGE_EXPIRED` coldkey block: it is the suspension plus the generic coldkey rule above.

### Failure model

- AWS Spot gives a two-minute notice at `GET /latest/meta-data/spot/instance-action`
  (`{"action", "time"}`, 404 when none), "on a best effort basis"; an instance can be
  interrupted before the notice appears, and a hibernate notice leaves no time at all. AWS
  recommends polling every 5 s. IMDSv2 needs a session from `PUT /latest/api/token`, whose hop
  limit of 1 blocks it inside a bridged container [V, AWS docs read 2026-10-26].
- Other providers: GCP Spot 0 s by default or 120 s, Azure Spot 30 s, CoreWeave up to 7 min,
  Nebius a SIGTERM 60 s before the stop [V, provider docs read 2026-10-26].
- Faults kept: notice then kill, hard kill without notice, partial network loss (the host runs
  but cannot reach the coordinator or the store), whole-host loss (8 GPUs and their local NVMe
  together), correlated regional loss, capacity drought. Measured Spot traces are bursty and
  zone-correlated (Bamboo, NSDI'23) [V].
- Rates: the AWS Spot advisor feed lists `p6-b300.48xlarge` only in us-west-2 at "<5%" per
  trailing month, and the feed is dated 2026-03-20 [V]. Pluralis-8B absorbed 607 departures with
  about 60 nodes over 40 days, about 0.01 departure per node-hour [V, computed from arXiv
  2607.13332]. Our own B300 rate is unknown [?] and must be measured (lot D).
- Invariants:
  - I1: no accepted work is lost.
  - I2: no step is credited twice.
  - I3: every accepted result is verifiable: a link replays from a stored parent state.
  - I4: waste is bounded: one checkpoint interval per departure, plus the drain.
  - I5: an honest departure is never penalised.

## Decision

Package `hypertrain.elastic` (`hypertrain/src/hypertrain/elastic/`), integer milliseconds on an
injected `Clock` throughout.

1. **Membership** (`membership.py`). A member is one host slot of a miner; an 8-GPU host is one
   unit of membership. A lease is live while `now < expires_ms` on the coordinator's clock
   only, so host clock skew never decides an expiry; a heartbeat renews it (simulator: 5 s
   heartbeat, 20 s TTL). Every grant takes a token from one counter that only grows. A rejoin
   under the same id from another host *replaces* the holder: the old token is fenced and its
   shards move to the new token. An id belongs to one owner, and a revoked id never rejoins.
2. **Shard assignment** (`assignment.py`, `coordinator.py`). A round is a set of work units
   (shard, base state, inner steps). `plan(seed, pending, free)` gives each pending shard, in
   ascending order, to the member with the highest rendezvous score
   `sha256("ht-elastic-assign-v1|" || seed || shard || member)` among members with free
   capacity (default 1 shard per host). It is a pure function of an ordered event log, so an
   auditor can recompute it. Only pending shards are planned: a departure moves only the
   departed member's shards, and a live holder is never displaced.
3. **Checkpoints** (`checkpoint.py`, `store.py`). What: the full state a shard resumes from
   (for hypertrain, the `pack_state` blob of theta, m and v plus the step and the data cursor
   implied by the step [H]). Format: a content-addressed state object and a manifest
   `ht-elastic-checkpoint-v1\n || JCS{run_id, policy, round, shard, base, parent, step, state,
   producer, lease}`, also content-addressed. A manifest is written only after its state
   exists. Where: the `CheckpointStore` protocol (`put`/`get`/`exists`); local filesystem now,
   the R2 store of v3-dataplane next (ADR 0003). Nothing in the store is mutable: the chain head
   lives in the coordinator, so R2 never needs a compare-and-swap or more than one write per
   key. Who reads: the coordinator (manifests only), the next holder and the auditor. Cadence:
   every `ckpt_every` inner steps and at the end of the unit, plus a flush on notice. The local
   store fsyncs the bytes under a unique temporary name, renames, then fsyncs the directory;
   `data.store.LocalFSStore` does neither and stays as it is for v2.
4. **Resume proof.** A resumer walks the chain from the head back to the round's base: every
   link must bind the same run, policy digest, round, shard and base, with steps strictly
   decreasing, and every state hash is checked on read. The coordinator accepts a new head only
   if its parent is the head it holds, its step is higher, its state exists, and its producer
   and lease are the committer's (compare-and-set). The hash link is the proof that a
   replacement resumed from the right state, and each link (parent step to step) is one
   replayable audit segment.
5. **Detection and the signed leave** (`notice.py`, `drain.py`). `AwsSpotNotice` reads IMDSv2,
   renews an expired session once, and refuses any other answer rather than guessing.
   `FakeNoticeSource` replays a schedule. The miner calls `DrainHandler.check()` between inner
   steps. If the time left covers the last measured flush plus a margin, it flushes a
   checkpoint at the current step, then sends the leave; otherwise it leaves at once. The leave
   goes out even when the flush fails. A drain counts as done only once the leave is sent, so a
   failed leave is retried. The leave is `JCS(LeaveNotice{run_id, member, signer, lease,
   reason, provider_time, heads})` under domain `hypertrain/elastic-leave/1|`, signed by the
   server key the host registered at join (`protocol.server_keys.check_sig`). A host without a
   notice is detected by lease expiry.
6. **Coordinator and settlement** (`coordinator.py`). Every entry point first lapses expired
   leases. A shard has at most one holder, a (member, token) pair. A commit or a leave already
   accepted is answered again without new effect, so a retry after a lost answer is safe.
   Credit is the steps each accepted link adds, paid to the owner of the member that produced
   it: an evicted miner keeps all it had accepted. A link that fails the audit penalises its
   producer, whose member is revoked; departures (`NOTICE`, `LAPSE`, `REPLACED`) are never
   penalised. A round opens only once every shard of the previous one is complete.
7. **Missed trial or eviction; admission and settlement (proposal P1, opt-in v3 flag).** No v2
   path changes in this batch. Proposal P1 is a new v3 run-policy field, bound into the run's
   policy digest (`WorkUnit.policy`), so changing it is a new run; v2 manifests cannot carry it.
   Under P1:
   - A round is a set of shards held under leases, not a fixed roster that must all commit.
     The quorum is "every shard complete", whoever completes it, so a no-show no longer stalls
     the round.
   - An eviction is a verified leave under a live lease, or a lapse (hard kill, host loss,
     partition: indistinguishable from honest failure). Neither is penalised; accepted links are
     credited. A held shard that never progressed earns nothing. Only a link that fails the
     audit is penalised.
   - A trial interrupted by an eviction ends `INTERRUPTED`, not `EXPIRED`: no suspension and no
     coldkey block, and the miner may ask for a new trial within a retry budget [founder].
   - The audit replays links, which maps to today's anchored replay; disputes bisect inside a
     link (step, layer, op) as today.
   - When P1 is wired into a challenge path, the modules that decide acceptance
     (`coordinator`, `checkpoint`, `membership`, `assignment`) join that path's
     `implementation_hash` set. That is a v3 protocol-version bump; v2 runs, golden roots and
     the v2 hash do not move.
8. **Not changed** [V]: `src/hypertrain/protocol/`, the golden leaf roots, the ten
   `implementation_hash` files and the store's hashed methods, the v2 admission, round,
   audit, dispute and escrow code, and the existing CI shards (`tests/elastic/` runs in its own
   job).

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

The simulator measures the same trade-offs on a toy configuration (4 shards of 60 steps x 10 s,
2 rounds, 100 seeds each; ideal compute 10.67 GPU-h) [V sim, `elastic/sim.py`]:

| Configuration | Scenario | GPU-h | Overhead vs ideal | Makespan min | Wasted steps |
| --- | --- | --- | --- | --- | --- |
| no spare, checkpoint every 6 steps | none | 11.51 | +8 % | 21.6 | 0.0 |
| no spare, checkpoint every 6 steps | loss | 13.38 | +25 % | 26.3 | 2.9 |
| 1 hot spare | loss | 14.27 | +34 % | 22.3 | 2.4 |
| no spare | churn | 14.80 | +39 % | 30.8 | 9.4 |
| 1 hot spare | churn | 15.32 | +44 % | 25.6 | 9.2 |
| 2 hot spares | churn | 17.20 | +61 % | 23.6 | 7.9 |
| no spare, checkpoint every step | churn | 18.49 | +73 % | 37.9 | 1.6 |
| no spare, checkpoint every 30 steps | churn | 14.93 | +40 % | 31.0 | 37.4 |

Decision: checkpoint + reassignment by default. Hot spares are a run-policy parameter, chosen
from the measured rate. Speculative duplicates are reserved for verification needs. k-of-n is
rejected for bitwise-verifiable pretraining.

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
- `region-notice` and `region-loss`: every host of one region at once;
- `drought`: a regional loss, then no capacity for 30 min;
- `churn`: 2-8 mixed faults.

Every tick checks that no shard leaves a live holder, that no lease holds more than its
capacity and that a dead host's lease is released within the TTL. Every round end checks that
every accepted link is in the final chain and that each step is credited once. At the end, the
run must match the uninterrupted state hash, every loss must stay within one checkpoint
interval and be accounted for, a noticed host's leave must go out before its kill, and no
honest owner may be penalised.

200 seeds per scenario, default configuration (4 hosts x 8 GPUs, 4 shards, 60 steps x 10 s,
2 rounds, checkpoint every 6 steps, lease 20 s, notice 120 s) [V, run 2026-10-26]:

| Scenario | Done, final = ref | Violations | Exits notice/lapse/replaced/revoked | Penalised runs | Wasted steps mean / max | Max loss (bound 6) | Drain s mean | Makespan min | GPU-h (vs none) |
|---|---|---|---|---|---|---|---|---|---|
| none | 200/200 | 0 | 0/0/0/0 | 0 | 0.0 / 0 | 0 | 0.0 | 21.6 | 11.51 (+0 %) |
| notice | 200/200 | 0 | 198/0/0/0 | 0 | 0.0 / 0 | 0 | 4.0 | 27.0 | 13.82 (+20 %) |
| short-notice | 200/200 | 0 | 82/116/0/0 | 0 | 2.8 / 6 | 6 | 0.0 | 26.2 | 13.35 (+16 %) |
| kill | 200/200 | 0 | 0/198/0/0 | 0 | 2.8 / 6 | 6 | 0.0 | 22.7 | 12.04 (+5 %) |
| loss | 200/200 | 0 | 0/196/0/0 | 0 | 3.0 / 6 | 6 | 0.0 | 26.3 | 13.37 (+16 %) |
| partition | 200/200 | 0 | 0/196/0/0 | 0 | 4.2 / 6 | 6 | 0.0 | 23.0 | 12.25 (+6 %) |
| region-notice | 200/200 | 0 | 398/0/0/0 | 0 | 0.0 / 0 | 0 | 8.2 | 26.8 | 13.10 (+14 %) |
| region-loss | 200/200 | 0 | 0/394/0/0 | 0 | 5.6 / 12 | 6 | 0.0 | 26.1 | 12.68 (+10 %) |
| drought | 200/200 | 0 | 0/390/0/0 | 0 | 5.3 / 12 | 6 | 0.0 | 33.0 | 12.20 (+6 %) |
| churn | 200/200 | 0 | 250/572/0/0 | 0 | 9.2 / 26 | 6 | 3.0 | 30.7 | 14.75 (+28 %) |
| churn + cheater (slot 1) | 200/200 | 0 | revoked in 200 runs | 200, all `miner-01` only | | | | | |

A notice drain wastes no step. A hard departure wastes at most one checkpoint interval, so a
regional loss of two hosts wastes at most 12. The GPU-hour cost of a notice is mostly the
evicted host billed until its kill plus the shard waiting for its replacement (no spare). The
soak (`soak`, random hosts, regions, shards, steps, rounds, intervals, restart times, wrong
host lease guards and 10-30 % lost acknowledgements) ran 4,000 seeds x 10 scenarios on the final
code with 0 violations [V, 16 processes, about 70 s].

### Mutation check

`scripts/check_elastic_mutants.py` applies 27 one-line mutants and requires each named test to
fail (pytest exit 1; a collection error does not count): 27/27 killed [V, 2026-10-26]. Among
them:

| Mutant | Caught by (`tests/elastic/`) |
| --- | --- |
| checkpoint write not atomic (written under its key) | `test_store.py::test_put_killed_before_fsync_publishes_nothing` (child killed by SIGKILL at the fsync) |
| checkpoint write not synced before the rename | same |
| lease never expires | `test_membership.py::test_lease_lapses_without_a_heartbeat` |
| double assignment: capacity not consumed | `test_assignment.py::test_plan_gives_each_pending_shard_to_one_member_within_its_capacity` (hypothesis) |
| double assignment: a held shard planned again | `test_coordinator.py::test_a_departure_moves_only_the_departed_members_shards` |
| head not compare-and-set | `test_coordinator.py::test_a_commit_must_extend_the_accepted_head` |
| retried commit credited twice | `test_coordinator.py::test_a_retried_commit_is_acknowledged_again_without_new_credit` |
| accepted work dropped at a departure | `test_coordinator.py::test_a_lapsed_lease_releases_its_shard_and_head_without_a_tick` |
| resume trusts a spliced chain | `test_checkpoint.py::test_resume_refuses_a_chain_spliced_from_another_shard` |
| forged leave signature accepted | `test_coordinator.py::test_a_forged_leave_signature_is_refused` |
| drain flushes past the deadline | `test_drain.py::test_a_late_notice_leaves_at_once_without_flushing` |
| a lapse is penalised | `test_sim.py::test_lapses_and_notices_never_penalise_an_honest_miner` |
| replacement resumes from the base, not the head | `test_sim.py::test_resuming_the_same_heads_again_and_again_changes_nothing` |

### Running it

From `hypertrain/`:

```bash
uv run --frozen pytest -q tests/elastic                   # CI job `elastic`, about 15 s
uv run --frozen python scripts/check_elastic_mutants.py   # about 30 s
uv run --frozen python -m hypertrain.elastic.sim table    # one run per scenario
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
  about 185 USD for 2 x 4 h, on the ledger through Pilote [A programme §6]). Inject real
  notices with the AWS FIS action `aws:ec2:send-spot-instance-interruptions`, which sends a
  rebalance recommendation and then the two-minute notice [V, FIS docs read 2026-10-26], and
  inject hard kills too.
- Measure: notice detection latency, flush time to R2 for the real state size, restore on the
  other host, and wasted GPU-time. The final state must equal the uninterrupted run bit for bit
  (the B300 requalification of ADR 0004 comes first).
- Wire the drain into the miner: an IMDSv2 hop limit of 2 or host networking inside the image
  (the token PUT fails otherwise), replacing the shell watchdog.
- Estimates [H]:
  - simulator: 1 session, done;
  - protocol P1 (messages, store path, settlement): 3-4 sessions;
  - miner client (drain, resume from island checkpoints): 2-3 sessions;
  - auditor (link replay, sampling): 2 sessions;
  - docs: 1 session;
  - GPU test: 1 session.

  That is about 2 weeks of sessions after founder approval of P1.

## Founder decisions

1. Protocol-version bump P1 (elastic v3 rounds, opt-in per run): founder and Principal SN100.
2. Reward rules for partial participation:
   - credit per accepted step to the producer's owner;
   - whether a revoked member keeps credit for its earlier audited links (the code keeps it);
   - how the v2 per-miner cap (Q/4, at least 4 verified miners) applies;
   - whether idle hot spares are paid.
3. Penalties: none for notices, lapses or replacements (proposed). Any slash or burn for a
   failed audit has on-chain effects and is founder-only.
4. A lapse budget against a miner that joins, holds shards and vanishes repeatedly (harm today
   bounded by one TTL per join), and the retry budget for `INTERRUPTED` trials.

## Consequences

- The whole elastic loop is proven on CPU without a GPU or a provider account. The real state
  size, R2 throughput and eviction rate are not [?]. A large state may not flush within a
  two-minute notice, in which case the interval checkpoint bounds the loss [H].
- Synchronous checkpoints cost C/I of GPU time. The simulator waits for each write; an
  asynchronous upload would overlap compute but bound the loss by two intervals instead of one
  [H].
- `gpu_ops.network_qualification.sources()` (`src/hypertrain/gpu_ops/network_qualification.py:229`),
  `scripts/verify_image.sh` and `scripts/relay_kind_smoke.py` hash every file under
  `src/hypertrain`. Their frozen source manifests are made stale by these new files, as by any
  source change: regenerate them with the next image [V].
- Lease checks inside `commit` duplicate the holder check: a holder is cleared at every
  departure. They are kept because they give the miner the precise answer (lease gone versus
  shard moved) [V, tests].

## Rejected alternatives

1. Synchronous all-reduce across hosts with over-provisioned peers (Templar): it needs
   over-provisioning and was never shown under preemption [A Scout benchmark].
2. k-of-n shards: which shards count depends on timing, so the result is not bitwise
   reproducible.
3. The chain head as a mutable object in R2 (`current.json`): it needs conditional writes and
   meets the one-write-per-second limit per key; the coordinator's compare-and-set replaces it.
4. Penalising lapses: a hard kill is indistinguishable from honest failure, and the harm is
   bounded by one TTL.
5. Training until the deadline, then flushing: the notice is best effort and may come late, and
   leaving at detection frees the shard for a spare about 100 s earlier.
6. Host-side wall-clock leases: clock skew would decide expiry. The coordinator's clock plus
   fencing tokens replace them.
7. Reusing `data.store.LocalFSStore` for checkpoints: no fsync, and it never rewrites a torn
   object under its key.

## Reuse and provenance

No third-party code was copied. Design ideas only, all permissively licensed:

- heartbeat, timeout and "deathrattle" from INTELLECT-1 / prime-diloco (Apache-2.0);
- expiring membership records and checkpoint catch-up from Agora (Apache-2.0) and hivemind
  (MIT);
- IMDS polling from aws-node-termination-handler (Apache-2.0).

## Sources

- Code facts: this branch at `ac4c4dec` and later, read 2026-10-26.
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
  - Bamboo, NSDI'23 <https://www.usenix.org/system/files/nsdi23-thorpe.pdf>
