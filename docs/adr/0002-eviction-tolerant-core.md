# ADR 0002: eviction-tolerant training core

- Status: draft for branch `v3-core-eviction`, 2026-10-26 (batch v3-core-eviction, Principal
  SN100 D13). Forge decides promotion to `v3`. The library is new code behind explicit
  interfaces: nothing in the v2 challenge imports it, and no production run, pin, master or
  emission profile changes. Amended 2026-10-27 by the branch `v3-core-eviction-fixes` (the Lens
  gate, F1 to F7). Rows "During island training" and "After commit, before the delta" amended
  2026-10-27 by ADR 0011 part 5 (AUD-06). Amended 2026-10-27 by ADR 0010 for every run, not only
  P1 runs: an unproven trial is closed `EXPIRED` by the clock and suspends nobody, an unsigned
  challenge GET no longer exists, a dead unfunded `APPLIED` row is suspended `APPLIED_EXPIRED` at
  the next `join`, and a signed leave names the coordinator epoch. The rows of "What happens
  today" and of "P1 and the v2 admission rules" that describe the earlier behaviour are kept as
  the record of the review.
- Order: founder, 2026-10-26: the network runs mostly on Spot 8x B300 hosts that can be evicted
  at any time, with no lost work and no reinstall by hand (programme §2.3-2.4). Design
  checklist: lot brief `B-eviction-tolerant-core.md`.
- Labels: **[V]** verified in code, by a test or a run, or read in a primary source, on the date
  above or on the date given; **[A]** reported by a cited secondary document or by this batch's
  research and review agents and not re-read here; **[H]** hypothesis; **[?]** unknown.
  Repository paths are relative to `hypertrain/`.

## Context

### What happens today when a participant disappears (hypertrain v2, this branch)

