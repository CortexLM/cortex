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
  contests; until then this is accepted (Q4). [V for the mechanics, H for the economics]
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
