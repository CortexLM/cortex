# ADR 0011: audit integrity (settlement of blockages, numerics pin, host and image binding)

- Status: draft for branch `v3-audit-integrity`, 2026-10-27 (Principal SN100 D35, D36 bis, D38, D38 ter,
  D38 quater, D40, D43, D44, D46, D47, D49, D51; Lens red team B findings DSP-01, AUD-01, AUD-06, DET-01,
  DET-01b, QUAL-01, QUAL-02, EXE-01; Lens C1/C2 and final synthesis F2 R-C2/R-D, F5, DSP-04). Part 8 is
  the one rule ("general exclusion exit") of which Parts 1, 2 and 5 are rows. Forge decides promotion to `v3`. This file grows one part per finding; each part is
  committed before the code it justifies. Parts present so far are listed under "Parts".
- Order: Forge briefs `v3-audit-integrity` and `v3-admission-hardening` (same rules: ADR first, no
  protocol constant change without the ADR, manifest-covered changes opt-in, hash consequences
  listed, reproducer turned into a regression test that is red before and green after, with a mutant).
  Order of work: DSP-01, AUD-01, DET-01b/DET-01, QUAL-01/QUAL-02, EXE-01, AUD-06.
- Labels: **[V]** verified in code or by a command on 2026-10-27 at base `65be1962`, **[A]** reported
  by a cited document or session, **[H]** hypothesis, **[?]** unknown. Paths are relative to
  `hypertrain/src/hypertrain/` unless they start with `docs/`, `tests/` or `scripts/`.
- Not in this lot: RTA-05 and RTA-06 (`v3-admission-hardening`), the audit-cheater suite
  (`v3-audit-cheaters`), and every economic parameter (bond release, slash, price of a contest).

## Parts

1. DSP-01: disputes that nobody can decide.
2. AUD-01: audit liveness (lease expiry, executor failure, exhaustion).
3. DET-01, DET-01b: process-environment pin of island workers.
4. QUAL-01, QUAL-02: host and image binding of the CUDA qualification.
5. AUD-06: a roster member that never commits.
6. EXE-01: host-controlled bytes cannot crash the CUDA executor.
7. DSP-03: one lock order for the dispute long poll.
8. The general exclusion exit: every wait on a peer is bounded; exclusion, sanction, retired round,
   withdrawal record, roster member state (D43, D44, D46, D47, D51, AC-CLIFF, R-C2, R-D, DSP-04).

## Part 1: disputes that nobody can decide (DSP-01)

- Nothing in this part changes a manifest, a wire format, a protocol constant, an economic
  parameter or `implementation_hash`. The multiplier `ABANDONMENT_GRACE` is a default that awaits
  Principal sign-off.

### Context

- Time is the beacon table. `ChallengeStore._now` is `MAX(round)` (`challenge/store.py:612-614`); its
  only writer is `push_beacon` (`:707-755`), fed by `POST /v1/admin/beacon` from the external loop
  `python -m hypertrain.beacon push` (`beacon/push.py:94-141`). The server has no timer. [V]
- A funded contest fixes its horizon at `open()`: `absolute_deadline = now + 2*T*(sum(ceil(log2 span)) +
  3)` with `T = dispute_per_level`, and each move sets `turn_deadline = min(now + T, absolute_deadline)`
  (`challenge/disputes_v2.py:308-337,464-467`). With spans 1/8/74 and T = 600 the horizon is 15,600
  rounds (about 13 h at 3 s). [V]
- An open dispute pins the run on four sides: `finalize_v2` -> `finality_v2.settle` refuses
  `unresolved_disputes` (`finality_v2.py:68-79`); `require_open` blocks `w+2` (`:41-57`);
  `rollback_v2` refuses (`store.py:8999`); an included member with an open contest is marked pending
  (`:7883-7898`) and raises `TapeError` in `aggregate` (`aggregator/tape_v2.py:152-153`). [V]
- Nothing ends it. [V]
  - `referee_v2` convicts only on a paused final op with both signed outputs; on any other shape it
    answers 409 "mismatch contest must reach exact signed final-op outputs" for a non-`MATCH`
    verdict (`:8860,8902-8905`). It also refuses at `now >= absolute_deadline` (`:8539`).
  - `timeout()` needs a delivered, acknowledged turn and two witnesses at `now`
    (`disputes_v2.py:716-758`). The only `BisectV2` constructor in `src` is the miner's
    (`miner/dispute_watch.py:226`); no auditor-side responder exists, so the turn waits unanswered.
  - `DisputesV2.exclude` (`:782-815`) is the only server-side `INFRASTRUCTURE` resolution and has no
    caller in `src`.
- Reproduced on the base tree: the Lens S2 test (honest leaves, forged `delta_hash`) passes in 163 s:
  `paused=True level=step interval=(0,1)`, referee 409, `unresolved_disputes = 1`, 100 units stay
  `dispute_locked`; 50 rounds past `absolute_deadline` change nothing. The cheapest variant (forge,
  open the contest, never answer) freezes the same 15,600 rounds. One identity chained three such
  contests, each frozen for a full horizon, each returning the bond (critic, measured). [V]
- The audit already excluded the forger: a non-`MATCH` verdict writes `excluded/UNVERIFIED`
  (`store.py:7600-7606`). `settle_lock` slashes only `FRAUD` with the miner as loser
  (`disputes_v2.py:886-901`). `Admission.resume` needs a non-fraud settlement and restarts at
  `clean_count = 0` (`admission.py:1029-1091`); no client calls `POST .../admission/recover`. [V]

### Decision

1. **The server closes the dispute; it does not convict.** The resolution is `ResolutionV2(reason=
   "INFRASTRUCTURE", loser=None)` from the existing `exclude`, then `settle_lock` releases the bond
   in full. The verdict stands, the audit's `UNVERIFIED` exclusion stays, nobody is slashed, struck
   or banned (D40). **This is a deliberate deviation from the literal D38 options (1) and (2):**
   no `PARTY_TIMEOUT` default judgment (it needs an ack a silent party withholds, and it is a rollback
   cause), no referee verdict on the commitments. The forger is excluded for the round and not
   sanctioned. Bond economics belong to RTA-06; a conviction before DET-01 would slash honest miners
   of another CPU class. Needs Principal sign-off.
2. **Trigger.** `push_beacon` calls `_close_blocked_disputes_v2(run_id)` for each instantiated run,
   after the maturation and lease block and in its own transaction. No lazy trigger in an `owned`
   function (they bind `implementation_hash`).
3. **Rules, in order, `DisputesV2.blocked_cause(turn, now)`.**
   - Horizon: `now > absolute_deadline`, any turn, no record read. Strict `>`: `resolution()` still
     accepts the referee's signed resolution at `now == absolute_deadline`.
   - Dead end: `paused and not at_final_op`. The referee cannot score it: off the final op
     `adjudicate` returns `MATCH` or raises, and `referee_v2` answers 409 on a non-`MATCH`
     verdict. A `MATCH` resolution has the same downstream effect as `INFRASTRUCTURE`. Closes at
     the next beacon.
   - Abandoned: unpaused and `now > turn_deadline + ABANDONMENT_GRACE * T`, regardless of acks. After
     `turn_deadline` no party can move; the operator keeps `(turn_deadline, turn_deadline + T]` for the
     witnessed `timeout()`. The reaper calls `pause()` first, then `exclude()`.
   - A paused turn on the final op stays with the referee until the horizon.