| Phase | Today | Where |
| --- | --- | --- |
| Joined, challenge not yet fetched | After `seed + 100` rounds the challenge answers 409 `CHALLENGE_EXPIRED` and nothing changes. The admission stays `APPLIED` for good: it holds one of the 32 pending places (`PENDING_CAPACITY`), its hotkey is spent, and its coldkey stays blocked by the generic rule below [V] | `src/hypertrain/challenge/admission.py:236-240`, `:166-171`, `:158-166` |
| During a trial, past its deadline | The trial stays `OPEN` and holds one of the run's 2 trial places (`TRIAL_CAPACITY`), so two vanished miners block every newcomer. That lasts until someone requests that admission's challenge [V code, A review reproduction]. That request suspends the miner (`SUSPENDED`, reason `CHALLENGE_EXPIRED`) and marks the trial `EXPIRED` [V]. `join` refuses any coldkey with a non-`ACTIVE` admission [V]. `resume` itself accepts a `SUSPENDED` row with no dispute, but in `src/` only `resolve_pending` calls it, and that needs a pending dispute, which an expiry never creates: no route back found [V code, H no other route]. The challenge request is an unsigned GET, so any caller can trigger the suspension [V] | `admission.py:241-246`, `:216-228`, `:162-166`, `:1029-1091`; `challenge/app.py:449-454` |
| During island training | `torchrun --max-restarts=0`, no elastic rendezvous; nothing is published [V]. `schedule_audits_v2` leaves a roster member with no commit out of the round (`excluded`, reason `NO_SHOW`, ADR 0011 AUD-06), at the first beacon push after `d_commit` (ADR 0011 Part 8); the others are audited, aggregated and finalized. The miner is suspended with one strike and its bond burned unless the master was refusing its callers during the window or nobody committed, which conflicts with I5 above and is a sign-off item of ADR 0011 Part 8. The run stalls only when fewer than four members remain [V] | `src/hypertrain/miner/island_launch.py:402-415`; `src/hypertrain/challenge/store.py` (`schedule_audits_v2`, `_exclude_no_shows_v2`); `src/hypertrain/challenge/finality_v2.py:41-57` |
| After commit, before the delta | A member that passed its audit and uploads no delta by `d_upload` is left out by `aggregate_v2` (`NO_SHOW`, stage `delta`) and the round settles without it [V]. Missing leaves end in audit exhaustion, which excludes the miner (`INFRASTRUCTURE`) and marks the whole round `EXCLUDED`: nobody in it is paid [V] | `store.py` (`verified_inputs_v2`, `_exclude_missing_deltas_v2`), `:6650-6672` |
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
  - I1: no accepted work is lost to an eviction. An audit that later finds a link invalid
    voids it and the links built on it (`withdraw`, Decision 6).
  - I2: no step is credited twice.
  - I3: every accepted result is verifiable: a link replays from a stored parent state. That
    state stays stored until the link is audited and out of dispute ("Retention of checkpoint
    objects").
  - I4: wasted GPU time is bounded: one checkpoint interval per departure, plus the drain.
  - I5: an honest departure is never penalised by the core; the re-seed budget of P1 is the
    one cost it can carry (founder decision 5).

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
     leave therefore names one lease for ever and cannot
     release a later one, and a host of the old process is fenced [V
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
       from an older copy before a restart hands the same tokens out again, and nothing inside
       the log can tell, since a restore brings the mark back with the file. The leave therefore
       does not rest on the token alone (ADR 0010): each start of a coordinator draws an epoch
       of 32 random bytes that is never written to or read from the log; every lease carries
       it, the signed leave names it, and a leave of another epoch is refused with
       `STALE_EPOCH` before its signature, the retry answers or any lease is looked at. The old
       leave that a restore would have revived names the epoch of an earlier start, so it
       releases nothing whatever token the restored mark re-issues [V
       `test_a_leave_signed_before_a_token_log_was_restored_cannot_release_the_lease_it_reissues`].
       A copy or a hard link of the log has a lock of its own, so two coordinators can run on
       copies and issue the same tokens; each has its own epoch, so a leave signed under one
       releases no lease of the other [V
       `test_a_leave_signed_under_one_coordinator_cannot_release_a_lease_of_a_copy_of_its_log`].
       An epoch kept in the log would come back with it, so none is stored. What this does not
       do: it does not fence heartbeats and commits (a stale host that holds a re-issued token is
       refused by the wiring's signature check, not here), it does not stop two coordinators
       from running (that is the lock's work, and a copy defeats it), and it does not survive a
       restore of the whole process, which brings back its epoch with its mark [H]. A restore
       call that the wiring adds must draw a new epoch and never persist one. The complete
       answer is a single writer on a compare-and-swap row (ADR 0003), which is the wiring's.

     The library persists nothing else: a restarted coordinator has forgotten everything but
     the mark (heads, credits, owners, revocations, retry answers, progress windows, penalties),
     and nothing in the API restores it yet. The wiring must add a restore call; until then a
     restart can only reopen the round from its base, and nothing it had accepted is credited.
     Every host joins again.
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
   capacity (default 1 shard per host). It is a pure function of its inputs (seed, shards,
   free capacity), so an auditor who has the ordered calls and their clock readings can
   recompute it; the library keeps no log. A departure moves only the departed member's
   shards, and a live holder is never displaced, bar the hand-over below.
   **Progress deadline**: a member holding work loses its lease (exit `STALLED`, not
   penalised) when it makes no accepted link within `progress_ms`. The window is counted per
   member, from its last accepted link or from when it started holding work, and a new shard
   does not reopen it. Its shards are reassigned and the member may join again. A sweep lapses
   every late member before it plans the freed shards. A host that heartbeats but cannot
   upload, or restarts in a loop, therefore never stalls a round; a crash loop loses its shard
   within about two windows.
   **Hand-over** (capacity above 1). Because the window belongs to the member, a host that
   advances one of its shards could keep another for ever: Lens measured a second shard held
   960 windows without a link while a live member waited. Each shard therefore also keeps the
   time it was assigned or last advanced, and the time of its last link; a replacement host
   does not refresh them, or a host that rejoins once per link would keep an unworked shard for
   ever. A shard that is a window old is handed to a live member that holds nothing (the idle
   member with the highest rendezvous score for that shard), one shard per idle member and
   sweep, unless it is the one its holder works:
   - a host works its shards in ascending order, which is the order of its grant, so the first
     shard of a holder is taken only if the holder has advanced another of its shards after it
     got, or last advanced, that first one (strictly after: a link in the same millisecond does
     not count). A host that trains its shards in turn is not robbed of the one it trains; a
     host that works its second shard first and starves the first still loses the first;
   - a host never loses its last shard, also when several of its shards are old in one sweep.

   It is a `Move` in the settlement, not a departure: nobody is penalised, the holder keeps its
   lease and its other shards, and its progress window restarts at each Move, since the
   coordinator took work from it; a rejoin after a Move does not renew it again, a link
   re-arms that. A host that never works its first shard therefore outlives its window by up
   to capacity - 1 further windows, one for each shard it loses, and then stalls as before
   (3 shards at capacity 3 and a window of 4 s: stalled 2.75 windows after the last link
   instead of 1 [V, review of 2026-10-27]). The freed shards are planned before the hand-over
   runs, so an idle member takes a shard a stall freed, not one from a working host. With no
   idle member nothing moves. A member that holds a shard is taken to be working it, so
   shards never pass between busy members [V
   `test_a_shard_its_busy_holder_never_works_goes_to_a_member_that_holds_nothing`,
   `test_a_hand_over_takes_a_queued_shard_never_the_one_its_holder_works`,
   `test_a_host_that_works_its_second_shard_first_loses_the_first`,
   `test_a_host_never_loses_its_last_shard_to_the_hand_overs_of_one_sweep`,
   `test_a_shard_a_stall_frees_goes_to_the_idle_member_not_to_a_hand_over`,
   `test_the_window_a_hand_over_gives_is_not_renewed_by_a_replacement`]. Costs:
   - a shard can still be taken while its holder works its first link on it, when a lower
     shard was assigned to the holder after it started: it then wastes that restore and up to
     one checkpoint interval of steps (161 hand-over losses in 13,123 capacity-2 simulator runs
     over seeds 0 to 2,399 of `random_config`, 5 to 19 for each 150 seeds; 131 of them more
     than half an interval, none above one) [V, a throwaway script of the review, not in the
     tree];
   - the planner is not load-aware: at `open_round`, and at each `join` while the first hosts
     fill their capacity, a host often wins two shards while others hold none, and the second
     waits a whole window before the first hand-over. The fault-free simulator at 4 hosts and 4
     shards, 2 rounds, took 26.3 minutes and 14.03 GPU-h at capacity 2 against 21.6 minutes and
     11.51 GPU-h at capacity 1, 22 % more of each (mean of 50 seeds) [V]. Capacity above 1
     should be configured only when hosts train their shards in parallel;
   - two idle members that heartbeat and never upload can take a waiting shard in turn, each
     stalled after a window without penalty, as they can take a pending shard at capacity 1
     today. The hand-over lets them take it from an honest holder. The stall budget of founder
     decision 4 bounds this; this library does not.
3. **Checkpoints** (`checkpoint.py`, `store.py`).
   - What: the full state a shard resumes from. For hypertrain that is the `pack_state` blob of
     theta, m and v in fp32 plus the step (`src/hypertrain/auditor/replay.py:142-149`), so
     about 12 bytes per parameter: 12 GB for 1 B parameters, 96 GB for 8 B [V format, H
     sizes].
   - Format: a content-addressed state object, and a content-addressed manifest
     `ht-elastic-checkpoint-v1\n || JCS{version, run_id, policy, round, shard, base, parent,
     step, state, producer, lease}`. A manifest is written only after its state exists.
   - Where: the `CheckpointStore` protocol (`put`, `get`, `exists`; no delete, no list). Local
     filesystem now; next, the R2 store of v3-dataplane (`hypertrain.storage.cas.R2ObjectStore`,
     in this tree and wired into no challenge path, ADR 0003). Nothing in the store is
     mutable: the chain head lives in the coordinator, so R2 never needs a compare-and-swap or
     more than one write per key. `exists` means "a put of this key completed"; only `get`
     proves the bytes, so the store's root belongs to the store alone. ADR 0003 (Decision 3)
     holds the key layout and the lifecycle; what a deletion may not break is set in
     "Retention of checkpoint objects" below, and the event that releases a state is founder
     decision 7.
   - Who reads: the coordinator reads manifests and checks that a new state exists; the next
     holder reads every manifest back to the base and the head's state; the auditor reads the
     parent state of the link it replays [H, P1 is not built]. The table in "Retention of
     checkpoint objects" gives the reader of each object.
   - Cadence: every `ckpt_every` inner steps and at the end of the unit, plus a flush on
     notice.
   - The local store fsyncs the bytes under a unique temporary name, renames, then fsyncs the
     directory, and the root before every write (a fan-out entry may come from a writer that
     died). It also fsyncs an object it finds intact. `data.store.LocalFSStore` does neither and
     stays as it is for v2.
4. **Resume proof.** A resumer walks the chain from the head back to the round's base: every
   link must bind the same run, policy digest, round, shard and base, with a step within the
   unit and strictly decreasing towards the base. Every state hash is checked on read; a torn
   object is `CheckpointError("CORRUPT_OBJECT")`. The coordinator accepts a new head only if
   its parent is the head it holds, its step is higher, its state exists, and its producer and
   lease are the committer's (compare-and-set). The hash link is the proof that a replacement
   resumed from the right state, and each link (parent step to step) is one replayable audit
   segment. The walk reads every manifest, so no manifest of a live chain may be deleted, and an
   object missing from it ends the resume with a fail-closed error that the coordinator, not the
   resumer, resolves ("Retention of checkpoint objects").
5. **Detection and the signed leave** (`notice.py`, `drain.py`).
   - `AwsSpotNotice` reads IMDSv2 and renews an expired session once, never through an HTTP
     proxy. Any other answer, and an unreachable or silent IMDS, raises `NoticeError`; the
     caller keeps working under its lease. `FakeNoticeSource` replays a schedule.
   - The miner calls `DrainHandler.check()` between inner steps and keeps heartbeating while it
     drains. If the time left covers the last measured flush plus a margin, it flushes a
     checkpoint at the current step, then sends the leave; otherwise it leaves at once.
   - The leave goes out even when the flush fails. A failed leave is retried alone, with the
     notice and the flush decision already taken.
   - The leave is `JCS(LeaveNotice{version, run_id, member, signer, lease, epoch, reason,
     provider_time, heads})` under domain `hypertrain/elastic-leave/1|`, signed by the host key
     the member registered at join (`protocol.server_keys.check_sig`). `heads` (at most 1,024,
     and no more than the round has shards) and `provider_time` are evidence: the verified
     notice is kept on the member's `NOTICE` departure and never trusted over the coordinator's
     own heads.
   - A host without a notice is detected by lease expiry or by the progress deadline.
6. **Coordinator and settlement** (`coordinator.py`).
   - Every mutating entry point (`open_round`, `join`, `heartbeat`, `leave`, `tick`, `commit`,
     `withdraw`) and `settle` first lapses expired leases and stalled holders. `holder` and
     `links` are pure reads.
   - A shard has at most one holder, a (member, token) pair.
   - Retries are idempotent. A commit of a link its member had accepted is answered again
     without new credit, before any lease check, and a leave already accepted under the same
     lease is answered with its departure. A retry after a lost answer is therefore safe, within
     one process: the answers are lost with it. Both are kept for the round they were given in
     and the round after it, then dropped: the retry memory of a long run is bounded by two
     rounds, and a retry two rounds late meets the ordinary checks (lease, holder, round
     binding) and is refused.
   - Credit is the steps each accepted link adds, paid to the owner of the member that produced
     it, so an evicted miner keeps all it had accepted.
   - Only a link that fails the audit penalises its producer, whose member is revoked.
     Departures (`NOTICE`, `LAPSE`, `STALLED`, `REPLACED`) are never penalised.
   - `audit` is a required argument of the coordinator: no link is credited without a verdict,
     and `check_link` alone only proves that the state object exists. The audit runs inside
     `commit`. A replay of a whole link may be too slow for that call (a GPU-scale replay), so
     it can be a cheap check, and the full replay a later job that calls `withdraw(key)` when
     it finds the link invalid. `withdraw` voids the link and every link built on it in the
     open round, with their credit; moves the shard's head back to the link before it, so the
     next holder resumes from a state that was not forged; penalises and revokes the producer
     of `key` as a failed audit does, and not the producers of the links built on it, who were
     honest; gives an honest holder of the shard a fresh window to restore; and plans the
     shard again. A closed round's settlement was returned and is not reopened: the audit must
     finish before the wiring opens the next round [V
     `test_a_withdrawn_link_takes_its_credit_and_the_links_built_on_it`]. The price of a
     deferred audit is that an honest host which resumed from an unaudited head loses every
     link it built above a forged one (a forged 1-step link can void hundreds of honest
     steps), so the wiring either audits inside `commit` or bounds how deep it lets work build
     on an unaudited link. The audit, `admit` and the store reads of `check_link` run in the
     serialised section: all three must finish well under `ttl_ms` minus the heartbeat interval,
     or every lease lapses while they run, and an audit that cannot decide must raise instead
     of returning False, which revokes and penalises its producer.
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

   The admission side is a successor of `AdmissionPolicyV2` with a re-seed budget and a dormancy
   delay (placeholder names). Its digest is pinned by `network.admission_policy_hash`
   (`challenge/admission.py:122-123`), so it changes the run ID too. What P1 does with each v2
   admission rule is in "P1 and the v2 admission rules" below.

   Under P1:
   - A round is a set of S shards held under leases, not a fixed roster that must all commit.
     Roster sizing: the run policy fixes S, and any M ≥ 1 live members make progress. At the
     default capacity of 1, ⌈S/M⌉ waves are needed and members beyond S are hot spares. With
     capacity c > 1, as few as ⌈S/c⌉ members may hold every shard while the rest stay idle for
     a window, until the hand-over of Decision 2 gives them one, so c > 1 pays only if a miner
     trains its shards in parallel. The quorum is "every shard
     complete, whoever completes it", so a no-show never stalls a round.
   - An eviction is a verified leave under a live lease, a lapse (hard kill, host loss,
     partition: indistinguishable from honest failure) or a stall. None is penalised, and
     accepted links are credited. A held shard that never progressed earns nothing. Only a link
     that fails the audit is penalised.
   - A trial keeps v2's proof window of 100 rounds (about 300 s from its seed; option E1 below).
     The lease and the progress deadline release a lost holder's trial place at once; they do
     not resume the trial, and a replacement host runs a new one, since this ADR's replacement
     delay (10 minutes, 200 rounds [H arithmetic]) outlasts the window. Replacing the window by
     lease and progress (E2) would edit `challenge_rounds: Literal[100]`, the
     `deadline == seed + 100` validator and every user of `deadline_beacon`, so it needs a new
     policy and a new run [founder].
   - A trial whose holder leaves, lapses, stalls or is replaced ends `INTERRUPTED`, not
     `EXPIRED`. The master writes it from a departure it has persisted
     (the coordinator keeps them in memory), and only while the trial is `OPEN` and unproven:
     `proof()` and
     `finalize_trial` never read the trial outcome today, and a late proof for a closed trial is
     refused only because no reference was committed (`admission.py:678-682`, `:888-890`). It
     frees the trial place, suspends nobody, leaves `screen_until` and `clean_count` as they are
     and adds no coldkey block. For a P1 run the lazy branch of `challenge()` (`:222-228`) must
     close an `OPEN` trial past its deadline as `INTERRUPTED` too, with no suspension: a holder
     that keeps its lease and never proves would otherwise be suspended with no route back, by
     any caller of the then unsigned GET (an owner-signed POST since ADR 0010, and no lapse
     suspends). A data-plane outage recorded by the coordinator is an
     excused miss in the same way; ADR 0003 (Decision 10) leaves that to this ADR.
   - A new trial needs a re-seed. `challenge()` refuses once `now > seed_beacon + 100`, and only
     `join`, a trial finalization and `resume` set the seed (`:197`, `:891-895`, `:1043-1048`),
     so an eviction longer than about 300 s after the last finalization strands the miner; this
     ADR's own replacement delay is 10 minutes and its drought 30. P1 adds a request signed by
     the admission's hotkey (a new v2 envelope type with a single-use `request_id`); the master
     sets `seed_beacon` to its own verified round plus one, as `join` does. It applies to
     `APPLIED`, `PROBATION` and `ACTIVE` rows, so for P1 runs it is the code-change option of the
     choice that `hypertrain/docs/network-v2.md` leaves open before a production admission
     policy is signed (a larger screen value or a code change; the idle-miner paragraph).
   - An `APPLIED` row whose seed window has passed is dormant, not expired: "expires with that
     window" is withdrawn. `join` refuses any coldkey with a non-`ACTIVE` row
     (`COLDKEY_PROBATION_EXISTS`, `admission.py:162-166`), and nothing in v2 expires a row by
     time (a raising `challenge()` rolls its transaction back). The dormant row keeps its
     pending place, its spent hotkey and its coldkey block, and can be re-seeded. After
     `dormant_rounds` the master suspends it with reason `APPLIED_EXPIRED`, at the start of
     every `join` and in the re-seed route; `join` then ignores rows in that state when it
     applies the coldkey rule, and every other non-`ACTIVE` row still blocks.
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
     `checkpoint`, `membership`, `assignment`, `tokens`, and `notice` for `verify_leave` and the
     `heads` bound) join that path's `implementation_hash` set.
     That is a v3 protocol-version bump; v2 runs, golden roots and the v2 hash do not move.
8. **Not changed** [V]: `src/hypertrain/protocol/`, the golden leaf roots, the ten
   `implementation_hash` files and the store's hashed methods, the v2 admission, round, audit,
   dispute and escrow code, and the existing CI shards (`tests/elastic/` runs in its own job).
   Nothing under `src/hypertrain` outside `elastic/` imports the package. The package imports
   from the rest of hypertrain 18 names in 8 modules: from `data.store` `CorruptObjectError`,
   `ObjectNotFound` and the private helpers `_check_key` and `_verified` (`_check_key` is also
   imported by `storage/cas.py`, `storage/offer.py`, `data/stream_store.py` and, inside a
   function, `aggregator/capacity_worker.py`; `_verified` by `storage/cas.py`); from `protocol`
   `envelope.EnvelopeError`, `envelope_v2.load_json`, `jcs.canonicalize`, `keys.Keypair`,
   `keys.KeyError_`, `keys.decode_hotkey`, `messages.Hex64`, `messages.SS58`,
   `messages_v2.U64`, `messages_v2.Identifier`, `messages_v2.Positive`, `messages_v2.WireModel`,
   `server_keys.Sig` and `server_keys.check_sig` [V `tests/elastic/test_imports.py`]; and
   `httpx` and `pydantic` from outside. A change to one of them can change what the package
   accepts. The protocol goldens guard `canonicalize` (`tests/protocol`), but the manifest bytes
   (hence every checkpoint key) and the leave message have no known-answer vector of their own,
   and a relaxed `Identifier` passed every suite in Lens' measurement [A, review F16, not
   addressed here]. Public wrappers for the two private helpers were not added: four other
   modules import `_check_key` and one imports `_verified` as they are, and renaming them is a
   change to `data/store.py` outside this batch.

### P1 and the v2 admission rules

Paths are under `src/hypertrain/` and `challenge/admission.py` unless prefixed; the left column is
[V, read 2026-10-27] unless marked. A round is 3 s: 100 rounds are about 300 s, the 6000-round
screen epoch about 5 h. A trial does not fit its window if the host is down for more than the
window minus one trial's duration (about 36 rounds, 108 s, with joinbase-6's 64 [A, arithmetic]).

| v2 rule | What P1 does | Status |
| --- | --- | --- |
| `screen_until` is written only at proof acceptance (`:745-754`) and reset by `resume` (`:1045`); eligible needs `now <= screen_until` (`:936-945`); W = 6000 in the example (`ops/manifest_v2.py:189`) | `INTERRUPTED` does not extend it: a screen is hardware evidence and an interrupted trial adds none. An evicted `ACTIVE` miner stays eligible until `screen_until`; after it, one re-seed and one `MATCH` trial restore it without leaving `ACTIVE`, since acceptance rewrites `screen_until` | proposal |
| `clean_count` +1 at finalization (`:891-893`), reset only by `resume` | `INTERRUPTED` neither adds nor resets; a `PROBATION` miner evicted for longer than W keeps its count | proposal |
| The renewal window after a finalization is `challenge_rounds` minus a trial; only suspend and `resume` reopen it, at zero (`hypertrain/docs/network-v2.md`, idle-miner paragraphs) | The re-seed is the code-change option of that note, for P1 runs. v2 runs keep today's rule; fixing them is a separate admission change | founder |
| The seed has three writers (`:197`, `:891-895`, `:1043-1048`) | A fourth, the re-seed, never inside the unsigned GET: any caller could otherwise spend a miner's budget and the 2 trial places | proposal |
| `TRIAL_CAPACITY` counts `OPEN` rows, at most 2 (`:242-246`); a vanished holder keeps one until someone requests the challenge (`:216-228`) | The place is freed when the lease or the progress deadline ends. The outcome `INTERRUPTED` is free text in `admission_trial_results`, no schema change | proposal |
| `max_pending` 32 counts `APPLIED` and `PROBATION` rows (`:167-171`); the hotkey stays spent (`:158-161`) | `dormant_rounds` covers `APPLIED` only; a dormant `PROBATION` row keeps its place and its count. The locked units of a dormant or `APPLIED_EXPIRED` row are not released by `suspend` (`:1012`); what releases them is open | founder; `PROBATION` and lock [?] |
| Lapses are not sanctioned, so a miner that takes a trial and vanishes dodges the v2 suspension for the cost of one TTL or progress deadline, and leaves a trial's inputs staged (`challenge/store.py:2752-2796`) | A re-seed budget per coldkey and epoch (`now // work_screen_epoch_rounds`), read through `admission_history.coldkey` so a new hotkey does not reset it. Every trial that closes without `MATCH` or `MISMATCH` counts, because lapses and honest evictions cannot be told apart. Exhausted: re-seed refused until the next epoch, nobody suspended. A flat value is wrong for a regional loss, which hits every host of a coldkey at once. This is a cost on an honest departure (I5) | founder |
| The screen belongs to the admission (`challenge/admission_store.py:47-49`), not to a host | A replacement host of the same owner inherits the live screen; only the periodic re-screen covers hardware that differs between Spot hosts | founder [?] |
| The admission modules are outside the ten `implementation_hash` paths and the 28 owned names of `challenge/store.py` (`:4249-4260`, `:4283-4312`) | Whether they should join is open; the policy fields and E2 change the policy digest and the run ID either way | [?] |

Rejected for P1: extending `screen_until` by the eviction time (the screen would vouch for
hardware that was not re-tested), resetting `clean_count` (it penalises an honest departure), an
automatic re-seed after `INTERRUPTED` (it opens windows for hosts that may not return), a longer
`challenge_rounds` (it is E2, and only moves the cliff, since a drought has no upper bound),
deleting the dormant row (it breaks the replay of its `join`, the transition log and the spent
hotkey), a new `AdmissionState` value (a wire Literal that `RosterEntryV2` also uses,
`protocol/messages_v2.py:51`), and suspending on a lapse (the v2 outcome: an honest eviction would
end the identity).

E1 is conditional: ADR 0003 puts the transfer of a trial (about 2.1 GB up and 1.98 GB down) at
196 Mbit/s over the 167 s that a 130 s probe leaves of the 297 s window, or 306 Mbit/s with 60 s
of margin [H], so E1 holds for a remote Spot host only if its link is that fast, and the founder
should weigh it against E2. Tests P1 needs before it changes any of this: one test
covers the lazy suspension today (`tests/challenge/test_admission_v2.py:220-235`) and
`COLDKEY_PROBATION_EXISTS` another (`tests/challenge/test_service_network_v2.py:2467`); none
covers the refusal at `seed + 100` of a never-fetched `APPLIED` row or its effect on the coldkey
rule [V grep]. P1 adds: a re-seed refused for a `SUSPENDED` row, a pending dispute or an `OPEN`
trial; `INTERRUPTED` leaving `screen_until` and `clean_count` alone; `join` ignoring only
`APPLIED_EXPIRED` rows; the budget counted per coldkey across hotkeys; `proof()` and
`finalize_trial` refusing a trial that is not `OPEN`.

### Retention of checkpoint objects

ADR 0003 (Decision 3) stores these objects and lets the coordinator delete them; this is what the
deletion may not break. It is a constraint, not a description, because nothing deletes yet:
`CheckpointStore` has no delete or list, nor has `R2ObjectStore` (its `R2Client` has both), no
module outside `elastic/` builds a coordinator or a checkpoint store, and the v3 audit, the dispute
path and the outer step belong to P1 [V, reading and grep]. The audit, dispute and round-close
columns are [H] constraints on the future auditor, taken from I3, Decisions 4, 6 and 7, the
simulator's audit (`Simulation._audit`) and the v2 readers (`auditor/replay.py`,
`auditor/island_bisect.py`, `challenge/disputes_v2.py`).

