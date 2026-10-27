# ADR 0011: audit integrity (settlement of blockages, numerics pin, host and image binding)

- Status: draft for branch `v3-audit-integrity`, 2026-10-27 (Principal SN100 D35, D36 bis, D38, D38 ter,
  D38 quater, D40; Lens red team B findings DSP-01, AUD-01, AUD-06, DET-01, DET-01b, QUAL-01, QUAL-02,
  EXE-01). Forge decides promotion to `v3`. This file grows one part per finding; each part is
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
   last configured auditor outside a cooldown; legacy `auditor-disabled` rows are inert.
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
  replacement inside the window, and Decision 8 gives one more window in every shape.

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
