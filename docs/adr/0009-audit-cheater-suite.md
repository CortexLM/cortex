# ADR 0009: audit-cheater regression suite

- Status: draft for branch `v3-audit-cheaters`, 2026-10-27 (Principal SN100 D35; scope added by
  Forge D37). Forge decides promotion to `v3`. The suite edits no `src/` file, no protocol
  constant, hash, pin, run, master, profile or emission. It ran on CPU, in process, with fixture
  keys: nothing was run on a GPU, on the s100 TEST stack or against any live host (see "Not
  verified").
- Order: brief `v3-audit-cheaters` ("prove the audit: honest miners MATCH, cheating miners are
  DETECTED and sanctioned"), plus Forge D37 (ledger/escrow defences, HTTP-level RTA-01..05, the
  relay's refused attempts) and its form rule: regression tests only, never an attack tool.
- Labels: **[V]** verified in code or by a command on the date above (a test id or `path:line`),
  **[A]** reported by a cited document or session, **[H]** hypothesis, **[?]** unknown.

## Context

The brief expects a caught cheater to be "sanctioned by the ledger (strike, suspension, coldkey
block, zero weight)". The code does that at one stage only. These facts decided the design.

- The audit is total and recomputed by the master. `schedule_audits_v2` leases one audit per
  roster entry, so there is no sampling (`challenge/store.py:6295-6446`); the master replays the
  assignment itself from master-staged inputs and sample ids (`execute_audit_v2`,
  `store.py:6897-7336`); a verdict is MATCH only if every published leaf preimage and the
  `leaves_root`, `delta_hash`, `final_theta_hash`, `ef_out_hash` and `ef_in_hash` equal the
  commit (`auditor/worker.py:196-210`). [V]
- The master checks little at upload time: window, assignment hash, `ef_in_hash`, leaf count,
  tokens (`store.py:6137-6253`, `messages_v2.py:372-379`); the leaves root, run and round of the
  leaf object (`store.py:6255-6293`); `delta_hash` and size against the relay's assembly and the
  master's own retrieval (`store.py:9886-9899`, `10101-10108`). Everything else, including
  altered weights, a forged delta, a copied or replayed result, a wrong shard, a wrong seed and a
  lie in `final_theta_hash` or `ef_out_hash`, is accepted at upload and caught only at audit. [V]
- A round MISMATCH has one consequence: `complete_audit_v2` writes `excluded: UNVERIFIED` (plus
  replay, verdict and contest records) and nothing else (`store.py:7600-7606`). No strike, no
  suspension, no ban, no burn. `strikes` is incremented only on a mismatching probation trial
  (`challenge/admission.py:733`) and read by no code; `Admission.settle_fraud` has no caller
  (`admission.py:1093`); `get_weights` ignores admission state. What the cheater loses is that
  round's reward and, as a side effect of the anchor chain, its place in round `w+1`
  (`open_round_v2` needs `_restore_anchor_v2(w-1)`, `store.py:5152-5155`). [V]
- At probation the master does strike: a proof that differs from its own replay of the trial
  gives SUSPENDED / PENDING_REPLAY_MISMATCH, one strike and a coldkey that cannot start another
  identity (`admission.py:662-761`, `158-166`). It also leaves the identity with no way back
  (PRB-01 below). [V]
- `finalize_v2` splits the round budget among the included miners only
  (`store.py:8336-8351`), so a caught miner's slice is paid to its peers. `docs/mechanism.md`
  section 4 says it burns; that text describes the v1 ledger. [V]
- No existing test ran a cheating `NetworkMiner` through the v2 HTTP round. [V: the harness
  exploration found only v1 and unit-level cheat tests]

## Decision

### 1. A regression suite in `hypertrain/tests/audit_cheaters/`, no production code

Cheating miners are **test doubles** that replace one step of an otherwise honest
`NetworkMiner.run_round`: they patch `island_launch.launch_island`, the torchrun worker (through a
shim), the staged job or the signed wire bodies. Nothing in `src/` carries a fault-injection hook.

| File | Role |
| --- | --- |
| `ac_world.py`, `conftest.py` | The in-process world: real challenge app, in-process relay, five graduated identities snapshotted once and restored per test in 0.1 s; `settle()` records every refusal and stall as data. |
| `ac_cheats.py`, `ac_cheat_worker.py` | 22 named cheat doubles and the honest control, each with its brief number, seam and expected first refusal. |
| `ac_precondition.py` | `PreconditionError`: a broken scenario setup is a failure, never hidden behind a strict xfail. |
| `test_cheats_audit.py` | Audit level: each double through the production `execute_island_audit`, first bad leaf asserted, plus the honest false-positive sweeps. |
| `test_cheats_through_rounds.py`, `test_replay_and_continuation_rounds.py`, `test_absence_and_eviction_rounds.py`, `test_probation_stage_cheats.py` | Service level: every cheat through intake, audit, aggregate, finalize and `get_weights`; replay and continuation over two rounds; absence and honest eviction; the probation stage. |
| `test_missing_sanctions_and_wedges.py`, `test_finalize_and_epoch_service.py` | The holes after the audit and the finalize/epoch races. |
| `test_admission_defences_http.py`, `test_relay_refusals.py`, `test_ledger_defences.py` | Forge D37: RTA-01..05 over HTTP, the relay's refused attempts, double credit, empty and unaudited epochs. |
| `test_world_baseline.py`, `test_evidence_report.py`, `test_mutation_runner.py` | The harness, the table renderer and the mutation runner tested themselves. |
| `scripts/check_audit_mutants.py`, `scripts/audit_mutants_table.py` | The mutation check of the detection paths. |
| `scripts/audit_cheaters_report.py` | Renders the recorded evidence as the cheat x outcome table. |

Rules the suite follows (the form Forge asked for): a test states the defence in its name and
docstring and asserts it; a hole is `xfail(strict=True, raises=AssertionError)` whose body is the
minimal reproducer asserting the **desired** defence, so it flips red the day the hole closes and
a failure of the setup cannot pass as the expected failure; a contested behaviour (below) is never
asserted as the contract; fixture keys come from `bytes([n]) * 32`.

### 2. How to run it

```bash
cd hypertrain
# audit-level, relay, admission, ledger, harness tests (CI: about 6 minutes in the shards)
uv run --frozen pytest -q tests/audit_cheaters
# service-level scenarios: opt-in, about 25 minutes (a cold run adds 3 minutes of graduation)
AUDIT_CHEATERS_FULL=1 AUDIT_CHEATERS_SNAPSHOT_DIR=/path AUDIT_CHEATERS_EVIDENCE=evidence.jsonl \
  uv run --frozen pytest -q tests/audit_cheaters
uv run --frozen python scripts/audit_cheaters_report.py evidence.jsonl
# mutation check of the detection code paths
AUDIT_CHEATERS_FULL=1 AUDIT_CHEATERS_SNAPSHOT_DIR=/path \
  uv run --frozen python scripts/check_audit_mutants.py
```

The service-level tests need five graduated identities because the 4-miner fixture cannot absorb
one exclusion (AC-CLIFF). The device is CPU; `AUDIT_CHEATERS_DEVICE` other than `cpu` skips with
"the GPU path needs the qualified CUDA TEST stack; not run here". The doubles are
device-agnostic, so the GPU run needs a qualified host and the same worlds, not new cheats. The
service-level tests are opt-in like the GPU tests so that the shards of `hypertrain-ci` keep
their budget; a dedicated CI job is proposed in "Consequences".

### 3. Result: cheat x outcome

Rendered by `scripts/audit_cheaters_report.py` from the evidence the service-level tests recorded
on 2026-10-27 (CPU, tiny model, H=1 J=1, 5 graduated identities, 4 honest miners per round). Stage
`round` is run-v2 through audit, aggregate, finalize and `get_weights`; stage `probation` is the
admission trial. "first refusal" is where the master first said no; `accepted` means the cheat
went through upload and only the audit can catch it. Cheat 9 is a double outside the brief's
eight (a rostered miner that stops the audits of the others). `metrics_root` is not in the table:
a lie in it is accepted by every stage (AC-METRICS-ROOT, tested at audit level).

| cheat | double | stage | first refusal | audit | cheater after the round | honest miners | hole |
|---|---|---|---|---|---|---|---|
| 0 | honest | probation | accepted | - | PROBATION, 0 strike(s) | not applicable |  |
| 1 | altered-weights-one-ulp | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | established identity record unchanged | PRB-01 |
| 1 | altered-weights-one-ulp | round | accepted | MISMATCH at leaf 1 | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION |
| 1 | altered-weights-state-file-only | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | established identity record unchanged |  |
| 1 | altered-weights-state-file-only | round | accepted | MATCH | ACTIVE, 0 strike(s); weight 200000 | weights 200000/200000/200000/200000; strikes 0 |  |
| 2 | replay-previous-round-relabelled | round | accepted | MISMATCH at leaf 0 | excluded UNVERIFIED; ACTIVE, 0 strike(s) | weights 250000/250000/250000/250000 | AC-SANCTION, AC-REDISTRIBUTION |
| 2 | replay-previous-round-verbatim | round | refused 422 POST /leaves: leaf object does not match accepted commit | - | - | - |  |
| 2 | replayed-earlier-trial-proof | probation | 409 POST /join/proof WORK_SCREEN_BINDING | - | PROBATION, 0 strike(s) | not applicable |  |
| 2 | replayed-round-without-relay-upload | round | - | - | - | - | AC-ACCEPTANCE-BINDING |
| 3 | forged-delta-valid-hash | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | established identity record unchanged |  |
| 3 | forged-delta-valid-hash | round | accepted | MISMATCH | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION, AC-COLDKEY, AC-CLIFF, AC-CLIFF-LIVENESS, AC-BAN |
| 4 | copied-peer-upload-resigned | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | peer stays PROBATION, clean_count 1, no strike |  |
| 4 | copied-peer-upload-resigned | round | accepted | MISMATCH at leaf 1 | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION |
| 4 | copied-peer-without-relay-upload | round | never sent upload-grant | MISMATCH at leaf 1 | excluded UNVERIFIED; ACTIVE, 0 strike(s) | - | AC-ACCEPTANCE-BINDING |
| 5 | skipped-optimizer-step | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | established identity record unchanged |  |
| 5 | skipped-optimizer-step | round | accepted | MISMATCH at leaf 1 | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION |
| 5 | truncated-step-count | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | established identity record unchanged |  |
| 5 | truncated-step-count | round | refused 500 POST /commit: Internal Server Error | - | - | - | AC-COMMIT-500 |
| 6 | wrong-data-shard | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | established identity record unchanged |  |
| 6 | wrong-data-shard | round | accepted | MISMATCH at leaf 1 | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION |
| 6 | wrong-initial-theta | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | established identity record unchanged |  |
| 6 | wrong-initial-theta | round | accepted | MISMATCH at leaf 0 | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION |
| 6 | wrong-seed-forged-rng-ctr | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | established identity record unchanged |  |
| 6 | wrong-seed-forged-rng-ctr | round | accepted | MISMATCH at leaf 1 | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION |
| 7 | commit-lie-delta-bytes | round | refused 409 POST /upload-grant: one current signed relay assignment required | - | - | - |  |
| 7 | commit-lie-delta-hash | round | refused 409 POST /upload-grant: one current signed relay assignment required | - | - | - |  |
| 7 | commit-lie-ef-out-hash | round | accepted | MISMATCH | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION |
| 7 | commit-lie-final-theta-hash | round | accepted | MISMATCH | excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0 | weights 250000/250000/250000/250000; strikes 0 | AC-SANCTION |
| 7 | commit-lie-leaves-root | round | refused 422 POST /leaves: leaf object does not match accepted commit | - | - | - |  |
| 7 | proof-lie-leaves-root | probation | accepted | - | SUSPENDED, 1 strike(s); pending dispute | not applicable |  |
| 8 | commit-without-delta | round | never sent upload-grant | MATCH | ACTIVE, 0 strike(s); weight 0 | weights 0/0/0/0; unsanctioned | AC-LIVENESS-commit-without-delta |
| 8 | commit-without-leaves | round | never sent leaves | none (stalled at lease) | ACTIVE, 0 strike(s); weight 0 | weights 0/0/0/0; unsanctioned | AC-LIVENESS-commit-without-leaves |
| 8 | evicted-during-trial | probation | accepted | - | SUSPENDED, 0 strike(s) | - |  |
| 8 | evicted-during-trial | round | - | - | SUSPENDED, 0 strike(s) | - | AC-EVICTED-TRIAL |
| 8 | evicted-mid-launch | round | accepted | MATCH | ACTIVE, 0 strike(s); weight 200000 | weights 200000/200000/200000/200000; unsanctioned |  |
| 8 | evicted-then-back-after-d-commit | round | refused 409 POST /commit: commit window closed | none (stalled at audits) | ACTIVE, 0 strike(s); weight 0 | weights 0/0/0/0; unsanctioned |  |
| 8 | late-commit | round | refused 409 POST /commit: commit window closed | none (stalled at audits) | ACTIVE, 0 strike(s); weight 0 | weights 0/0/0/0; unsanctioned |  |
| 8 | no-show-before-commit | round | never sent commit | none (stalled at audits) | ACTIVE, 0 strike(s); weight 0 | weights 0/0/0/0; unsanctioned | AC-LIVENESS-no-show-before-commit |
| 9 | non-finite-leaf-metric | round | accepted | none (stalled at lease) | ACTIVE, 0 strike(s) | - | AC-WEDGE-LEAF |
| 9 | short-lived-commit | round | accepted | none (stalled at lease) | ACTIVE, 0 strike(s) | - | AC-WEDGE-EXP |

Reading it against the brief:

- **Detected.** Every cheat that passes upload (1, 2 relabelled, 3, 4, 5, 6, 7 final-theta and
  ef-out) is a MISMATCH at the audit, at the leaf the Merkle windows predict (leaf 1 at H=1, leaf
  0 for a wrong start state; `test_cheats_audit.py` proves the positions 1, 2, 2, 1 for skips and
  ulp edits at steps 1..4 of an H=4, J=2 round). Cheats refused at upload (2 verbatim at
  `/leaves`, 7 leaves root at `/leaves`, 7 delta hash and bytes at `/upload-grant`, 5 truncated
  at `/commit`) never reach the audit. A one-ulp edit of the state file alone changes nothing the
  network consumes (inert, paid like an honest miner).
- **Sanction.** At the round stage the cheater loses that round's reward, its slice goes to the
  peers, and it has no anchor for round `w+1`. It stays ACTIVE with 0 strikes, keeps its stake
  and its earlier rewards (AC-SANCTION). At the probation stage the strike, the suspension and the
  coldkey block exist (cheats 1, 3, 4, 5, 6, 7), and nothing releases them (PRB-01).
- **Honest miners.** In every round with a caught cheater the four honest miners are MATCH,
  ACTIVE, unstruck and paid equally; the payout assertions hold under burning or redistributing.
- **Cheat 8.** No absence class is sanctioned (no strike, ban, slash or UNVERIFIED exclusion), and
  an evicted miner that reruns before `d_commit` is UPLOADED, MATCH and paid. But every absence
  class stalls the round for the honest miners (AC-LIVENESS), and a missed trial suspends the
  applicant for good (AC-EVICTED-TRIAL).

### 4. Holes found, with their reproducers (strict xfail) and the proposed check

Verified by independent agents that tried to refute each claim (the severity is theirs).
`path:line` are at `65be1962`. No check is added to `src/` by this ADR; each proposal needs the
owners' decision.

| Id | Severity | Claim [V] | Test | Proposed check |
| --- | --- | --- | --- | --- |
| AC-SANCTION | IMPORTANT | A round MISMATCH is not sanctioned beyond the round: `complete_audit_v2` never touches admission (`store.py:7600-7606`), strikes are write-only (`admission.py:733`). | `test_missing_sanctions_and_wedges.py::test_a_caught_round_cheater_is_not_left_active` | **P1**: suspend with a pending reference through the existing resume path; ban and burn only on a FRAUD resolution, so honest hardware drift (DET-01) is not banned. |
| AC-BAN, RTA-06 | IMPORTANT | A lost FRAUD contest burns only the contest bond; `settle_fraud` has no caller (`admission.py:1093`) and cannot be wired without a settlement record for `LOCK_ADMISSION` (`store.py:2318-2333`); the admission stake is neither burned nor releasable. | `test_a_lost_fraud_contest_ends_in_a_banned_admission`, `test_ledger_defences.py::test_lost_fraud_contest_burns_the_miners_admission_lock` | P1 |
| AC-CLIFF, AC-CLIFF-LIVENESS | IMPORTANT (BLOQUANT if the live roster has exactly four cap slots [?]) | With four cap slots one exclusion makes `aggregate` answer a bare 500 (`weighted_v2.py:103-104`, unmapped at `app.py:425-432`) and the run cannot go on: round `w` cannot be reopened, `w+1` needs `applied`, rollback needs an applied round. The protected-service open path requires exactly four actors (`store.py:4948`). | `test_one_exclusion_in_a_roster_of_four_is_a_handled_refusal_not_a_server_error`, `test_a_roster_of_four_run_can_go_on_after_one_exclusion` | **P2**: refuse at open a roster that cannot reach Q after one loss, or add a signed supersede transition; map the error to a 409. |
| AC-WEDGE-LEAF, AC-WEDGE-EXP | IMPORTANT | A rostered miner stops every other miner's audits at no cost: a NaN metric under a correct root, or a commit envelope that expires before `d_audit`, is accepted at intake (`store.py:6255-6293`, `6137-6253`) and `AuditJobV2` raises inside `lease_v2` (`messages_v2.py:473-476`, `479-497`). The row stays first, rows after it are never leased, and at the horizon one exhausted row marks the round EXCLUDED (`store.py:6668`), so nobody is paid. Position in the queue does not matter. | `test_a_non_finite_leaf_cannot_stop_the_audits_of_the_others`, `test_a_short_lived_commit_cannot_stop_the_audits_of_the_others` (head, middle, tail) | **P3**: per-row failure isolation in `lease_v2` and `schedule_audits_v2`; refuse non-finite metrics at `/leaves` and `exp_drand < d_audit` at `/commit`. |
| AC-LIVENESS | IMPORTANT | One rostered miner that does not commit, publishes no leaves or uploads no delta stalls the whole run, unpunished: `schedule_audits_v2` 409 for everyone (`store.py:6387`), `lease_v2` 409 at the head of the queue (`6593`), `verified_inputs_v2` 409 (`7904`). Reopening `w` or opening `w+1` is refused; no operator route unsticks it. | `test_absence_and_eviction_rounds.py::test_the_honest_round_settles_without_the_absent_miner` (no-show, commit-without-leaves, commit-without-delta; late commit is the no-show branch) | **P4**: deadline-driven exclusion of a member the master can prove absent, with a defined outcome below four survivors. |
| AC-EVICTED-TRIAL, PRB-01 | IMPORTANT | SUSPENDED is absorbing. A missed trial window (`admission.py:216-228`) or a trial mismatch (synthetic dispute id no settlement can exist for) leaves no way back, blocks the coldkey (`admission.py:162-166`) and freezes the stake. An honest evicted applicant and an honest AVX-512 drift are punished like a cheater. | `test_an_honestly_evicted_applicant_is_not_locked_out_by_its_missed_trial`, `test_probation_stage_cheats.py::test_a_miner_suspended_by_a_trial_mismatch_has_a_defined_way_back` | **P5**: close an expired trial without suspending and add a signed re-seed (ADR 0002 decision 6); bounded forgiveness or a referee rerun for a trial mismatch. |
| FIN-1 | IMPORTANT | An expired audit lease cannot be taken over for H <= 30: `AuditJobV2` caps `lease_expires` at `created_beacon + 100` (`messages_v2.py:449-451`) and `created_beacon` is `d_audit`; the silent auditor's miner is excluded as INFRASTRUCTURE. The S7 rule (`d_final - d_audit <= 100`) hides the symptom. | `test_finalize_and_epoch_service.py::test_expired_audit_lease_is_taken_over_by_the_other_auditor_and_the_round_settles` | **P6**: anchor the cap at the lease start. |
| FIN-2 (reframed) | MINEUR | `open_round_v2` accepts a RoundOpenV2 whose `d_final - d_audit` outlives the audit horizon (`store.py:5117-5118` checks only `>= 2`); only the operator tool and the service path refuse it. | `test_round_opening_whose_final_deadline_outlives_the_audit_horizon_is_refused` | **P6**: refuse at open. |
| FIN-3, LED-1 | IMPORTANT (availability; no corruption shown) | Production runs routes in threads on one SQLite connection; `aggregate_v2`, `round_view_v2`, `verified_inputs_v2`, `open_round_v2` and `Store.weights` read it before taking the store lock (`store.py:8063`, `6086`, `2064-2074`), so concurrent requests get spurious 409/500. The public round view is the realistic trigger. | `test_finalize_and_epoch_service.py::test_requests_use_the_shared_connection_only_under_the_store_lock` (aggregate, round_view, get_weights; deterministic lock audit) | **P7**: take the lock before any access to the shared connection. |
| RTA-01..05, RTA-10 | IMPORTANT | Lens' admission findings, reproduced over HTTP (rotation without the new hotkey's consent, unfunded pending places held for ever, never-proved trials block all trials, a non-owner opens or expires a rival's trial, an accepted proof suspended before it is finalized, prefix burst above `max_pending`). The fixes are the lot `v3-admission-hardening`, not on this base. | `test_admission_defences_http.py` (8 strict xfails with controls that pass) | Lens' one-line fixes. |
| AC-COLDKEY | MINEUR | A caught round cheater's coldkey joins a new hotkey at once; follows AC-SANCTION. | `test_a_caught_cheater_cannot_start_another_identity_with_the_same_coldkey` | P1 |
| AC-ACCEPTANCE-BINDING | MINEUR (defence in depth) | `/delta` compares only the acceptance's `delta_hash`, not its `w`, hotkey or the manifest's `grant_hash` (`store.py:6192-6200`): a delta can borrow another miner's or another round's acceptance and skip its own relay upload. The audit still returns MISMATCH and the master holds the bytes. | two strict xfails and `test_a_delta_that_borrowed_an_acceptance_still_earns_nothing_at_the_audit` | **P8** |
| AC-COMMIT-500, RELAY-01 | MINEUR | A bare `ValueError` (`CommitV2.validate_assignment`, `UploadChunkManifest.validate_grant`) is an unhandled 500; nothing is stored. | `test_a_truncated_step_count_is_refused_with_a_client_error_at_commit`, `test_relay_refusals.py::test_grant_with_a_swapped_chunk_manifest_is_refused_with_a_reason` | P8 |
| AC-METRICS-ROOT | MINEUR (inert) | `CommitV2.metrics_root` is signed but the live audit never compares it (`auditor/worker.py:202-209`); the trial publication path does (`admission.py:575-578`). Its loss and norm are verified through the leaves root, so a lie earns nothing today. | `test_cheats_audit.py::test_a_lie_in_metrics_root_is_refused` | P8: one equality clause, or drop the field. |
| RTA-09 | MINEUR | An unbounded `epoch_at` or `epoch` journals a newest answer and every later honest epoch answers `out_of_order` for good; needs the internal bearer. | `test_ledger_defences.py::test_far_future_epoch_at_does_not_freeze_later_honest_epochs` (and the `epoch` twin) | Lens' fix. |

Design decisions the code and the docs do not settle (no xfail asserts an answer):

- **Burn or redistribute.** `finalize_v2` pays a caught or absent miner's slice to the included
  miners; `docs/mechanism.md` section 4 says it burns and is never redistributed (v1 ledger text).
  The suite's payout assertions are neutral. The maintainers choose, then fix one of the two.
- **Payment lag (LAG-1).** An epoch asked while the master's pushed beacon is behind the unit's
  maturity is answered 200 unpaid and is then final; the unit pays one epoch later, never lost
  (`test_epoch_asked_while_the_master_beacon_lags_maturity_pays_each_unit_once`). The promise of
  `docs/network-v2.md:266-269` is true only once the master's beacon has caught up.

### 5. False-positive control

An honest miner is never flagged in this suite: **552 audited honest runs, 0 flagged** at audit
level (540 through `audit_island` over 3 state policies, 6 geometries, 5 seeds and chained
rounds; 12 through the production `execute_island_audit` on real torchrun) and every honest
miner of the service-level rounds (at least 23 recorded rounds with four or five honest miners
paid) was MATCH. Benign variations give byte-identical artifacts: another work directory, a
restart over an existing `published/`, another `PYTHONHASHSEED`. The sweep is a smoke test with a
known power (rule of three: a 0.6% false-positive rate would be seen with 95% confidence in the
540; 25% in the 12) and is **single-host evidence**: miner and master replay share one machine
and one environment. It does not prove cross-host, cross-ISA or CUDA stability (DET-01: the audit
is bitwise and nothing pins `ATEN_CPU_CAPABILITY`, `MKL_CBWR` or `ONEDNN_MAX_CPU_ISA`; the s100
TEST run flagged an honest AVX-512 miner in 1 trial of 6). A fix of AC-SANCTION must not ban on
a round MISMATCH alone for that reason.

### 6. Mutation check

`scripts/check_audit_mutants.py` runs each mutant in a private copy of the tree (never in the
working tree) and requires the named tests to fail; its own verdicts are proved on a toy tree
(`test_mutation_runner.py`) and the table is checked against the tree on every run
(`test_the_real_table_matches_the_real_tree`). The table has five rows, the comparison of the
production audit (`auditor/worker.py:196-210`): leaf preimages, leaves root, delta hash, final
theta hash, EF out hash. On 2026-10-27 all five mutants were killed (22 to 26 s each). The intake, sanction and ledger defences were
mutation-proved by hand in private copies by the authors and independently re-checked by
reviewers; they are not yet rows of the table (about 60 mutations are listed in `REPORT.md`).

## Consequences

- The service-level scenarios take about 25 minutes (CPU, 4 to 5 miners per round, 3 minutes of
  graduation when cold) and are opt-in (`AUDIT_CHEATERS_FULL=1`). The default run adds about 6
  minutes to the `hypertrain-ci` shards. A dedicated job is proposed, not added (a workflow I
  cannot run is not edited in this lot): `runs-on: blacksmith-8vcpu-ubuntu-2404`,
  `timeout-minutes: 60`, `env: AUDIT_CHEATERS_FULL: "1"`, step
  `uv run --frozen pytest -q tests/audit_cheaters`, then the mutation check.
- Every strict xfail flips the run red the day its hole is closed; the fix lot removes the
  marker, which turns the reproducer into a normal regression test. Several tests share a root
  cause (P1, P3, P4): one fix may flip several.
- The fixture rounds open with `d_final - d_audit = 269`, which `live_operator.py` refuses; the
  findings above were re-run by a verifier with a gap of 100 and do not depend on it, but a
  future refusal at open (P6) would turn the scenarios red until `World.open_round` uses a
  compliant gap.
- No `src/` file, constant or hash changes (`implementation_hash` and `sources()` cover
  `src/hypertrain/**`, which this lot does not touch).

## Rejected alternatives

- **Fault-injection hooks in `src/`.** The suite would exercise a code path production never
  takes, and a hook is itself an attack surface. Rejected: doubles at the miner's seams.
- **CLI-subprocess miners (`mine_smoke_v2.py`, `mine_multi_round_v2.py`).** The seam is
  unreachable from outside the process and the first refusal stops the run. They stay the
  smoke test of the real CLI.
- **One long chain with all cheats in one roster.** Each cheater is excluded and loses its
  anchor; a stalled cheat blocks everyone. One cheater per clone of a graduated snapshot keeps
  every scenario independent, at 0.1 s per restore.
- **A fixed `/tmp` snapshot cache.** The graduated snapshot lives under pytest's temp path or
  `AUDIT_CHEATERS_SNAPSHOT_DIR`; `/tmp` is shared with other sessions and was full during this
  work.
- **Asserting today's payout split (4 x 250000).** It would pin a contested rule (AC-REDISTRIBUTION).
  The payout assertions hold under both designs.

## Not verified

- GPU and the s100 TEST stack: nothing ran there. `AUDIT_CHEATERS_DEVICE` other than `cpu` skips.
- Production roster size, and whether the live run is service-admitted: unknown (AC-CLIFF
  severity depends on it).
- H > 30 behaviour of FIN-1 (code-derived, not run); service-admitted runs (`schedule_audits_v2`
  re-intakes the commit there, a short-lived commit probably fails scheduling outright).
- Real SIGKILL of a torchrun process and a real Spot eviction (the double leaves the same
  directory); a real uvicorn socket (the harness uses the TestClient, and the 500s are recorded
  with `raise_server_exceptions=False`).
- A fix for any hole: the xfail assertions were shown satisfiable by crude simulations in private
  copies, not by a real implementation.
- Platform safety controls: none blocked any step of this lot.

## Sources

- Brief `v3-audit-cheaters.md` and `v3-common-impl.md` (Forge, 2026-10-26/27); Forge D37 scope
  message (2026-10-27).
- Lens red team A: `livrables/lens/2026-10-27_cortex-redteam-A-session-report.md` and
  `redteam-live/rtA.md` (RTA-01..10), `redteam-live/rtB.md` (DET-01, QUAL-01, EXE-01, EB-01).
- Code at `65be1962` (`origin/v3`) with the line cites above; `docs/network-v2.md`,
  `docs/mechanism.md`, `docs/protocol.md`; ADR 0002 (eviction-tolerant core).
- Evidence of this lot: the test ids named above, `REPORT.md` of the worktree.