A shard has a base state B(r) (a key per unit; the simulator gives all units one), a state S_j and a
manifest M_j per link, and a head, the key of its newest accepted manifest. The head is coordinator
state, not a store object. "Release of link k" means: its audit is final (a failed audit withdraws
it first, so its parent becomes the head again), every dispute and penalty naming it is closed,
and a grace has elapsed. A round's retention ends when every link of that round is released:
its manifests go then, not before (rule 3), and so does the wiring's record of its heads and
statuses (rule 4). With the audit inside `commit` the audit part is zero; the audit must finish
before the next round opens (Decision 6), and after a round closes only a dispute can void a
link.

| Object | resume | check at `commit` | audit replay of link k | dispute inside link k | round close | Deletable after |
| --- | --- | --- | --- | --- | --- | --- |
| B(r) | only if the shard has no link | `open_round` needs it stored | parent of link 1 | parent of link 1 | B(r+1) is built from the heads [?] | release of link 1 of every unit that names it, and B(r+1) stored |
| S_j | S_head only | `exists` of the new state, no bytes | S_(k-1) bytes; S_k by hash | both ends of the contested link may be asked for [H] | the heads' states build B(r+1) | the release of link j and of link j+1; the last state of a shard: round settled and B(r+1) stored |
| M_j | every M from the head to M_1 | M_new and M_head | M_k and M_(k-1) | the chain, as evidence | the chain of each shard | never before the round's retention ends |
| head | where the walk starts | must equal M_new.parent | names link k | names the contested link | `Settlement.heads` | as long as the manifests |