4. **Evidence.** `exclude(..., cause=)` sets `op_spec` to `bounded-infrastructure-exclusion` (default,
   today's bytes), `unadjudicable-paused-transcript` or `abandoned-turn`.
5. **Failure isolation.** One savepoint per dispute; any `Exception` is journaled as
   `records_v2 kind='dispute-exit-failed'` `{stage, error}` with no beacon field, rewritten only on
   change, so `_rollback_snapshot_v2` is stable under a persistent failure. A failure never reaches
   the beacon route.
6. **Admission.** After a closure, `Admission.resolve_pending` moves the miner from `SUSPENDED` to
   `PROBATION` (`RESUME_FRESH_PROBATION`, `clean_count 0`, strikes untouched), so an
   infrastructure fault never leaves an honest miner suspended (D40). A fresh probation is the price
   of a contest; restoring `ACTIVE` would make serial contests free.
7. **Sequencing.** DSP-01 first. AUD-01 and AUD-06 edit the block before the hook. Option B, a
   referee verdict on a commitment mismatch (`delta_hash`, `ef_out_hash`, `final_theta_hash`),
   waits for DET-01b and RTA-06.

### Consequences

- Freeze bound for a run, per dispute, from its last move: a dead end lasts one beacon round; an
  unpaused turn lasts at most `T` (its own deadline) plus `T` (the grace), about 1 h at 3 s for
  T = 600; only a paused final op the referee cannot score lasts the horizon. Before: the horizon for all of them, and no exit at all in
  production. Prototype: S2 closes at the next beacon, bond restored, `burned 0`, `unresolved_disputes
  1 -> 0`; a silent contest closes at `turn_deadline + T + 1`; a 5-miner run finalizes round 0 and
  opens round 2 right after the push. [V]
- `w+2` opens in two steps: the closure, then the coordinator's `finalize` for `w`. The Lens S2
  fixture hand-builds the round history and cannot show `w+2`; the 5-miner e2e does. [V]
- After a long beacon outage the first push closes every dispute past its horizon or grace at once
  (`beacon/push.py` catches up at most 20 rounds, so `_now` can leap). Money-safe: verdicts stand,
  bonds return. The operator's `timeout()` window is skipped in that case. [V by reading]
- Residual: serial contests. `open()` has no round-age check, a contest pins `w+2` for any
  unclosed round, and the auto-lift removes the manual `recover` between two contests. One identity
  can stall the run about `2T` per contestable verdict at the cost of its own probation. RTA-06 prices
  contests; until then this is accepted (open question for Forge in the lot report). [V for the mechanics, H for the economics]
- The honest miner of a false rejection (DET-01b) keeps its bond but loses the round reward and
  restarts probation. That cost belongs to DET-01b. [V]
- A referee run that overlaps a closure may finish and register evidence that no resolution can use
  (`RESOLUTION_CONFLICT`). Harmless, wasted work. [V by reading]

### Rejected alternatives

1. **PARTY_TIMEOUT against the silent party (D38 option 1):** needs an ack the attacker withholds;
   books an eviction as miner fault against D40 and ADR 0002 invariant I5; a rollback cause.
2. **Referee verdict on commitments (D38 option 2):** the referee replays in the challenge
   environment; DET-01/DET-01b would convict and slash an honest miner; `referee_v2` is `owned`.
3. **Close at `turn_deadline`:** pre-empts the designed `timeout()` window used by
   `tests/e2e/test_service_network_v2_e2e.py:937-960`.
4. **Horizon only:** the 15,600-round freeze stays for the cheapest variant.
5. **Ack-based abandonment:** an attacker who acks then goes silent defeats it.
6. **Relax `exclude()` instead of `pause()`:** changes a contract that two tests rely on.
7. **Verdict-dependent dead end:** adds a record read that can block the horizon (reproduced); `MATCH`
   closes to the same effect.
8. **Lazy triggers in `open_round_v2`/`finalize_v2`/`aggregate_v2`:** change `implementation_hash`;
   the beacon is the only clock.
9. **Wall-clock timer thread:** wall-clock must not decide protocol state.
10. **Restore `ACTIVE` on closure:** makes serial contests free.
11. **Cap one open contest per miner:** already enforced (`disputes_v2.py:289-292`).

### Not verified

- No CUDA run; the production `T`, spans and worker count (the manifest is not in Git) [?].
- The reaper visits only runs instantiated in the process that handles the push; after a restart the
  first route that calls `_services` re-arms it, as for lease expiry [?].
- A DET-01b contest end to end, and a contest with an auditor-side responder (none exists) [H].
- The `ABANDONMENT_GRACE` multiplier; the e2e in CI (the `test-slow` job is skipped on `v3-*` PRs).

## Part 2: audit liveness (AUD-01)

- Lens red team B finding AUD-01 (IMPORTANT) with Principal D38 ter ("the finality must distinguish
  auditor/host fault from miner fault: an audit failure must not exclude a miner by default").
  No manifest, policy value, wire format or run id changes. Decision 8 is isolated in one commit.
  `implementation_hash` changes (`lease_v2` and `execute_audit_v2` are owned names).

### Context

- Mechanics. `schedule_audits_v2` (`challenge/store.py:6295`) queues one lease per roster miner with
  `absolute = min(d_final, d_audit + 2 * lease_rounds)` (`:6396`). `lease_v2` (`:6448`) gives a queued job to an allowlisted
  auditor: one running lease per auditor (`:6562`), at most two running, never the same auditor twice on a job,
  `expires = min(now + lease_rounds, absolute)`. Time is the verified beacon (`_now`, `:612`). [V]
- Permanent disable. `_expire_leases_v2` (`:6629`) writes `auditor-disabled` at every expiry (`:6640`); `lease_v2` then
  answers 403 (`:6554-6558`) and `relay_observer_v2` refuses the observer (`:5308-5312`). Three references in the repository,
  no deleter, no test, no document. The manifest requires >= 2 auditors, so two events end audit liveness. [V]
- Exhaustion poisons the round. With `attempts >= 2` or `now >= absolute` the lease is `exhausted`, the miner gets
  `excluded/INFRASTRUCTURE` and the round's finality becomes `EXCLUDED` (`:6652-6672`). `finality_v2.settle` refuses
  `EXCLUDED` (`finality_v2.py:74`), relay settlement needs `SETTLED` (`:8486-8492`), and `rollback_v2` repairs only referee-signed
  causes (`:9226`). A MISMATCH, by contrast, drops only its miner (`excluded/UNVERIFIED`, `:7600-7606`). [V]
- Executor exceptions. `execute_audit_v2` (`:6897`) has `finally` only (`:7335-7336`): an exception leaves the lease `running`
  until expiry. The auditor client exits 2 on a 500 (`auditor/__main__.py:101,178`) and its restart meets
  `409 one live lease per auditor` until then. Lens S3 reproduces: 1 passed in 152 s; `RecursionError`, lease `running`,
  later `exhausted`, one `auditor-disabled`, one `excluded`, finality `EXCLUDED`. [V]
- Single window. `lease_v2` dates a job `created_beacon = row["created"]` (`:6618`) and `AuditJobV2` caps
  `lease_expires <= created_beacon + 100` for H <= 30 (`protocol/messages_v2.py:450`). `scripts/ops/live_operator.py:123-133`
  therefore refuses `d_final - d_audit > 100`, and every lease of an operator-built round runs to `absolute` (lease 21 -> 120):
  a crashed or hung auditor is never replaced in that shape. A lease requested after `d_audit` gets 409 for wider gaps. [V]
- Head of line. A miner seals its `CommitV2` with any `exp_drand` (intake checks only the commit window, `:6162`) and its leaves
  with any finite-or-not f32 metric (`leaves_v2` checks root and count, `:6255-6288`). Either makes `AuditJobV2` raise inside
  `lease_v2` for the first queued row, which is never skipped: both auditors get 409 on every poll while valid jobs wait
  until `absolute`. Reproduced for the expiry (adversarial review) and for a NaN loss (this design). [V]
- Concurrency. Two `execute_audit_v2` calls for one lease: the second registers its guard (`:7149`), then dies with
  `FileExistsError` outside the `try`, leaving an orphan guard that replaces the running one; and `finally: pop(job_id)` can
  release the next attempt's guard. [V by reproduction]
- Stranding. `open_round_v2` restores the w-1 anchor of every roster entry (`:5155`); anchors come from genesis, a MATCH
  replay (`:7319`) and rollback repair, so a miner left out of round w cannot be in round w+1. A round needs >= 4 verified
  inputs (Q/4 cap; `tape_v2.py:88`, `messages_v2.py:728,735`). The generic `verified_inputs_v2` skips `excluded` miners and
  demands a `replay` record from the others (`:7875-7906`); the service branch demands the complete roster. [V by reading]
- `anchor_age`. `AuditJobV2.anchor_age <= 1`, `max_anchor_age_rounds = 1` and `audit_island_chain` (`auditor/replay.py:519-560`)
  replay one missing predecessor under a 2H budget, but only tests call it and `lease_v2` hard-codes `anchor_age=0` (`:6609`). [V]

### Decision

1. Cooldown, not a ban: `auditor-cooldown` record, one lease then two, strike forgotten after a quiet lease, never refuses the
   last configured auditor outside a cooldown; legacy `auditor-disabled` rows are inert. The cooldown starts at the
   `expires` of the lease that lapsed, not at the round that observes the lapse: `lease_v2` and `relay_observer_v2`
   refuse by raising, which rolls back their own lazy expiry, so anchoring on the observing round would let a polling
   auditor push its own release date forward at every poll.
2. Cause per attempt: `AUDITOR_SILENT` (cooldown), `EXECUTOR_INTERRUPTED`, `EXECUTOR_FAILED` (no cooldown). The auditor is
   silent only if it never asked for the execution: `audit-execution` is committed `REQUESTED` before the preparation can refuse.
3. One retire path; today's exhaustion block moves verbatim into `_exhaust_audit_job_v2`, the only place Decision 8 touches.
4. An exception leaving the executor boundary fails the attempt at once (re-raised, class name only) unless the guard is
   cancelled. Circuit breaker: the same error class on a second job within `lease_rounds` is held to the lease expiry.
5. Execution is single-flight (409), the guard is registered last and released by identity.
6. `created_beacon = now`; `live_operator` accepts any gap up to the 200-round cap (A1 option a).
7. A job whose miner-sealed inputs cannot be leased (`COMMIT_EXPIRED`, `LEAVES_NONFINITE`) is dropped as the miner's fault
   (`excluded/UNVERIFIED`) and the queue moves on. Other validation errors stay loud.
8. An exhausted job is re-armed once (option O2 of the open question on exhaustion, chosen for this lot
   because D38 ter says an audit failure must not exclude a miner by default; isolated in one commit so
   Forge can swap it for dropping at once or for today's behaviour); a second exhaustion drops its miner from that round (`excluded/INFRASTRUCTURE`,
   `unresolved_audits - 1`). The round disposition is never touched. Service-admitted runs skip the re-arm.
   The second window ends where the miner's sealed commit ends: `absolute = min(now + 2 * lease_rounds,
   exp_drand)`, and no window is opened when that leaves fewer than two rounds (AUD-07, Lens verdict
   2026-10-27 16:05Z). `AuditJobV2` embeds the commit live (`validate_embedded`) and an honest miner
   seals it until `d_final` (`miner/core.py:1274`), so a window opened at `d_final` could only end in
   `COMMIT_EXPIRED`, a drop booked as the miner's fault (`UNVERIFIED`) for the silence of the auditors.
9. INFRASTRUCTURE is never a round disposition. No sanction, strike or suspension is added.

### Consequences

- One failing host or auditor costs one lease of one auditor; after a second failed window it costs one miner one round.
  It no longer silences the auditors for good or voids the round for everybody.
- Total audit time per job doubles: a job nobody audits holds its round (verified inputs, w+2 opening, rollback, relay
  settlement) for up to `2 * lease_rounds` more beacon rounds (H <= 30: 200 rounds, 10 min; policy lease at H = 512: 2932
  rounds, 2.4 h) before it is resolved. Nothing needs an operator.
- A miner dropped after two windows is stranded for the rest of the run (`open_round_v2` needs its anchor). This lot does not change
  that. Recovery would need the `anchor_age = 1` chain (`audit_island_chain`) wired into `open_round_v2`, `lease_v2` and the
  worker: a protocol-level change, not part of this lot (option O3 of the exhaustion question).
- The public view labels a dropped miner `EXCLUDED_V2`, a fault (`public_api/views.py:15,118`), as MISMATCH drops already are (open question for the frontend owner).
- A 4-miner roster cannot absorb any drop (>= 4 inputs); a service-admitted run cannot lease today (heavy gate) and needs its complete roster.
- The auditor process exits 2 on the cooldown 403 and on a 500; its supervisor should restart it with a delay.
- The first lease of an operator-built round with a gap of 100 is still the only lease of that window; gap 101-200 gives a
  replacement inside the window. Decision 8 gives a second window only to a job that is exhausted before the
  miner's commit expires (attempts that fail early, window shorter than the round). In the operated shape, where the first
  window ends at `d_final`, a job nobody finished costs its miner the round at once, as `INFRASTRUCTURE`. Making that
  window real would need miners to seal commits past `d_final` (`miner/core.py`), a miner-side change this lot does not make.

### Rejected alternatives

An operator re-enable route (no automatic liveness, the 403 text promises a review nobody implements); a column or table
(`store.py` has no migration facility; `records_v2` is the idiom); a wall-clock timeout (the store is beacon-time); a new
`AuditPolicyV2` field for the lengths (manifest-covered, derived values suffice); catching only `IslandFailure` (S3 raises
`RecursionError`); wrapping all of `execute_audit_v2` or wrapping it outside (would discard a good verdict on a beacon pause,
bypassable); refunding the attempt (`max_attempts` is `Literal[2]`); relaxing `AuditJobV2._bounds` (protocol model change);
clamping `absolute` at scheduling (kills the second lease of every H <= 30 round); a round-wide `EXCLUDED` for
infrastructure (O4: one poison job or one outage voids everybody's reward and the chain stalls); including the unaudited miner
(pays a cheater who causes an outage); re-arming without a limit (the literal reading of D38 ter: a poison job would hold the
round and block w+2 for ever); a pause before the re-arm (the window itself is the pause for time-based exhaustion, the
breaker bounds the attempt-based one).

### Not verified

- No real executor host, GPU or auditor process: executor failures are injected by replacing `worker.launch_island`. [?]
- No round of five or more miners was aggregated and settled with one dropped miner; the generic path is argued from
  `verified_inputs_v2` skipping `excluded` records and from `live_operator.included`. [?]
- Service-admitted runs: neither the re-arm (skipped by construction) nor the lease path were exercised. [?]
- `complete_audit_v2` with the stale nonce of a re-armed lease (argued from the nonce check, not run). [?]
- A deployed run's `service-admission` records; the frontend schema for miner statuses; supervisor restart behaviour on exit 2. [?]
- `leaves_v2` end to end with a NaN metric (the records are written directly by the harness; the code path is read). [?]

### Sources

`hypertrain/src/hypertrain/challenge/{store,finality_v2}.py`, `protocol/messages_v2.py`, `auditor/{replay,__main__}.py`,
`scripts/ops/live_operator.py`, `docs/s100-test-stack.md`, 2026-10-27. Lens red team B `rtB_audit_store_attacks.py::test_S3_*`,
2026-10-27 [A]. Adversarial review of the first AUD-01 design, 2026-10-27 [A]. S7 s100 TEST notes [A].

## Part 3: process-environment pin of island workers (DET-01, DET-01b)

- Lens red team B findings DET-01 and DET-01b (IMPORTANT) with Principal D36 bis and D38. Opt-in: a
  manifest without the new field keeps today's bytes, run_id, goldens and behaviour. No wire message,
  protocol constant or economic parameter changes. `implementation_hash` changes (the lot-level hash
  table lists when). Host of every measurement: AMD EPYC 9575F (Zen 5, avx2 + avx512), torch 2.14.0+cpu,
  MKL 2024.2, oneDNN 3.12, no GPU.

### Context

- The island environment is the launcher's `os.environ` plus five fixed keys (`miner/island_launch.py:
  608-616,658-666`); the repair and history workers copy it (`aggregator/rollback_v2.py:157-159,
  290-292`). The manifest pins only `CUBLAS_WORKSPACE_CONFIG`, `CUDA_DISABLE_PTX_JIT` and `cpu_threads`
  (`protocol/messages.py:267-270`, `trainer/determinism.py:17-34`). [V]
- DET-01 reproduces: kimi MICRO, one round, fresh process per variant, gives 5 distinct (leaves, delta,
  state) triples for fp32 over the 7 Lens variants and 3 for bf16; 12 ambient variants give 6 (fp32), 3
  (bf16), 6 (tiny decoder). DET-01b reproduces through the production comparator: a two-rank torchrun
  claim minted on AVX512 and audited on AVX2, and the reverse, give MISMATCH `first_bad_leaf=1`; equal
  environments MATCH. What differs is mostly the optimizer moments in `stages`, not loss/norm. [V]
- Necessity per compute dtype (kimi MICRO, the 8 subsets of the three CPU pins): fp32 depends on
  `ATEN_CPU_CAPABILITY` and `MKL_CBWR`, not on `ONEDNN_MAX_CPU_ISA`; bf16 on `ONEDNN_MAX_CPU_ISA` only;
  the eight loop configs agree. Also changing roots: `HT_ATTN_BACKEND=sdpa-flash`, `HT_FP8=1`,
  `OMP_DYNAMIC=true` (training loop, 2 and 4 threads on one core) and `OMP_THREAD_LIMIT=2`/`3` at
  `cpu_threads`=4, where `torch.get_num_threads()` still says 4 and `require_threads` cannot see it.
  Thirty-eight further variables (`MKL_ENABLE_INSTRUCTIONS`, `DNNL_*`, `GLIBC_TUNABLES`, `KMP_*`, other
  `OMP_*`, thread counts) changed nothing under the pin. Thread variables are inert only at
  `cpu_threads`=1, which every manifest uses today. [V]
- Under the full pin this AVX-512 host gives the AVX2-bound goldens (kimi fp32 `7f01e9f9...`, bf16
  `684efad6...`; loops `ba33f946...`, `12a144e0...`, `78787023...`, `1c281757...`), which the test
  files attribute to the GitHub runner [A]. An Intel Xeon Platinum 8573C CI run with the same trio
  produced roots outside GOLDEN [A: `livrables/forge/01-inconnus.md`, run 39334967275]. [V, A]
- The miner replays CPU publications for dispute answers in its own daemon (`miner/dispute_follow.py:
  80-85`). Minted under the pin and replayed under the default ambient it fails
  `REPLAY_COMMITMENT_MISMATCH`, which `follow()` propagates ("skipping forfeits"): pinning only the
  launcher would create this defect for honest default miners. [V]
- On the base tree a refusal escaping `execute_audit_v2` is an HTTP 500 with no verdict, and
  `_expire_leases_v2` (`store.py:6629-6672`) books `auditor-disabled` and, at exhaustion,
  `excluded/INFRASTRUCTURE` for the miner with `disposition = EXCLUDED`. An OPEN trial past its
  deadline becomes `SUSPENDED CHALLENGE_EXPIRED` (`admission.py:221-224`). Nothing convicts, but the
  party loses its round or its admission. [V]
- `compress`/`decompress` bytes are equal under ATen AVX2 and AVX512 and differ under
  `ATEN_CPU_CAPABILITY=default`; the capacity worker (genesis, outer) runs in a fresh environment. [V]

### Decision

1. Optional `reference_spec.env.pinned = {present: {NAME: VALUE}, absent: [NAME]}` over a closed,
   append-only, measured-values-only table in the torch-free module `protocol/env_pins.py`: present
   `ATEN_CPU_CAPABILITY=avx2`, `HT_ATTN_BACKEND` math|sdpa-flash, `MKL_CBWR=AVX2`, `OMP_DYNAMIC=FALSE`,
   `ONEDNN_MAX_CPU_ISA=AVX2`; absent `HT_COMPILE`, `HT_FP8`, `HT_MOE_GROUPED`, `OMP_THREAD_LIMIT`. A
   manifest constrains exactly what it names; no required set, no version. Appending never invalidates
   a signed manifest; removing or narrowing is forbidden (snapshot test) and every `HT_*` read of the
   trainer must be in the table or in a reasoned "unpinned on purpose" map. The table is documented
   in `docs/protocol.md` because the JSON schema cannot express it.
2. Presets by compute dtype, kept outside `src`: `CPU_FP32` = ATEN avx2, MKL_CBWR AVX2, math attention,
   OMP_DYNAMIC FALSE, the four absent names; `CPU_BF16` = `CPU_FP32` + `ONEDNN_MAX_CPU_ISA=AVX2`.
3. Enforcement: `TrainConfig` carries the pin; `require_reference_env(cfg)` (threads + pin) replaces
   `require_threads` at the three train entry points, `apply_reference_env` checks it before
   `init_process_group`. The check reads the pinned names and ATen's effective capability. v1
   manifests carrying the field are enforced, not rejected (rejection breaks v2 training bodies
   converted through the v1 model, `trainer/config.py:145`).
4. Launch: one builder `island_env` for `launch_island` and `launch_window`; `_launch_unbounded`
   re-applies the pin itself so direct callers (the GPU operation script, tests) cannot forget it;
   the rollback workers use the same function.
5. Dispute follower: a pinned publication is answered from the actual-path trace, not from an
   in-process replay in a daemon whose environment need not honor the pin; the referee side is
   actual-path too.
6. Preflight: a cached child probe under the pinned environment answers "can this host honor the
   pin". The parent raises `EnvPinRefused` before any capacity is charged or rank started, at miner
   `join` and `probe`, at the CPU service bootstrap (`SERVICE_PROFILE_ENV_PIN`), and from a CLI.
7. A refusal is an executor fault, never evidence about the miner's work; it relies on AUD-01
   decision 3 (fail fast, `EXECUTOR_FAILURE`, no quarantine). Honor system: a miner running modified
   code (still caught by its roots), `LD_PRELOAD`, `sitecustomize`, anything outside the table.
8. CUDA: nothing verified. The pin reaches the executor path through `_launch_unbounded`; the field
   must not be used on a CUDA production manifest before the qualification run carries it.

### Consequences

- Removes ambient-induced divergence: the reproducers go from 6/3/6 distinct root triples to 1/1/1 and
  from MISMATCH to MATCH. It does NOT remove hardware-class divergence: the Intel 8573C case would still
  be convicted, with no refusal, because the check only catches pins the host cannot honor. Measured
  here: Zen 5 pinned equals the goldens attributed to the GitHub runner [A]; no other class was run.
  Cross-vendor equality is not claimed.
- Throughput. `ONEDNN_MAX_CPU_ISA=AVX2` alone (1 thread): bf16 1024^3 matmul 575 -> 1.4 GFLOPS (x358),
  `F.linear` 581 -> 32 (x18), attention-shaped bmm x74; at n=64..512 linear x11..27, matmul x15..181,
  bmm x39..79; fp32 unchanged (129 vs 126, linear 134 vs 125). `ATEN_CPU_CAPABILITY=avx2` and
  `MKL_CBWR=AVX2` cost nothing on this host. The kimi MICRO round does not notice (1.8 s vs 1.7 s). A
  bf16 CPU manifest above toy size pays one to two orders of magnitude; fp32 CPU manifests, which
  include the only CPU service profile, pay nothing. Hence two presets.
- An honest default miner stays honest: its children are pinned, its dispute answers do not depend on
  its daemon environment, and a host that cannot honor the pin is told at `join`, not after paying for
  admission. Until AUD-01 (lease expiry -> `auditor-disabled`, miner `excluded/INFRASTRUCTURE`) and D40
  (trial expiry -> `SUSPENDED CHALLENGE_EXPIRED`) land, a host that cannot honor the pin still
  penalizes the party that ran into it; the preflight is the control. In-process users (honeypot, v1
  flows) must run under the pin or refuse.
- A pinned CUDA manifest cannot be qualified until the qualification run carries the pin (fails closed).
- `implementation_hash` changes; runs holding a `service-admission` record then fail
  `SERVICE_PROFILE_AUTHORITY` (`store.py:4631`) and cannot be re-bootstrapped after a round opened.
- `MKL_CBWR=AVX2` falls back to AUTO off Intel [A: oneMKL "Specifying Code Branches"]; `COMPATIBLE` is
  the documented portable branch at the cost of SSE2-only GEMM. It is not in the table yet: it enters
  with a measurement on a second CPU class.

### Rejected alternatives

1. `reference_spec.env.numerics`, 8 required keys: a new required key invalidates signed manifests;
   cannot say "unset"; scrubs whole prefix families (`MKL_NUM_THREADS` is set by `Dockerfile.gpu`).
2. Always-on pins: changes every CPU root and every golden; the two root sets in `GOLDEN` are deployed.
3. Named profile `Literal`: the manifest bytes bind a name, not the values.
4. Worker re-exec from `job.json` before importing torch: silently corrects instead of refusing, cannot
   cover `rollback_v2._context_worker`, and hides a launcher change from `implementation_hash`.
5. Setting the variables inside `apply_reference_env`: ATen, MKL and oneDNN read them at first use.
6. Static preflight (`platform.machine`, `/proc/cpuinfo` flags): duplicates ATen's own detection.
7. Replaying the dispute in a pinned child (new module, a second torch process per round): same bytes
   as the referee's actual-path re-run for more machinery; the fallback.
8. `IslandFailure` subclass classified by stderr substring: brittle (torchrun exits 1) and echoing.
9. Presets as constants in the protocol module: unused by production code and bakes in MKL_CBWR.
10. Attestation of the miner environment at admission: self-reported by the party being checked.

### Not verified

- A second CPU class (no Intel, no older AMD, no ARM here); Intel vs AMD under `MKL_CBWR=AVX2`.
- Everything CUDA (executor, `HT_ATTN_BACKEND` on sm_103, NCCL, TF32 override effects).
- `HT_COMPILE=1` (no C++ compiler here); `HT_MOE_GROUPED=1` showed no effect on the CPU configs.
- MKL CNR and oneDNN ISA state is not observable: the check proves the environment string and ATen's
  capability; an environment mutated after library initialisation is not detected.
- The repair/history torchrun path with pins, the capacity worker's outer path on a DEFAULT-class host,
  and the challenge path end to end with AUD-01 (read, not run).

## Part 4: host and image binding of the CUDA qualification (QUAL-01, QUAL-02)

- Lens red team B findings QUAL-01 and QUAL-02 (IMPORTANT) with Principal D36 bis and D38 quater. No
  manifest, schema, run id, golden, constant or installed record changes. No QUAL commit changes
  `implementation_hash` by itself (the gate and the dispatch check live outside the owned names). No code
  of this part ran on a GPU host.

### Context

- `network_qualification.compare` (`src/hypertrain/gpu_ops/network_qualification.py:507-555`) accepts two
  results when `machine_id`, `instance_id` and `role` differ and the profile, sources, image digest and
  rounds are equal. All of these are labels written into files by whoever holds the files. A byte copy of
  one host's result with three labels rewritten returns `{'two_host_bitwise_match': True}`; `drivers`,
  `torch_path`, `config_sha256` and `elapsed_seconds` can be changed afterwards and are never compared.
  `Environment` (`:141-173`) holds no device UUID, host key or signature. [V: Lens `rtB_qualification_relabel.py`
  5 passed; a scratch old-API test fails with `DID NOT RAISE Reject`]
- The production gate `_bootstrap_backend_v2` (`challenge/store.py:2376-2485`) re-derives nothing: it checks
  each result against its own config, calls `compare`, and the owner `Receipt` signs the digest of 7
  evidence keys. `_backend_v2` (`:2487-2534`) re-reads only that evidence (ADR 0007). [V]
- The executor derives `machine_id` from the GPU UUIDs the host prints (`cuda_executor.py:508-519`): one box
  under two labels that prints fresh UUIDs is admitted as two machines (Lens X4). [V]
- Qualification measures `cfg.tree` (`PYTHONPATH=cfg.tree/src`, `network_qualification.py:436-443`); audits
  run the package installed in the image (`docker/Dockerfile.gpu:37`). `HT_IMAGE_DIGEST` is an environment
  echo (`miner/island_worker.py:50`). Nothing links the sources a result claims to the code the audited
  image runs. [V]
- `Qualification.known_hosts_sha256` already exists, is compared with the lifecycle receipt
  (`network_qualification.py:136,282`) and is filled with `fsha(ssh.known_hosts)` by the orchestrator
  (`experiments/gpu_network_v2/orchestrate.py:3797-3829`). [V]
- The executor probe holds no boot id and no instance evidence (`cuda_executor.py:201-210`). The SSH
  channel, authenticated by the operator's pinned `known_hosts`, is the strongest machine handle the
  challenge side has. [V] No non-Vast tool in the repository writes a qualification config (ADR 0007). [A]

### Decision

1. **Host statement.** Each host signs, with its SSH host key, a canonical JSON statement
   `{version, config_sha256, result_sha256, image_digest, gpu_uuids, drivers, boot_id}` (namespace
   `hypertrain-qualification-v1`). `scripts/ops/qualification_attest.py` runs on the operator machine over the
   host's pinned SSH channel (`[sudo -n] ssh-keygen -Y sign -f /etc/ssh/ssh_host_ed25519_key`), verifies its
   output against the pin, and writes `attestation.json` and `attestation.sig` beside `result.json`. Nothing
   is staged on the host.
2. **`compare(paths, manifest, *, host_keys)`.** The keyword is required. `None` is the explicit label-only
   mode (item 7). With keys, `compare` also requires per host: a valid signature under the pin
   (`ssh-keygen -Y verify`, `namespaces=` in `allowed_signers`), a canonical statement, `result_sha256`,
   `config_sha256` and `image_digest` equal to the result's, the result's GPU UUIDs a subset of the
   attested ones, and across hosts different pins, disjoint GPU sets and different `boot_id`.
3. **Pins.** A pin is the single `ssh-ed25519` key of the operator's `known_hosts` file for the host. The
   file must hash to `Qualification.known_hosts_sha256` and its `SHA256:` fingerprint must equal the one the
   owner typed from the provider console (`backend_bootstrap.py --host-fingerprint`). The pin and the
   fingerprints are recorded in the evidence; nothing in the repository verifies where the owner got them.
4. **Gate.** `_bootstrap_backend_v2(..., *, pins)` takes the pins as a required keyword (`None` = label-only).
   One non-owned builder, `ChallengeStore._qualification_evidence_v2`, serves the store and
   `scripts/ops/backend_bootstrap.py`. Strict evidence adds `host_attestation` (fingerprints and sha256 of the
   four files); the four files enter the object store. Legacy results with `pins=None` give today's 7 keys.
5. **Image binding (QUAL-02).** `cuda_executor_runner` (stdlib only) measures the package installed in the
   image with `python -I` (so `PYTHONPATH` and the cwd cannot substitute it): sha256 of the compact sorted
   JSON of `{"src/hypertrain/<rel>": sha256}` over every `*.py`, equal to the JCS form for the ASCII names
   the program enforces. `Environment.image_sources_sha256` records it. `run()` refuses to start when the
   `*.py` of `sources(tree)` differ from the measurement (`qualification_tree_is_not_the_image`), `compare`
   re-checks it against `result.sources`, and the value enters the evidence.
6. **Audit-time check.** `_dispatch_env_v2` compares the dispatch record's `image_sources_sha256` with the
   qualification record's (409 on a difference, 503 when absent); a record without the field adds no check.
   The executor reports the measurement, and an optional pin in `ExecutorConfig` refuses a host that reports
   another value, so reference and referee runs are covered too. The executor config also refuses two hosts
   that pin the same SSH key or the same address and port, and `Executor._admit` refuses overlapping GPU sets.
7. **Legacy and Vast.** Installed records stay valid. Strict is the default for new bootstraps; label-only
   (`--label-only`, `pins=None`, `host_keys=None`) is the explicit switch and remains the route of the Vast
   lifecycle (`launch_cuda_run.sh`, `orchestrate.py:2303,3338`), whose `machine_id` comes from the provider
   listing and is checked distinct by the owner-run admission (`launcher.py:220-222`) [A]. A label-only record is not marked: its evidence is byte-identical to today's.
   An image built before this change cannot run the new `run()`; it can only be qualified label-only.
8. **Stated limits.** The gate protects the engine path against accidental or lazy relabelling (a relabelled
   copy, a stale or foreign attestation, one machine listed twice). It does not bind a malicious owner:
   an operator who also forges a second key and a matching inventory, the TEST stand-in
   (`scripts/ops/s100_test/cuda_qualification.py`) and the owner key can still install any record (QUAL-03).
   A compromised host can sign anything with its own key. The signature proves that the holder of the pinned
   key agreed to the statement, not where the computation ran. The in-image digest covers the package `*.py`
   only and is produced by the image's own interpreter on an operator-controlled host: it detects drift and
   mistakes, not a malicious host. Package data, dependencies, torch and the base image are bound by the
   image digest alone. The numbers 16 GPUs, 4 KiB, 2 KiB and 10 s are local limits with no consensus effect.

### Consequences

- A strict CUDA backend record can no longer come from one host relabelled twice, nor from a tree that
  differs from the audited image, unless the operator forges a second key and inventory.
- The server image must contain this change to parse new results (`extra="forbid"`) and carries
  `openssh-client` already (`docker/Dockerfile.server:41`). The GPU image must be rebuilt from a tree that
  contains this change for strict or tree-bound qualification; image `4e5f4b7e` cannot.
- Every change to a `src/hypertrain/**/*.py` file now needs a rebuilt GPU image before qualification
  (`qualification_tree_is_not_the_image`); this was already implied by the image digest pin.
- `sources()` changes in six files, so older qualification configs no longer validate against the tree and
  dispatch source pins must be recomputed (ADR 0007 and ADR 0008 precedent). Run ids and installed records
  do not change.
- Cost: one SSH round trip per host and about 6 ms of `ssh-keygen`.

### Rejected alternatives

1. Host-printed identity inside `result.json` (UUIDs, boot id): the party being checked writes it (Lens Q1, Q2, X4).
2. A host-side program that builds and signs the statement: stages code on a machine whose output is the
   thing under test; the operator-side tool uses only fixed commands.
3. A pure-Python Ed25519 and SSHSIG verifier: own cryptography to review, while `openssh-client` is already in
   the server image and `ssh-keygen` is used by three existing test modules.
4. Required EC2 identity-document evidence: AWS only, needs AWS's certificate [?], absent from the probe.
5. Expected image hash from the challenge's own installed package: server and GPU images are released
   independently (ADR 0008). From the registry manifest: it lists layers, not sources. From an operator
   config field: the party being checked writes it; the owner-signed record carries the value instead.
6. A baked `/opt/hypertrain/build.json`: lives inside the digest it vouches for. Hashing `uv.lock`: the
   tree's lock does not affect execution.
7. A default-label-only `compare`: the bare Lens Q1 call would keep passing. A marker key in label-only
   evidence: it would change every existing evidence digest.
8. Driving qualification through `cuda_executor`, so the challenge observes each host itself: the stronger
   long-term design, but it needs a new qualification driver. Left for later.

### Not verified

- No run on a GPU, AWS, Lium or Vast host. `sudo -n ssh-keygen -Y sign` on a real
  `/etc/ssh/ssh_host_ed25519_key` is [H]; whether the D34 toolbox writes the real `known_hosts_sha256` is [?].
- A Vast container's sshd key may be shared across instances [H]; strict Vast qualification is a follow-up.
- The in-image program matched `sources()` on a wheel of this tree and on the dev editable install (144 files,
  same digest). In the CI-built image it is [?].
- `ssh-keygen` under a uid without a passwd entry exits 255 with `No user exists for uid` [A: adversarial review]; not reproduced.
- Interaction with AUD-01 (a 409 after publication is booked as auditor silence) is not studied.

### Sources

- `hypertrain/src/hypertrain/gpu_ops/{network_qualification,cuda_executor,cuda_executor_runner}.py`,
  `challenge/store.py:2376-2534,4249-4322,7338-7377`, `miner/island_worker.py:45-70`,
  `experiments/gpu_network_v2/orchestrate.py:2303,3338,3797-3829`, `docker/Dockerfile.{gpu,server}`, 2026-10-27.
- Lens red team B report 2.1 to 2.3, `rtB_qualification_relabel.py`, `rtB_executor_host_attacks.py`, 2026-10-27 [A].
- Adversarial review of the first QUAL-01 design, 2026-10-27 [A].
- `docs/adr/0001`, `0007-sm-counts-per-arch.md`, `0008-generic-cuda-executor.md`.

## Part 5: a roster member that never commits (AUD-06)

- Lens red team B finding AUD-06 (IMPORTANT, documented in ADR 0002) with Principal D38 quater.
  Nothing in this part changes a manifest, a policy, a wire format, a protocol constant, an economic
  parameter or a run id. `implementation_hash` changes (owned names). Sanction S0 and the reward rule
  below await Principal sign-off.
- Sources, 2026-10-27: Lens red team B report 1.4; ADR 0002 (context rows, decisions 5 to 7, I5); the
  code at `65be1962`; throwaway tests and 5-identity runs on a scratch copy of `hypertrain/` with the
  fix applied; the adversarial review of the design.

### Context

- `schedule_audits_v2` reads the commit of every roster member and a missing one raises 409
  `missing accepted commit` for the whole round (`challenge/store.py:6385-6387`, raised by
  `_record_v2` at `:2151-2161`). No audit job exists for anyone and nothing times out. [V]
  Base tree, 5 roster entries and 4 commits: 409 at `d_audit` and again 500 rounds past
  `d_final`, no lease, finality `PENDING` with `unresolved_audits = 5`, no `excluded` record. In a
  real 5-identity round the `audits` call answers the same 409. [V]
- The absence is final at `d_commit`. `training_v2` takes accept and commit only for
  `d_assign <= now < d_commit` (`:6161-6162`), delta only for `now < d_upload` (`:6163`), and
  `RoundOpenV2` orders `d_commit < d_audit` strictly (`protocol/messages_v2.py:330-334`). [V]
- Kind `excluded` has one reader, `verified_inputs_v2`, and only for presence (`:7873-7879`; the
  operator tool mirrors it, `scripts/ops/live_operator.py:242-255`). `finalize_v2` pays the members
  that function returns, split by tokens (`:8292`, `:8342-8351`). `settle` refuses only
  disposition `EXCLUDED`, unresolved audits or disputes (`challenge/finality_v2.py:68-79`). An
  `UNVERIFIED` verdict already drops one member and leaves the disposition alone (`:7600-7606`). [V]
- A round needs four members whose caps carry Q. `TapeBodyV2.inputs` has `min_length=4`
  (`aggregator/tape_v2.py:88`), a member is capped at Q/4 (`protocol/messages_v2.py:638`) and all
  probation members together at Q/4. Three survivors raise `InsufficientEligibleWeight`. [V]
- A member needs its own anchor of round `w-1` to be rostered at `w` (`:5155`, `:6069-6071`).
  Anchors come from genesis (`:5205`), from a `MATCH` audit (`:7318-7328`) and from rollback
  (`:9157`). A member left out before its audit never re-enters this run (measured: 409
  `missing accepted anchor`); one left out after a `MATCH` audit does (measured: round 1 opens). [V]
- The audit step has a window. `schedule_audits_v2` answers 503 `audit absolute horizon exhausted`
  when `absolute <= now + 1`, with `absolute = min(d_final, d_audit + 2 * lease_rounds)`
  (`:6396-6398`). A first call at `d_final - 1` gets that 503 for a full honest roster too
  (measured), because no job can be created. The operated shape keeps
  `d_final - d_audit <= lease_rounds` (`live_operator.py:123-134`). [V]
- Without leaves, `lease_v2` raises 409 at the first queued job (`:6593`) and never skips it, so
  the honest jobs behind it wait until `absolute`, when every queued job is exhausted. [V]
- ADR 0002: a verified leave, a lapse and a stall are never penalised, because a lapse cannot be
  told from an honest failure; only a failed audit penalises (decisions 5 to 7, I5). v2 has no
  leave message, so an honest eviction and a deliberate no-show look the same. [A]

### Decision

1. **When.** A roster member with no accepted `commit` for round `w` is a no-show. Grace: none,
   because intake closed at `d_commit`. The delay is the commit window the operator already
   chose in `RoundOpenV2`; no constant is added.
2. **Trigger: lazy.** The first `audits` call (`POST /v2/runs/{id}/admin/rounds/{w}/audits`) with
   `d_audit <= now <= absolute - 2` leaves the absent members out, in the transaction that creates
   the jobs of the others. A call outside the window answers 503 for every roster and writes
   nothing. An eager trigger in `push_beacon` was weighed and not taken: it cannot revive a round
   whose jobs can no longer be created, and it adds a writer to the beacon route.
3. **Record.** `excluded` / `{w}:{hotkey}` = `{reason: "NO_SHOW", stage, evidence_hash}` with
   `stage` in `commit`, `delta` and `evidence_hash = sha256("ht-no-show/1|run|w|hotkey|stage")`.
   The tape `Exclusion.reason` literal (`tape_v2.py:74`) is not touched.
4. **The rest of the round.** `unresolved_audits` drops once per absent member and the disposition
   stays `PENDING`: the others are audited, aggregated and finalized by existing code. The absent
   share goes to them by the existing token rule (measured: 5 x 200000 becomes 4 x 250000).
5. **Per-round budget.** A probe (`allocate_weights` over the remaining members) refuses with 409
   `ROUND_INFEASIBLE`, persisting nothing, when scheduling and when aggregating. `NO_SHOW`,
   `UNVERIFIED` and `INFRASTRUCTURE` exclusions of one round together must leave members whose
   caps carry Q: four equal members, fewer once probation or owner-group caps bind.
6. **Sanction S0.** None: no admission, strike, bond or ledger effect. The member loses that
   round's reward and, if it was left out before its audit, its place in the run. It is not
   suspended or struck. This is ADR 0002's rule for an eviction, applied to a member that cannot
   be told from one.
7. **Delta stage.** `aggregate_v2` leaves out a member that passed its audit and has no delta once
   `now >= d_upload`. No decrement: its audit already did it. The member keeps its anchor.
8. **Leaves stage.** `lease_v2` skips a queued job whose member has no `leaves` record. The job
   stays queued until `absolute`; its exhaustion is AUD-01's. Naming that exhaustion `NO_SHOW`
   is a separate, droppable step that awaits Forge.
9. **Not solved here.** Survivors that cannot carry the round (the run stays blocked, explicitly);
   rejoin of a member left out (Q3); a rollback whose history holds such a round; tapes that name
   the exclusion; a round whose audit window was missed (no exit for any roster).

### Consequences

- A 5-member round with one absent member settles with four, pays them, and opens `w+2`. [V:
  real round 0 settled, round 1 settled, round 2 opened; the silent member's admission row is
  unchanged.] Six members with two absent: store level only. [H for the real round]
- The same machinery serves any member left out before aggregation: this real round also
  exercises the `verified_inputs_v2` skip that Part 2's drops rely on. [V]
- A 4-member roster cannot lose anyone: `ROUND_INFEASIBLE`, no worse than before. The TEST
  runbook opens rounds on four miners; it must ask for five when an eviction is plausible.
- A member that commits but never uploads its delta no longer freezes the round at aggregate
  (before: 409 `missing accepted delta` after `d_upload`, with all audits `MATCH`). [V]
- The composite case (a `NO_SHOW` and a later `UNVERIFIED` leaving fewer than four) now answers
  409 `ROUND_INFEASIBLE` at aggregate instead of a bare `InsufficientEligibleWeight`; the round
  stays `PENDING` and the run blocked. No void-round lineage exists. [V for the delta variant, H
  for the composite]
- Any later rollback whose bounded history holds a round with a pre-apply exclusion fails closed
  (`aggregator/rollback_v2.py:455-458`: tape inputs must cover the opening roster). This already
  holds after `UNVERIFIED` and `INFRASTRUCTURE`; one idle identity can now cause it. [V by reading]
- Beacon lag then burst across the commit window excludes members that could not get in (503
  while lagging, then 409 closed). They are not sanctioned and lose the round. D40's own case, a
  trial across a pause, is outside this part.
- P1 (elastic shards) stays the real fix for eviction; this is the v2 floor and changes no wire.

### Rejected alternatives

1. **Whole-round `EXCLUDED` for a no-show:** voids the honest members' reward (`settle` refuses it,
   `finality_v2.py:74`) and does not resume.
2. **A signed amended opening without the absent member:** changes `roster_hash`, the slot
   assignment and the wire; every member would retrain.
3. **A synthetic `MISMATCH` for the absent member:** invents a verdict with no commit to compare.
4. **Commit grace beyond `d_commit`:** intake is closed there; a protocol decision, not a constant.
5. **`admission.suspend`, a strike, a per-coldkey budget:** `resume` has no caller in `src` except
   after a dispute, `strikes` means a failed proof, and a budget needs new policy fields that
   change the run ID. The `NO_SHOW` records are the evidence a later budget would count.
6. **Eager exclusion from `push_beacon`:** see Decision 2.
7. **Writing in `verified_inputs_v2`:** a read function used by aggregate and finalize.
8. **A void or null round when the survivors are too few:** needs lineage design (`applied[w]` opens
   `w+1`, survivors need anchors, rollback assumes one tape per round).

### Not verified

- Six members with two absent end to end; the composite case; a beacon lag then burst; CUDA; the
  service path (roster over 16 or a service-admission record), which still ends in the heavy 503.
- A leaves-less job whose member posts leaves after `absolute`; the NO_SHOW naming of leaves
  exhaustion (needs AUD-01's code, absent from this tree).
- The `implementation_hash` of the final tree and a rollback after a `NO_SHOW` round (reading only).

## Part 6: host-controlled bytes cannot crash the CUDA executor (EXE-01)

- Lens red team B finding EXE-01 (IMPORTANT) with Principal D36 bis ("catch RecursionError and any
  parse error on host-controlled JSON and quarantine the host exactly as ADR 0008 section 4
  promises"). Amends ADR 0008 section 4 (rows "host refused" and "attempt failed") and nothing else
  of it. Edits `gpu_ops/cuda_executor.py` only: no manifest, schema, wire format, protocol constant
  or run id changes, and `implementation_hash` does not change (it hashes ten other files and the
  owned names of `store.py`, measured). No code of this part ran on a GPU host.

### Context

- ADR 0008 section 4 promises that "host-reported fields that do not parse or exceed their bounds"
  refuse the host (quarantined until the challenge restarts, next host tried), and that a result
  failing validation is a failed attempt with nothing published (`docs/adr/0008:121,123`). [V]
- `Executor.launch` handles `HostRefused`, `(HostLost, HostBusy)` and `AttemptFailed` only
  (`gpu_ops/cuda_executor.py:960,965,969`). Anything else leaves through the `finally` (`:974`): no
  journal row, no quarantine, no failover, and `_order` (`:477-480`, a function of the job digest)
  puts the same host first again for the same job. [V]
- Four escapes reproduce at `65be1962`. (1) The RepoDigests line goes through `json.loads` (`:240`,
  handler `:242`): 100,000 nested `[` raise `RecursionError` (Lens X2). (2) `COMPLETE` goes through
  `json.loads` under `except ValueError` (`:711-714`): same (Lens X1). (3) `tarfile` reads the size
  an extended header claims in one `fileobj.read` (PAX `x`, `g`, GNU `L`, `K`): 2**40 to 2**62 raise
  `MemoryError`, 2**63-1 and more `OverflowError`; the handler at `:706` lists neither. (4) The
  probe clock is an unbounded int and `abs(snapshot.epoch - (before + after) / 2)` (`:566`) raises
  `OverflowError` for a 400-digit `date +%s`. (3) and (4) are not in the Lens report. [V]
- Three gaps do not escape but defeat section 4. `_check_rank` accepts any JSON list as
  `trace.json` (`:1023`): `[1, 2, 3]`, `[{}]`, a 9,000-level list and a rank-swapped trace were
  published by the hostile host, `IslandParty.published` (`store.py:7268`, `auditor/worker.py:194`)
  then raised a pydantic `ValidationError`, and the resume path (`published()`, `:899-926`) returned
  the same publication with no host call: the job is stuck. A probe or pull that prints more than
  1 MiB is an attempt failure worded "result exceeds max_result_bytes" (`:401-405`) and burns a
  slot of `max_attempts`. A RepoDigests entry with a lone surrogate passes the probe and fails in
  `canonicalize` (`protocol/jcs.py:23`) after a whole container run. [V]
- The recursion limit protects nothing: the `json` C scanner of CPython 3.12.15 stops at 9,997
  levels whatever `sys.setrecursionlimit` says, and importing `hypertrain.challenge.store` raises
  the limit to 100,000 (`py_ecc`). Already defended: pydantic's own parser, the `except Exception`
  of `_publish` (`:776`), tar header chains (`RecursionError` in the tuple at `:706`). 20,000
  random probe and 6,000 random tar mutations find only the clock overflow; targeted bombs find
  the rest. [V]
- Existing tests pin malformed result metadata as a failed attempt, "a bad result is not a bad
  host" (`tests/gpu_ops/test_cuda_executor_dispatch.py:810,921`). `miner/island_launch.py` is one
  of the ten `implementation_hash` files, so validation fixes stay in the executor. [V]
- `execute_audit_v2` ends in `try/finally` and no handler (`store.py:6897-7336`); Part 2 books
  whatever leaves the executor boundary. This part decides what that is. [V]

### Decision

1. Classification, resolving the overlap of rows 3 and 5. What the host says about itself outside
   the container result (the probe, the pull) is row 3: anything that does not parse or exceeds a
   bound is `HostRefused`. Inside the result, what an honest but broken environment can produce
   stays row 5, `AttemptFailed` (malformed JSON, schema violation, hash mismatch, unsafe member,
   size over a cap, unreadable trace). Two inputs only an attack on our parsers explains are
   promoted to `HostRefused`: a metadata document nested deeper than 8 levels (the runner writes 2
   in `COMPLETE` and 3 in `environment.json`) and a tar read above `SMALL` (1 MiB).
2. The RepoDigests line is read by a closed grammar, not by JSON: at most 32 double-quoted
   references of 1 to 256 printable ASCII characters without space, quote or backslash. No parser
   recurses, no escape, no surrogate, no big integer.
3. The probe clock must be a `time_t` (`-2**63 <= epoch < 2**63`); the comparison with the
   challenge clock then cannot overflow.
4. `shallow(raw)` decodes `COMPLETE` and `environment.json` strictly as UTF-8 and counts nesting in
   one linear loop that skips strings; a document deeper than `HOST_JSON_DEPTH = 8` is refused
   before any parser sees it. Sizes are bounded first (members over `SMALL` are refused at `:688`).
5. The result tar is read through `_CappedReader`: a single `read` above `SMALL` is `HostRefused`.
   Members are streamed in 1 MiB chunks, so the bound is inclusive.
6. `Executor._attempt` turns `RecursionError`, `MemoryError` and `OverflowError` raised by
   `_read_result` or `_check_environment` into `AttemptFailed`. No quarantine: for a site nobody
   foresaw the cause may be ours, and quarantining on it could take every host out.
7. A probe or pull whose output passes the transport bound is `HostRefused` ("host probe output
   exceeds its bound", "image pull output exceeds its bound"); the container run keeps
   `AttemptFailed`.
8. A trace is published only if `IslandParty.read_trace`, the reader of the store, the audit
   worker and a dispute, accepts it and each entry carries the rank of its directory; otherwise
   `IslandFailure`, which `_publish` turns into `AttemptFailed`. The resume path applies the same
   gate. A rank's `summary.json`, `leaves.json` and `trace.json` may not exceed 64 MiB (the
   reader's own `TRACE_BYTES`): refused as an unacceptable member before staging.
9. `Executor.launch` is not edited. What it raises when no host can serve the job is plain
   `IslandFailure` (never a subclass): "cuda executor found no host able to run the job: ..." when
   every host is quarantined or unavailable (at once, with no pause between sweeps), "cuda
   executor gave up: ..." after `max_attempts` failed attempts. `Aborted`, a subclass, only for a
   deadline or a cancelled lease. Part 2 books what leaves the executor boundary as the
   executor's failure, unless the lease guard is already cancelled.

### Consequences

- A hostile host costs at most one container run (a bomb in the result) or none (a probe), is
  quarantined until the challenge restarts, and the next host serves the same job. After this part
  the only exceptions that bytes of a host can raise out of `launch` are `IslandFailure` and
  `Aborted`: 26,000 random mutations and 68 targeted inputs raise nothing else.
- Stricter than JSON means fail closed: a daemon that prints the image list with an escape or a
  space after the bracket is refused, with a journaled reason, until the host or the grammar is
  fixed [H on real docker output]. `cuda_executor_check.py` shows it before any job.
- The executor and the audit reader can no longer disagree about a trace. Each trace is parsed
  twice (executor gate, then store), and again when a finished job is returned.
- The validation fixes are in the executor, so `miner/island_launch.py` and `implementation_hash`
  are untouched. `sources()` and the bundle change in one file; the runner is unchanged, so
  `runner_sha256` and the GPU image stay valid.
- Not changed: a host that accepts the job and then hangs, or trickles bytes, holds it until the
  job deadline (`Aborted`); `max_result_bytes` still bounds the binary files that
  `validate_artifacts` reads whole.

### Rejected alternatives

1. Add `RecursionError` and `MemoryError` to the two `except` tuples: passes the reproducers, keeps
   a 10,000-frame C recursion on host bytes and leaves the late surrogate failure, the tar claims,
   the clock overflow and the deep trace.
2. A blanket `except Exception` in `launch` that quarantines: it cannot tell the probe from the
   result from our own journal and lock code, hides programmer bugs, and a `MemoryError` of our
   own would quarantine every host in turn.
3. `envelope_v2.load_json` for `COMPLETE`: its object bound (4,096 keys) is below `MAX_MEMBERS`
   (16,384) and it cannot read a list. Its depth scan is reused, copied as `shallow()`.
4. A regex string-skipping depth scan: quadratic (1.8 s at 16,000 escaped quotes, x4 per doubling).
5. Every result-side failure stays `AttemptFailed`: smallest diff and literally row 5, but a hostile
   first host then costs a container run per job and `max_attempts` slots (three hostile hosts and
   one healthy: the job gives up before reaching it).
6. Quarantine every parse or schema failure of the result: contradicts row 5 and about ten pinned
   tests, and one deployment bug (a `torch_path` over 256 characters) would take every host out.

### Not verified

- Real `docker image inspect --format {{json .RepoDigests}}` output (the grammar assumes Go's
  compact encoding of ASCII references), a GPU run, a live SSH host: the fake host replaces the
  daemon and the training step. [H]
- Other CPython 3.12 patch releases: the 9,997-level `json` limit and the tar chain threshold
  (33,000 chained empty PAX headers parse at the production limit of 100,000 and cost 0.5 s) were
  measured on 3.12.15 only. [H]
- Production sizes of `leaves.json`, `summary.json` and `trace.json`; 64 MiB is the largest bound a
  reader accepts, not a measured need. [?]
- The store side of the chain (500 from `/execute`, the booked attempt) is Part 2; here only the
  class and message that reach `execute_audit_v2` are pinned, over the fake host. [A]

### Sources

`hypertrain/src/hypertrain/gpu_ops/cuda_executor.py:212-252,401-405,477-480,553-584,651-723,754-791,
793-884,899-984,1018-1024`; `auditor/island_bisect.py:111,159-171`; `auditor/worker.py:185-198`;
`challenge/store.py:4249-4262,6897-7336`; `protocol/jcs.py:23`; CPython 3.12.15 `tarfile.py:1461`;
`docs/adr/0008-generic-cuda-executor.md` section 4; Lens red team B `rtB_executor_host_attacks.py`
X1-X3 (5 passed in 2.55 s: they assert the bug), 2026-10-27 [A].

## Part 7: one lock order for the dispute long poll (DSP-03, F5)

- Lens C2 finding DSP-03 (BLOQUANT) and Lens C1 v3d F5, with Principal D49. Nothing here changes a
  manifest, a wire format, a protocol constant, a stored record or `implementation_hash`
  (`disputes_v2.py` is not one of its inputs). Edits `challenge/disputes_v2.py` only.

### Context

- Two locks meet in `DisputesV2`: the escrow mutex, which is `ChallengeStore._lock` once hosted
  (`store.py:2586`), and `self.condition`, the long-poll condition. [V]
- `events()` took the condition and, inside it, the mutex: `wait_for(lambda: bool(available()))`
  evaluates `available()` with the condition held, and `available()` reads the event table under
  the mutex. Every mutator takes the mutex first (`escrow.tx()`, or the store's `_tx` around it)
  and calls `notify()` before it commits (`open`, `post`, `state_request`, `reassign`, `timeout`,
  `exclude`, `resolve`), which takes the condition. Condition then mutex on one side, mutex then
  condition on the other: a classic ABBA. [V]
- The hosted callers already call `notify()` again after their own commit (`push_beacon`, the
  accept route, the referee and rollback routes), but that cannot help: the first notification of
  the same mutator is the one that blocks, with the store lock held, and every other route of the
  store waits behind that lock. [V]
- Reproduced on the tree before this change: a mutator inside a transaction waits for the condition that a
  long poll holds while it waits for the mutator's lock; both threads were still alive after
  10 s (`test_a_mutator_that_notifies_under_the_store_lock_does_not_deadlock_a_parked_poller`). [V]

### Decision

1. **One order, outermost first: the store lock (`escrow.mutex`), then `DisputesV2.condition`.**
   The condition is a leaf: it guards one integer (`_beats`) and nothing is taken while it is held.
   The order is written in the class docstring.
2. **`events()` does not read inside the condition.** It reads `_beats`, reads the event table under
   the mutex alone, and parks on the condition until `_beats` moves; the counter is read before the
   table so that a notification between the two is not lost. `notify()` bumps `_beats` under the
   condition. A long poll is therefore never between the two locks, and a mutator may notify while
   it holds the store lock: the notification takes the leaf and never waits for anybody.
3. **Notifying under the store lock is allowed, not required.** Hosted callers keep notifying after
   their commit (a woken poller then finds the committed row at once, and the other processes
   are told through `_notify_processes_v2`); a poller woken early finds the mutex taken, waits
   for the commit and reads the committed table. A rolled-back transaction costs one spurious
   wake-up.
4. **A debug assertion detects an inversion.** Under `__debug__` the mutex of the escrow is wrapped
   (`_OrderChecked`): a thread that holds the condition (`_leaf()` is the only way in) and asks for
   the mutex fails with `AssertionError("lock order inverted ...")` instead of deadlocking against
   the next mutator. It sees every acquisition made through `escrow.mutex` (`escrow.tx()` and
   `events()`); it cannot see the store taking `_lock` directly, which is not a place that holds the
   condition.

### Consequences

- The long poll can no longer block the store: a store route that waits for `_lock` waits for
  another route's transaction, never for a poller.
- A poller that wakes finds `_beats` moved and reads the table once more; a poller that times out
  returns what the table holds, as before. `timeout=0` reads once.
- The assertion is a development aid: `python -O` removes it. The structure (leaf condition) is
  what prevents the deadlock; the assertion is what tells the next editor who breaks it.

### Rejected alternatives

1. **Deferring every `notify()` to after the outermost commit** (D49 as first worded): needs a
   commit hook in the store's `_tx` and in standalone use of `EscrowV2.tx`; the hosted callers
   already re-notify after commit, and the leaf order makes an early notification harmless.
2. **Holding the store lock for the whole wait in `events()`:** blocks every route for up to
   30 s per poller.
3. **Releasing the condition around `available()` inside `wait_for`:** the predicate runs with the
   lock held by construction; releasing it from the predicate is the lost-wakeup bug again.
4. **Polling the table on a timer:** wall-clock decides when an event is seen; the counter keeps
   the wake-up exact and costs one integer.
5. **A global lock hierarchy checker:** two locks in one class do not justify a framework.

### Not verified

- A process-level run of several long pollers against a busy store; only two threads and the
  fixture's locks were exercised. [?]
- F4 (a lock id named like a dispute id) is the escrow-relay lot's and was not touched. Both lots
  edit `disputes_v2.py`; this change keeps to the constructor, `notify`, `events` and one class.

## Part 8: the general exclusion exit (D43, D44, D46, D47, D51, AC-CLIFF, DSP-04, R-C2, R-D)

- Principal D43 (every wait on a peer has a bounded exit: one table, one helper, one test per state), D44
  (two free identities must never freeze a run), D46 (one rule: exclude and sanction the faulty peer,
  recompose or close the round, record the withdrawal), D47 (sanction rules), D51 (one extended ADR),
  Forge's AC-CLIFF, Lens F2 (roots R-C2 and R-D), Lens DSP-04 and Lens m2. This part is the rule; Parts 1,
  2 and 5 are three of its rows and keep their text, amended where this part says so.
- Amends Part 5: decision 2 (lazy trigger) and its rejected alternative 6 (an eager trigger), decision 6
  (sanction S0: none) and decision 9 (survivors that cannot carry: the run stays blocked). Supersedes
  Part 5 rejected alternative 8 (a void round needs a lineage design: it is designed below).
- Nothing here changes a manifest, a policy, a wire message, a protocol constant or a run id. One new
  disposition (`RETIRED`), one new escrow event kind (`ROUND_WITHDRAWN`) and a few record kinds, all
  inside the stores. `implementation_hash` changes through the owned names listed under "Hash
  consequences". Rules that are the founder's to change are marked **[sign-off]** and collected under
  "Decisions for Forge".
- Written before the code, from seven read-only design packages with private prototypes
  (exit core, retired round, ledger withdrawal, roster member state, sanctions). Facts are labelled
  [V] (verified by a command on a private copy of `f3d51478`), [V by reading], [A], [H], [?]. The
  code that follows this text may refine a mechanism and will correct this text in the same commit.

### The rule

Every wait on a peer has a bound that is a value of the round schedule the operator already chose
(`d_commit`, `d_upload`, `absolute`, `d_final`, a dispute deadline); no constant is added. At the bound,
in this order and without the peer:

1. the peer that holds the round is **excluded by a record** (`excluded`, with its reason, stage and
   evidence), or, when nobody left can carry the round, the round is **retired**;
2. its **blame** is booked once: the miner's, or infrastructure's. Blame is a witnessed fact, never a
   guess;
3. a miner's blame becomes a **sanction** (strike, suspension, bond burned) through one hook, applied by
   a separate retried step so that a failing sanction never keeps the peer in the round;
4. the round is **recomposed** (the others are audited, aggregated and paid) or **closed without
   reward** (retired), and a round closed without reward is **withdrawn from the reward order** of the
   ledger by a recorded event, so `FINALIZE_ORDER` goes on;
5. a wait that no peer holds (too few anchored members for the next round) answers a **defined 409**,
   never a 500, and idles with nothing locked.

### Context

- Time is the beacon table and `push_beacon` is its only writer (Part 1). The lazy exclusion of Part 5
  needs somebody to call `audits` inside `[d_audit, absolute - 2]`, although commit intake closed at
  `d_commit`; a missed audit window has no exit at all (503 for ever from `absolute - 1`). [V]
- The caps are in the protocol: `miner_cap_units` is `Literal[4194304]` (Q/4) and `TapeBodyV2.inputs` has
  `min_length=4`. `allocate_weights` over a roster passes with 4 ACTIVE members, raises with 3, passes with
  3 ACTIVE + 1 PROBATION. A roster of R tolerates at most R - 4 exclusions. [V: `quorum.py` of the exit
  prototype] D44's sentence "2 silent out of 5: next round open, 3 honest paid" cannot hold (three members
  carry 3Q/4); "2 silent out of 6: 4 honest paid, round 1 opens" does, and is the test.
- A member needs the anchor of round w - 1 to be rostered at w. A silent member has none, and a
  never-rostered ACTIVE identity has none either (409 `missing accepted anchor`): a mid-run joiner is
  Q3, not solved. So "a new draw among the ACTIVE miners" can only draw from the survivors of the
  previous round. [V]
- A round the survivors cannot carry wedges the run for good: `open_round_v2(w + 1)` reads `applied[w]`,
  which only a tape of at least four inputs writes; an opening is immutable (`conflicting round
  opening`), so the roster cannot be amended. It also pins the bond of every honest roster member
  (`lock_settlement` keeps a lock while a round that lists the admission is not closed). [V]
- The ledger refuses to skip an ordinal: `reward_finalize` raises `FINALIZE_ORDER` unless the round is
  `MAX(escrow_finalized.w) + 1`, and nothing writes the row of a round that finality closed without
  reward (an accepted rollback today, the exits of this part tomorrow). Real HTTP: after an accepted
  two-round rollback and a fully audited round 2, `finalize` answers 409 `FINALIZE_ORDER`. [V]
- A roster member's admission can stop matching its roster entry after the round opened: a rotation (the
  old hotkey is refused at intake, `aggregate` answers 409 `UNKNOWN_HOTKEY` for ever), a contest of its
  own MATCH verdict (DSP-04: it suspends the contestant, DSP-01 closes the dispute, the contestant is
  `PROBATION` with `clean_count 0`, not live, and `aggregate` still answers 403), a screen that lapses.
  `verified_inputs_v2` raises for the whole round instead of leaving the member out. [V]
- Our own sanction can wedge the run. Round 0 applied and not finalized, member M MATCH in it and silent in
  round 1: the sanction makes M ineligible and `finalize(0)` answers 403 for ever
  (`verified_inputs_v2` re-checks eligibility). 403 without a gate, 200 with it. [V, six identities]
- `Admission.seize_bond` (ADR 0010 Part 6) exists and has no caller. `NO_SHOW` burns the bond and leaves
  the admission as it is; `AUDIT_MISMATCH` suspends, adds a strike and burns. `strikes` is read by
  nothing; `SUSPENDED` without a pending dispute has no route back. [V by reading]
- Unmapped refusals. `app.py` maps `AdmissionError`, `DisputeError`, `EscrowError`, `EnvelopeError` and
  `ValidationError`; `InsufficientEligibleWeight` (a `ValueError`), `FinalityError` and `TapeError` (a
  `RuntimeError`) are not: `POST /admin/rounds` with a 3-member roster and `w = 2` without round 1 answer
  500. The aggregate path is already 409 `ROUND_INFEASIBLE` (Part 5). [V]
- The numerics pin of Part 3 is decided and not yet in the sanctions' tree: enforcing `AUDIT_MISMATCH`
  before it is in code would burn honest miners of another CPU class. [V]

### The table

`L` = `AuditPolicyV2.lease_rounds` (100 for H <= 30), `A = 2L`, `absolute = min(d_final, d_audit + A)`,
`T` = `dispute_per_level`. A "push" is one `push_beacon`; every bound reads "the first push at or after".

| # | Waiting state | Who can block it | Bound | Exit action | Blame | Sanction | Test |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | dispute (Part 1, done) | a funded contestant that cannot win; a silent auditor or referee | dead end: next push; abandoned: `turn_deadline + T + 1`; paused final op: `absolute_deadline + 1` | pause, `exclude` (loser None), `settle_lock`, `resolve_pending` | infrastructure | none; a referee `FRAUD` goes through `settle_fraud` | `test_dispute_exit_v2` |
| 2 | roster commit (Part 5) | a roster member that never commits | `d_commit` (was: the first `audits` call) | `excluded NO_SHOW/commit`, `unresolved_audits - 1`; if the rest cannot carry: row 7 | miner, unless the assignment window was paused or nobody committed | D44 (decision 12) | `test_exit_commit` |
| 3 | leaves never posted | a member that committed and never posts `/leaves` | `absolute` (<= `d_final`) | the queued job is dropped as `NO_SHOW/leaves`, no re-arm | miner (no auditor ever touched the job) | as 2 | `test_exit_leaves` |
| 4 | delta never uploaded | an audited member, or the relay | `d_upload` | `excluded NO_SHOW/delta` | miner, unless nobody delivered or the upload window was paused | as 2 | `test_exit_delta` |
| 5 | audit job, lease (Part 2, done) | an auditor, an executor host, a miner whose sealed input cannot be leased | attempt `L`; two windows of `A`; the second ends where the sealed commit ends | cooldown, requeue, re-arm once, drop only the miner (`INFRASTRUCTURE`); `MINER_INPUT_REFUSED` dropped at once | infrastructure; miner only for an input it sealed short | none | `test_audit_liveness_v2` |
| 6 | audit window missed | the operator who never calls `audits` | `absolute - 1` | the round is retired (`AUDIT_WINDOW_MISSED`) | infrastructure | none | `test_exit_audit_window` |
| 7 | round infeasible (AC-CLIFF) | nobody in particular: the Q/4 caps after any exclusion above | the push that first sees the survivors cannot carry | blame booked per member; the round is retired (`INFEASIBLE`); the next opening answers 409 until four anchored members are eligible | per member (rows 2 to 4) | per member | `test_exit_infeasible` |
| 8 | roster member changes state (R-C2, DSP-04) | the member: rotation, contest of its own MATCH, a lapsing screen; our own sanction | refused at the door (rotation, MATCH contest, screen at open); judged at `aggregate`; if too few remain, retired at `d_final` | `ROTATION_ROUND_OPEN` / `NOTHING_TO_CONTEST` 409; `excluded INELIGIBLE` with its cause; `finalize` pays the applied tape | the member's own state; never a second sanction | none | `test_member_state` |
| 9 | `FINALIZE_ORDER` without a withdrawal record (R-D) | nobody: a round closed without reward has no ledger row | the next reward finalize | `ROUND_WITHDRAWN` event for every closed-unpaid round below it | none | none | `test_round_withdrawal` |
| 10 | a round with no finality record (Lens m2) | nobody: both writers write the `round` and `finality` records in one transaction and nothing deletes either | n/a | fail closed: `rounds_listing` is `None`, so the bond stays held and the rotation refused; the real pin, a round that never closes, is removed by retirement | none | none | `test_member_state` (pinned) |

Not rows: a run that lacks four anchored ACTIVE members for the next round (capacity, answered by a 409 at
open), and an operator that stops calling `aggregate`, `finalize` or `open`. Both are named so nobody reads
the table as covering them.

### Decision

**A. The mechanism**

1. **The bounds are data, the table is this ADR's.** `challenge/exit_v2.py` holds `Wait` (an enum of the
   states, with the two refusals at the door of decision 21), `BOUNDS` (one function of the round
   schedule per state, no new constant), the pure planner and the blame rule. The table above is the one
   table; the code does not repeat its prose, which would drift. A test asserts that every `Wait` has a
   bound, that each is a value of the round schedule no later than one round past the end of the second
   audit window, and that every exit the planner books has a seizure reason (decision 12). The bound of
   row 1 in code is its outer one (a turn cannot outlive its job, so the end of the second audit window
   plus one); the nearer ones are Part 1's. The lag after which the master counts a pause (decision 5)
   is the single constant `MAX_LAG_ROUNDS`, shared with the freshness gates of the store.
2. **One reaper on `push_beacon`**, after the dispute reaper and before the trial reaper: its own
   transaction, one savepoint per unclosed round, a run-level guard so that nothing reaches the beacon
   route after the beacon committed, failures journaled as `exit-failed` and rewritten only when they
   change (so `_rollback_snapshot_v2` is stable under a persistent failure). It reads a snapshot of each
   round, asks the pure planner and applies the answer through `_exclude_v2`, the single function that
   records an exclusion and books its blame. It skips a run with a `service-admission` record (its roster
   must stay complete).
3. **Eager exits.** The first push with `now >= d_commit` excludes every member with no commit, the first
   with `now >= d_upload` every audited member with no delta. The records are byte-identical to the lazy
   writers' (`_exclude_no_shows_v2`, `_exclude_missing_deltas_v2`), which stay as a backstop. A queued job
   whose miner never posted leaves is dropped at `absolute` as `NO_SHOW/leaves`, with the miner's blame
   and no re-arm: only the master lacks the leaves, no auditor ever touched the job, and a second window
   cannot help. This amends Part 2 decisions 8 and 9 for this one case.
4. **Liveness before accountability.** The exit is committed first and never depends on the sanction. The
   sanction is a row of its own (`records_v2 exit-sanction`, id `{w}:{hotkey}`: PENDING, APPLIED, WAIVED
   or CANCELLED) applied by a sweeper that retries on every push. ADR 0010 and Part 5 asked for one
   transaction; a sanction that raises would keep the member in the round for ever, which is the freeze
   this part removes.
5. **Blame is a witnessed fact.** `NO_SHOW` is the miner's only if the window it missed was served and
   somebody used it: the reaper records `beacon-pause` when the wall clock was more than two rounds ahead
   of the last round it had (the predicate `_fresh_v2` refuses on); a commit-stage absence in a round
   where nobody committed is infrastructure; a delta-stage absence is infrastructure when nobody
   delivered a delta or the upload window was paused. No waiver is one a miner can pick (an `accept` or
   an upload grant costs a free identity nothing). The wall clock never decides when an exit happens, only
   whether the miner is blamed.
6. **Not automated.** The master does not open the next round, draw its roster or lower the quorum: a
   `RoundOpenV2` is the operator's signed schedule and the caps are pinned by the schema.

**B. A round that cannot reach a tape is retired**

7. **A retired round is a record, not a tape.** `applied[w] = {kind: "null", out_state = prev_state, ...}`,
   `finality[w]` has the new closed disposition `RETIRED`, `retired[w]` holds the evidence
   `{v: "ht-round-retired-v1", run_id, w, cause, roster_hash, survivors, theta_hash}` whose sha256 is the
   evidence hash. `open_round_v2` is unchanged: it reads `applied[w]` and the anchors of round `w`.
8. **Causes, each re-derived by the primitive** (`_retire_round_v2`, called by the reaper's per-round
   savepoint and by nothing else; idempotent, it cannot void a healthy round): `INFEASIBLE` (`now >= d_commit`
   and `allocate_weights` over the survivors raises; every term only shrinks) and `AUDIT_WINDOW_MISSED` (no
   lease row and `now >= absolute - 1`). Both require every earlier round closed (so no rollback window
   contains a retired round; an open predecessor delays the retirement and the state is recorded in
   `round-exit`), an unapplied `PENDING` status and no `service-admission` record. The one definition of
   "survivors" is the planner's: the roster minus the excluded, minus the members with no commit from
   `d_commit`, minus the committed members with no delta from `d_upload`. A round whose audited members
   became ineligible (decision 22) is retired at `d_final` if too few remain.
9. **Anchors carried, nothing recomputed.** Every roster member's anchor `w` is its anchor `w - 1` under new
   hashes (`ht-carried-proof/1`, `ht-carried-anchor/1`): the tensor files are linked and only the metadata
   is rewritten, `state_root` and `ef_hash` are unchanged. Survivors restart `w + 1` from the optimizer
   state and error feedback they held at the start of `w`. Training: theta, outer momentum and centre are
   unchanged (an identity, not a zero-update step, which would move theta by momentum); the data window of
   `w` is consumed. A miner needs no action: its start state is pinned by hash (`IslandJobV1`) and a
   commit with another `ef_in_hash` is refused.
10. **Audits, disputes, locks, grants.** Open leases become `exhausted/ROUND_RETIRED` and their guards are
    revoked; new work on the round answers 409 `ROUND_RETIRED` (`schedule_audits_v2`, `aggregate_v2`,
    `finalize_v2`, `rollback_v2`; owned names); completed verdicts stay as evidence. A dispute is not
    waited for: an open one leaves the round retired but not closed until Part 1 closes it.
    `relay_settlement_v2` answers for the grants of a retired round. The honest bonds are released by
    `lock_settlement` as soon as the round is closed.
11. **The next round.** The operator opens `w + 1` with `prev_final_hash = retired[w].evidence_hash`
    (`live_operator` reads it). It needs four eligible members that hold an anchor for `w`; fewer answers
    409 `ROUND_INFEASIBLE` and the run idles with nothing locked. A new member cannot join: Q3.

**C. The sanction (D44, D47)**

12. **`NO_SHOW` sanctions as `AUDIT_MISMATCH` does [sign-off].** `seize_bond(NO_SHOW)` suspends, adds one
    strike and burns the bond (unvested rewards spared, the coldkey stays bound by the one-non-ACTIVE-row
    rule); a one-tuple change in `admission.py`. It resolves by the admission id of the roster entry, so a
    rotation cannot dodge it, and is idempotent per (reason, evidence).
13. **No gate: `finalize` pays the applied tape (decision 23).** An earlier design made the sweeper wait
    for the member's earlier unclosed rounds, because a sanction makes the member ineligible and `finalize`
    of an applied round re-checks eligibility. A gate cannot hold `settle_fraud` (it runs inside a referee's
    resolution), a lapse or a rotation; decision 23 cures every path and lands before the first caller of
    the hook. The gate (about 25 lines: a PENDING row until no earlier unclosed round lists the member) is
    the documented fallback if decision 23 is refused.
14. **`SanctionHook`.** The sweeper calls the hook through a Protocol of one method
    (`seize_bond(admission_id, *, reason, evidence_hash, now)`), bound to `Admission` in production and to a
    fake in tests. It never calls `TRAINING_FRAUD`: that stays `settle_fraud`'s. The evidence hash is the
    exclusion's own, re-derived by the reaper before use; a record whose evidence does not re-derive is
    never sanctioned.
15. **What reaches the hook.** Two shapes only: an `excluded NO_SHOW`, and an `excluded UNVERIFIED` backed
    by a completed `MISMATCH` lease, the stored executed verdict whose digest is the excluded evidence and a
    graded verdict. Nothing the auditor or the host did reaches it: executor exceptions, quarantines, lease
    lapses, cooldowns, exhaustion and dropped jobs (`MINER_INPUT_REFUSED`) leave no verdict. A host that
    returns well-formed false leaves is not detectable by the master; the window, the numerics pin and an
    operator-run executor list are the controls, and the limit is stated.
16. **Confirmed (D47a).** `LOCATED` (`first_bad_leaf` named) or `COMMIT_CONTRADICTS_LEAVES` (no leaf named
    and the replay's leaves root equals the committed root: the miner's signed commit contradicts its own
    published leaves, the Lens S2 shape and the least numerics-prone one). Anything else is an audit anomaly
    and is never sanctioned. A dispute closed against the miner is a resolution `FRAUD` or `PARTY_TIMEOUT`
    with the miner as loser; a closure `INFRASTRUCTURE` of any cause leaves the verdict standing and an
    abandoned turn never exonerates (otherwise every contest would be an escape).
17. **Window and one free contest (D47b) [sign-off].** `due = first_seen + verify.T.dispute_per_level`, a value
    the manifest already gives a party to make one move. The contest bond is refundable (returned on every
    closure but a `FRAUD` ruling against the miner) and is funded from available units, never from the
    admission bond; one dispute per verdict. A resolution `FRAUD` or `AUDITOR_FAULT` against the lease's
    auditor cancels the sanction; an open contest postpones it. Once the sanction is applied a new contest
    of that verdict answers 409 `SANCTION_APPLIED`. The verdict hash is published to the miner in the audit
    result it can read (D51: it needs it to contest). What a contest buys today is postponement: no
    auditor-side responder exists, so the Part 1 closure ends it `INFRASTRUCTURE` and the verdict stands.
18. **Order.** The `AUDIT_MISMATCH` rows are enforced only after Part 3 is in the code. Until then they run in
    `record` mode (the sweeper journals `WOULD_APPLY` and calls no hook; a constructor keyword, not a
    manifest field). `NO_SHOW` rows have no such dependency.
19. **No route back [sign-off].** A round-stage sanction binds no pending reference; the only exoneration is
    decision 17. The re-seed of ADR 0002 (P5) and any amnesty belong to the admission lot and need policy
    fields.

**D. A roster member that changes after the round opened (R-C2, DSP-04)**

20. **A roster is a contract.** From the opening to the closure of its round the admission behind an entry
    keeps its hotkey and either keeps its standing or is left out by a record.
21. `Admission.rotate` refuses with 409 `ROTATION_ROUND_OPEN` while a round that lists the admission is not
    closed (a round without a finality record counts as open). A contest of a verdict whose audit result
    is MATCH is refused with 409 `NOTHING_TO_CONTEST` by the store's `contest_of` callback, so
    `disputes_v2.py` is not touched; a MISMATCH can still be contested. `open_round_v2` refuses an entry whose
    `screen_until` is before `d_final`.
22. `aggregate_v2` first leaves out, with `excluded {reason: INELIGIBLE, stage: aggregate, cause}`, every
    audited member whose admission no longer qualifies (`ROTATED`, `REASSIGNED`, `BANNED`, `SUSPENDED`,
    `NOT_LIVE`, `SCREEN_LAPSED`, `UNFUNDED`). The survivors check that follows answers 409
    `ROUND_INFEASIBLE` and persists nothing when the rest cannot carry Q (then the reaper retires).
23. `finalize_v2` pays exactly the inputs recorded with the applied tape and no longer re-derives the live
    set. This is a prerequisite of every sanction: it must land before the first commit that calls the hook.
24. No sanction for `INELIGIBLE`: its cause is the member's own state or the effect of a sanction already
    applied. A sanction is not booked for an absent member whose admission is already unfit.

**E. The ledger (R-D)**

25. **A closed round is recorded as an event.** `ROUND_WITHDRAWN`, request `{"closure_hash", "w"}`,
    appended to the existing hash chain: no table, no snapshot element, no issuance. The order rule becomes
    "the first round that is neither reward-finalized nor withdrawn". `EscrowV2` takes an optional
    `round_closure(w)`; the store answers with the round's resolution hash iff its finality record is
    closed and was closed without reward (`EXCLUDED` after its rollback, or `RETIRED`).
26. **Written at the point of need**, inside the reward finalize that needs it, after its checks (a refused
    finalize records nothing). The id is `sha256("ht-round-withdraw-v2|run|w|<head of the chain>")`, a
    server-derived id in its own domain, never client-chosen (F1 compatible). A round finality closed
    without reward is never paid (`ROUND_CLOSED_UNPAID`).

**F. AC-CLIFF**

27. `InsufficientEligibleWeight` maps to 409 `ROUND_INFEASIBLE`, `FinalityError` and `TapeError` to 409
    `ROUND_BLOCKED`, on every route (three handlers in `app.py`); the store keeps raising the bare
    exceptions. The roster floor is documented as the arithmetic (R tolerates R - 4 exclusions: five
    tolerate one, D44's two need six) and enforced in `scripts/ops/live_operator.py open` (`--tolerate`,
    default 1; `--tolerate 0` opens four, explicitly), not in `open_round_v2`: the store keeps the protocol
    minimum of four, the protected-service open stays at exactly four, and a store floor of five would be a
    new constant that breaks every four-member fixture and the TEST runbook. The detail is
    `<CODE>: <message of the exception>`; the two rollback routes mapped a `TapeError` to a bare 409
    before the handlers existed and keep that message.

### The tests: one per waiting state, each proves the run continues

| State | Test (file) | Scenario | Mutant |
| --- | --- | --- | --- |
| dispute | `tests/challenge/test_dispute_exit_v2.py` (exists) + the e2e | forged delta + contest: the run restarts, `w + 2` opens | the closure removed |
| commit | `tests/challenge/test_exit_v2.py::commit` | 6 rostered, 2 silent: both excluded at `d_commit`, sanctioned, 4 audited and paid, round 1 opens | eager exit removed; sanction gate removed |
| leaves | same file | a member commits and never posts leaves: dropped at `absolute` as `NO_SHOW/leaves`, the others settle | stage naming |
| delta | same file | an audited member never uploads: excluded at `d_upload`, the round settles | the delta exit |
| audit job | `test_audit_liveness_v2.py` (exists) | one poisoned job costs its miner, not the round | exhaustion drop |
| audit window | same file | nobody calls `audits`: retired at `absolute - 1`, round 1 opens | the cause |
| infeasible (AC-CLIFF) | `tests/e2e/test_retired_round_v2.py` | 4 rostered, 1 silent: 409 `ROUND_INFEASIBLE` (not 500), retired, honest bonds released; round 1 opens with four anchored eligible members; 5 rostered, 1 silent: settles with four | retire on any exclusion |
| member state | `tests/challenge/test_member_state_v2.py` | rotation refused while rostered; MATCH contest refused; lapsed screen excluded `INELIGIBLE`; sanctioned member of an applied round is paid by `finalize` | each guard |
| finalize order | `tests/challenge/test_round_withdrawal_v2.py` | accepted rollback then a paid round: `FINALIZE_ORDER` before, 200 after, `escrow.verify()` passes | the callback |
| no finality record | `tests/challenge/test_member_state_v2.py` | a round without a finality record keeps the rotation refused and the bond held (pinned, with the rotation case red) | `rounds_listing` returns closed |
| sanction | `tests/challenge/test_sanction_v2.py`, the cheaters suite | confirmed MISMATCH sanctioned after the window; auditor/host failures never reach the hook | grade, gate, cancel |

### P1 to P8 of ADR 0009 section 4 (D47)

| Id | Verdict |
| --- | --- |
| P1 sanction of a round MISMATCH, coldkey, ban | implemented here (decisions 12 to 19); ban on FRAUD is `settle_fraud` (ADR 0010); the proposal of a synthetic pending reference is rejected (PRB-01 shape) |
| P2 AC-CLIFF | decisions 7 to 11 and 27 |
| P3 wedges | done in Part 2 (`lease_v2` skips a job it cannot lease); refusing `exp_drand < d_audit` at `/commit` is optional; refusing a non-finite leaf at `/leaves` is **deferred: harmful** (it turns a fast `UNVERIFIED` drop into a commit without leaves that exits after two audit windows) |
| P4 liveness | decisions 2 to 5 |
| P5 trial route back | (a) done by ADR 0010; (b) re-seed and (c) route back after a trial mismatch **deferred to the admission lot**: policy fields, a new policy digest |
| P6 lease cap, refuse at open | (a) done (Part 2); (b) refuse `d_final` beyond the audit horizon at open **deferred**: fixtures and scripts open rounds with gap 269 and must move first |
| P7 store lock before shared-connection reads | implemented: `require_roster_v2` (owned) and `weights` take the lock |
| P8 acceptance binding, commit 500, metrics_root | acceptance binding and commit 500 implemented in `training_v2`; `metrics_root` **deferred**: a comparator clause in the code the 22-cheat / 552-audit evidence pins; inert, bound through the leaves root; `RELAY-01` belongs to the escrow-relay lot |

### Hash consequences

- `implementation_hash` changes (it already changed in Parts 2, 3, 5): owned names edited are
  `schedule_audits_v2`, `aggregate_v2`, `finalize_v2`, `rollback_v2`, `open_round_v2`, `training_v2`,
  `require_roster_v2`; owned names added are `_exclude_unfit_members_v2`, `_unfit_cause_v2`. The reaper,
  the sweeper, the retirement, the withdrawal, the hook and the HTTP mapping are outside it. One final
  value for the lot is in the report. Runs holding a `service-admission` record then fail
  `SERVICE_PROFILE_AUTHORITY` and cannot be re-bootstrapped after a round opened (as in Parts 2, 3, 5).
- `run_id`, manifest, policy hashes, `wire_v2.json`, `rust_wire.json`, goldens: unchanged. New persisted
  shapes without a migration: record kinds `retired`, `exit-sanction`, `exit-failed`, `beacon-pause`,
  `round-exit`; `applied.kind = "null"`; `finality.disposition = "RETIRED"`; `anchor` records with
  `carried_from`; one escrow event kind. An older binary treats an unknown disposition as not closed (fails
  closed) and verifies a chain that holds withdrawals.

### Consequences

- The longest stall a peer can cause, in rounds after the event: commit, one push after `d_commit`; delta,
  one push after `d_upload`; leaves, until `absolute` (at most `d_final`); audit window, until `absolute - 1`;
  lease, Part 2; dispute, Part 1. After it the round is excluded-and-settled or retired and closed, the
  honest bonds are releasable and the next round can open. Before: for ever.
- Two silent members of six: both sanctioned at `d_commit`, 2000 units burned, four members audited and paid
  250,000 each, round 1 opens with the four. Two of five: both sanctioned, no 500, the round retired, round
  1 answers 409 `ROUND_INFEASIBLE` until four anchored members are eligible. A void round pays nobody and
  mints nothing, and a free identity in a roster of four can void every round it is in until it is
  sanctioned: the operator's default roster is five or six.
- An honest evicted miner is sanctioned like a free identity (ADR 0002 I5 and D40 say the opposite; v2 has
  no leave message to tell them apart). The pause witness and the zero-commit rule are the only
  exculpation, and there is no route back from `SUSPENDED` **[sign-off]**.
- Owners rotate between rounds only. A rollback across a retired round and a checkpoint that spans one are
  refused (fail closed). A rollback whose history holds an exit exclusion fails closed, as after
  `UNVERIFIED`.
- Existing tests that change deliberately: `test_seize_bond_burns_the_bond_of_a_no_show_and_nothing_else`
  (the state becomes `SUSPENDED`), the e2e no-show test that expects 409 `ROUND_INFEASIBLE` after the push
  (it expects `ROUND_RETIRED`), and every test that opens a funded dispute against an honest MATCH (the
  helper marks the lease row MISMATCH first). Each is updated in the commit that changes the behaviour.

### Rejected alternatives

1. A new `RoundOpenV2` without the silent members: `roster_hash`, slots and start states change, an opening
   is immutable. 2. The master opens the next round at the push: it needs the operator's schedule and a
   roster draw. 3. Sanction inside the exclusion transaction (decision 4). 4. A tape with no input
   (`min_length=4`: a wire change) or a zero-update outer step (it moves theta by momentum). 5. `exclude()`
   on an unapplied round (closes it, writes no `applied[w]`, so `w + 1` cannot open). 6. Carry-forward of the
   real post-round anchors (absent for unaudited members, unsound for error feedback). 7. A `ROUND_WITHDRAWN`
   table and a fifth snapshot element (old code then refuses the database), a sentinel row in
   `escrow_finalized` (turns on the live-reward gate), a signed withdrawal message (a new wire model), writing
   in the closing transaction (an owned function; leaves a frozen run frozen). 8. Writing the exclusion inside
   `verified_inputs_v2` (a read function used by two callers). 9. Tiering the sanction by an `accept` record or
   waiving a member that asked for an upload grant (a free identity signs an `accept`). 10. Detecting a paused
   window by gaps in the beacon table (the pusher fills gaps, the harness jumps). 11. A wall-clock deadline for
   any exit (Part 1). 12. Enforcing five at open (a new constant). 13. A grace period for a lapsed screen
   (renewal does not exist). 14. Treating a missing finality record as closed in `lock_settlement` (a corrupt
   record would release a bond).

### Decisions for Forge

1. `NO_SHOW` suspends and strikes (decision 12), as D44 says, against ADR 0002 I5 and D40; the alternative is
   bond-only (one keyword).
2. The window of decision 17 (`dispute_per_level` rounds from the first push that sees the verdict) and the
   grades of decision 16.
3. Whether `AUDIT_MISMATCH` rows are enforced in production before the auditor-side responder exists
   (decision 18).
4. A route back from `SUSPENDED` (decision 19): an operator amnesty route is the smallest honest step.
5. The operator's default roster floor (`--tolerate 1`).
6. The late-join anchor (Q3), which would make "a fifth miner becomes ACTIVE" literal.
7. D51 (2): the merged `GET .../challenge` of ADR 0010 is signed in headers; the `GET /disputes` route too.
   Both are converted to a signed body envelope in separate commits (D52).
8. A `TRAINING_FRAUD` ban that lands between the aggregate and the finalize of a round leaves that round's
   reward with the convicted coldkey (decision 23); the clean rule ("a banned coldkey's later origins
   mint into `burned`") belongs to the ledger lot.
9. The contest window counts beacon rounds, so a beacon outage consumes it; extend `due` by the pause
   length before production.
10. A retirement costs every roster member a round of work and reward, and a free identity in a roster of
    exactly four can void each round it is in until it is sanctioned: the bond prices it; document it in
    the operator guide.
11. A correlated honest outage (a region loss silencing several honest miners while the master is up) is
    sanctioned for all; no threshold is proposed, a majority attacker would use it.
12. Order with F1 (`v3-escrow-relay-fixes`): F1 first, as the Principal ordered. Whoever lands second
    resolves one-line conflicts in `escrow_v2.py`; with F1 present the withdrawal id uses the server key
    space and the chain-head term is redundant.

### Not verified

- A real pusher with real lag and the production freshness gate (only the test-mode predicate was read);
  CUDA; service-admitted runs (skipped by design); a retirement with an open dispute (follows from `closed`,
  not run); the real relay `release` after a retired round; a checkpoint that starts after one.
- `AUDIT_MISMATCH` against a CUDA host class; a hostile host that returns well-formed false leaves.
- Concurrency of the reaper with a long poll (read: it takes the store lock only).

### Sources

`hypertrain/src/hypertrain/challenge/{store,finality_v2,admission,disputes_v2,app}.py`, `ledger/escrow_v2.py`,
`aggregator/{weighted_v2,tape_v2}.py`, `protocol/messages_v2.py`, `scripts/ops/live_operator.py`,
2026-10-27. Lens C1 report (F2: R-C2, R-D; Lens m2), Lens synthesis (DSP-04), ADR 0009 section 4, ADR 0010
Part 6, ADR 0002 [A]. The design packages and their prototypes are scratch, not Git.