1. **Invariant.** An object is never deleted while it is (a) a manifest on `chain()` from the head
   of a shard of any round whose retention has not ended, (b) the state of such a head, or B(r) of
   a shard with no link, (c) the state of a link that is not released, or the parent state of one,
   or (d) needed under (a) to (c) by another link, round or evidence, since equal bytes share one
   key. The `base` and `state` fields of a retained manifest do not pin an object by themselves.
2. **When.** "Superseded by a newer head" releases nothing. v2 gates custody on finality, vesting,
   closed disputes and an epoch grace (`protocol/relay_messages.py:466-489`,
   `challenge/store.py:5415-5438`) and extends the horizon only by an explicit control action issued
   before it expires (`challenge/store.py:5390-5413`) [V]. Proposed [H]: the same release for
   checkpoints. With the audit inside `commit` and no dispute, two states per shard suffice only if
   the dispute window plus the grace is at most one checkpoint interval [?]; a sampled or later
   audit needs the parent state of every unreleased link, and a link that is never sampled becomes
   final when the sampling decision closes, or its parent is never deletable.
3. **Manifests** are 344 to 720 bytes, about 430 with ordinary ids [V, measured with
   `CheckpointManifest.encode`]: deleting one saves
   nothing and breaks `resume`, a rollback and the keys that notices and penalties name. They go
   when their round's retention ends.
4. **Record.** The `CheckpointStore` protocol has no list (`R2Client.list_prefix` exists, but cannot
   tell accepted links from orphans), so the wiring persists each round's heads and each link's
   audit and dispute status until the round's retention ends. Without that record no delete is
   safe and none is made. Objects no head reaches (a flush killed after its state, a link refused
   by the compare-and-set) are not deleted one by one; they leave with the run-level expiry.
5. **Mechanics** [H]. A multipart state has a sidecar block list in the same prefix
   (`storage/cas.py:277-279`, `:329-339`): delete the state, then the sidecar. Only the
   coordinator's credential deletes. An R2 lifecycle rule counts object age and prefix, not run
   state [A, `r2/buckets/object-lifecycles` as read for ADR 0003 on 2026-10-26], so on `ckpt/`
   only a run-level rule
   created after the run is settled and its disputes are closed is allowed, or the coordinator
   lists and deletes the prefix then: a rule installed at run start would expire the head state of
   a stalled round.
6. **A missing object is a fail-closed error** [V code]: `MISSING_MANIFEST`, `MISSING_STATE` or
   `CORRUPT_OBJECT` (`elastic/checkpoint.py`); a store outage is a different error. `resume` never
   falls back to the base or to an older link, which `check_link` would refuse as `NOT_HEAD_CHILD`.
   A missing or corrupt head manifest then fails every commit on the shard, since `check_link`
   reads it; a missing or corrupt head state fails the resume of the next holder. Each holder is
   stalled in turn [H]. The coordinator decides the fallback [H]: (i) check the store and
   any mirror, since bytes verify by hash anywhere (ADR 0003, Decision 10); (ii) roll the shard back
   to the newest intact link by withdrawing the links above it (`withdraw` penalises the producer
   of the key it voids, and a rollback caused by lost objects needs a variant without penalty
   that does not exist [V]; the credit of the voided links is founder decision 2); (iii) if no
   link is intact, restart from B(r) when it is intact; (iv) otherwise the round stays open and an
   operator acts. An audit that cannot read its parent state must fail the audit job with no
   verdict against the producer, as v2's `AuditInputError` does (`auditor/replay.py:77-79`):
   returning False would revoke and penalise an honest member.

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

Checks of the core's observable behaviour. `tests/elastic/test_sim.py` kills the lease-expiry,
double-assignment, no-progress-deadline, retried-commit-credited-twice and dropped-work mutants
of the list below. It does not kill the crash-loop window, late-member sweep and
retry-after-lease mutants (a 300-seed soak of each reports 0 violations), nor the store, notice
and signature, admission, compare-and-set, chain-splice, token-log, `withdraw`, required-audit,
hand-over bookkeeping and import-list ones: those are caught only by their unit tests, because
the simulator uses an
in-memory store, a fake notice source, a memory token log that never restarts, admitted honest
hosts, an audit that runs inside `commit`, and never rejoins in a loop. The checks:

- no shard leaves a live holder, bar a recorded hand-over;
- no lease above its capacity;
- a hand-over goes only to a member that held nothing;
- a dead host's lease is released within the TTL;
- no holder is stalled without a fault (partition, kill, loss or eviction) in its window;
- no live work is refused (`refused`) or displaced (`displaced`), bar a recorded hand-over;
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

The soak (`soak`) ran 6,000 seeds x 11 scenarios (66,000 runs) with 0 violations on the final
code [V, 2026-10-27], in three batches over disjoint seed ranges (1,800, 2,400 and 1,800
seeds; the last, with the final oracle, is seeds 4,000,000 to 4,001,799). A batch before the
last change had found the one run (seed 2000019) that led to the fresh window of a host that
loses a shard to a hand-over;
`test_a_host_that_loses_work_to_a_hand_over_is_not_stalled_for_it` pins it. With the capacity
forced to 2, 3 and 4, 600 seeds each (6,600 runs each; 2,854, 5,430 and 5,910 hand-overs), no
run was flagged [V]. Its configurations draw at random:

- hosts, regions, shards, steps and rounds, and a capacity of 1 or 2;
- checkpoint intervals of 1 to `steps` inner steps, and write times of 5, 15 or 30 s;
- TTLs of 20 or 60 s and restart times;
- host lease guards that are wrong;
- lost acknowledgements for 0, 10 or 30 % of the answers.

### Mutation check

`scripts/check_elastic_mutants.py` applies 170 single-snippet mutants and requires each named
test to fail (pytest exit 1; a collection error does not count). The child writes no bytecode
and SIGTERM restores the file. Result: 170/170 killed [V, 2026-10-27]. Among them:

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
| a leave of another coordinator epoch is accepted | `test_coordinator.py::test_a_leave_signed_under_one_coordinator_cannot_release_a_lease_of_a_copy_of_its_log` |
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
uv run --frozen pytest -q tests/elastic                   # CI job `elastic`, about 25 s
uv run --frozen python scripts/check_elastic_mutants.py   # about 2 min
uv run --frozen python -m hypertrain.elastic.sim table    # one run per scenario
uv run --frozen python -m hypertrain.elastic.sim aggregate --seeds 200   # the evidence table
uv run --frozen python -m hypertrain.elastic.sim soak --seeds 5000
# long soak, 16 processes over disjoint seed ranges:
for i in $(seq 0 15); do
  uv run --frozen python -m hypertrain.elastic.sim soak --seeds 2500 --start $((i * 2500)) &
done; wait
```

CI: job `elastic` of `hypertrain-ci.yml`. It installs no torch, runs the suite under a 5-minute
step timeout and then the mutation check under a 10-minute one, on a 4-vCPU runner. The
timings above are those of a busy 128-core host; the runner's are not measured [?].

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
   - whether the honest producers of links built on a withdrawn link keep partial credit (the
     code voids it, since their work started from a forged state);
   - how the v2 per-miner cap (Q/4, at least 4 verified miners) applies;
   - whether idle hot spares are paid.
3. Penalties: none for notices, lapses, stalls or replacements (proposed). Any slash or burn for
   a failed audit has on-chain effects and is founder-only.
4. Budgets:
   - a lapse or stall budget against a miner that joins, holds shards and vanishes repeatedly
     (harm today bounded by about two progress windows per join, with nothing to bound the
     joins: two idle members that join and stall in turn can hold one shard for as long as they
     like, a pending one at capacity 1 and also a waiting one of an honest holder at capacity 2);
   - the re-seed budget for trials: decision 5.

5. Admission rules under P1 ("P1 and the v2 admission rules"):
   - the signed re-seed request, for `APPLIED`, `PROBATION` and `ACTIVE` rows;
   - the re-seed budget, its window and whether exhaustion only refuses the re-seed (proposed)
     or is excused after a verified leave or a recorded outage; its value is sized from the
     measured eviction rate (lot D) [?];
   - `dormant_rounds` for `APPLIED` rows (proposed: one `work_screen_epoch_rounds`, 6000
     rounds), and whether it also releases `PROBATION` rows;
   - E1, keep the 100-round proof window (proposed), or E2, replace it;
   - whether a replacement host inherits the live screen (proposed) or must re-screen.
6. Whether v2 production runs get the re-seed too, as a separate admission change, before the
   first production admission policy is signed.
7. Checkpoint retention ("Retention of checkpoint objects"): the release event after which a
   parent state may be deleted (proposed: v2's custody release, that is finality, vesting, closed
   disputes and an epoch grace), the delay T_hold from a link's acceptance to that release, and
   with them the end of a round's retention (every link of the round released), after which its
   manifests go. T_hold sets how many states ADR 0003's cost model holds per shard.

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
- Nothing deletes a checkpoint object yet [V]: the store protocol has no delete, and the coordinator
  keeps no record of past rounds' heads or of audit and dispute status. Until "Retention of
  checkpoint objects" is built, storage only grows, and the cost model of ADR 0003, which assumes
  deletes, is a lower bound.
- The flush on a Spot host has no implementation in either branch [V]. `DrainHandler` calls an
  injected `flush`; the only non-test one is the simulator's, which saves into the coordinator's
  in-memory store. ADR 0003 (Decision 5) says that the offer-client adapter a host without an S3
  credential needs is not built, and it depends on coordinator routes that no document defines
  yet. A flush on notice has therefore never reached a real store, and the flush time of a real
  state stays [?] until lot D. `R2ObjectStore` holds the S3 credential, so it runs only on the
  coordinator and on trusted operator hosts.
- Known limits [V]:
  - a put killed before its rename leaves its temporary file (`.<sha>.<uuid>.tmp`) under the
    store root until the host's owner removes it, and a token log made or saved by a process
    killed at the wrong moment leaves `.<name>.<uuid>.tmp` next to the log, a copy of an old
    mark that nothing reads;
  - no test injects a power loss, so the directory fsyncs, also those of the token log, are
    covered by review only;
  - a member killed repeatedly before it completes a link loses its shard within about two
    progress windows, by design and without penalty;
  - within a round the coordinator keeps one departure per exit, with the leave notice it
    carried, and it does not rate-limit joins; a flood of joins and leaves by an admitted key
    is bounded by the wiring's request limits, not by this library. A notice names at most as
    many heads as the round has shards (Lens measured 517,522 bytes for one at 1,024 heads), and the
    retry answers kept across rounds are bounded by two rounds; the member table and the
    progress windows keep one entry per admitted member id, which `admit` bounds [V
    `test_a_leave_names_no_more_heads_than_the_round_has_shards`,
    `test_the_leaves_kept_do_not_grow_with_the_rounds`];
  - the coordinator's heads, credits, links, owners and revocations live in memory: only the
    token mark survives a restart, and rebuilding the rest is the wiring batch's work, for
    which the API has no restore call yet;
  - the simulator's oracle does not check the Move rule itself (that a hand-over leaves its
    holder a shard and never takes the shard the holder trains): its snapshot of the holders is
    taken once per tick, so a source that completed a shard in the same tick looks emptied, and
    a check on it gave false alarms in 410 of 6,600 runs. Those properties are held by the unit
    tests of the hand-over and by their mutants [V];
  - the progress deadline is a run parameter and must exceed the slowest honest link, which the
    simulator checks for its own configurations; it asks for one link, not for a pace, so a
    holder can keep a shard with one step a window;
  - a coordinator that is called from several threads is not supported, and a `Departure` is
    recorded only for a member that was live: a `withdraw` of the link of a member that had
    already left leaves only its penalty, so the wiring persists what `withdraw` returns.

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

- The Lens review of `v3-core-eviction`, `livrables/lens/2026-10-26_cortex-core-eviction.md` and its
  session report (F1 to F7 of the 2026-10-27 amendment).
- Code facts: this branch at `ac4c4dec` and later, read 2026-10-26, and the fixes branch on top
  of `0a5d431d`, read 2026-10-27. The research and review
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
