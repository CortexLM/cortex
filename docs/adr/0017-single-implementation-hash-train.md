# ADR 0017: one `implementation_hash` change for the eight D150 findings (the hash train)

- Status: draft, 2026-11-01 (Principal SN100 D150, brief `adr-0017-consolidated`). The founder decides the
  train (F14); Forge decides promotion to `v3`; Lens reviews it before Principal SN100. Written for branch
  `adr-0017-consolidated` on origin/v3 `dc014508`. Documentation only: no file under `hypertrain/src`, tests
  or scripts changes [V], so no hash moves in this lot. Nothing here ran on s100, a node, a GPU, R2 or
  on-chain; the hash values were measured on `git archive` trees on 2026-11-01.
- Number: 0017 is free on `dc014508` (`docs/adr/` holds 0001-0003 and 0006-0016) [V]. Two pushed, unmerged
  branches each carry a draft numbered 0017 with an index line: `0017-admission-trial-window.md`
  (`v3-trial-rounds` `f4f2bc2c`) and `0017-miner-fast-path.md` (`v3-miner-fast-path` `f6b9ce02`) [V]. A draft
  that merges after this record must be renumbered; Forge decides which.
- Order: D120 (one hash change, one ADR 0017, golden roots regenerated once), D128 (an owned-body fix goes to
  the train), D147-D149 (the S31 findings), D150 (this record: eight findings and a one-page sheet for the
  founder). The numbers below are D150's; D148 and D149 called findings 6 and 7 "n°7" and "n°8".
- Labels: **[V]** proved: read in code, run as a command or pinned by a test, at `dc014508` unless a commit is
  named. **[A]** reported by a cited document; a measure from a run says how many times it was observed.
  **[H]** inferred, or arithmetic on cited figures. **[?]** unknown. A figure that no file holds is written
  "not measured".
- Paths: code paths are relative to `hypertrain/src/hypertrain/`; scripts and tests are written in full.
  `livrables/…` is the private operator share (`/var/lib/openbot/OpenBot/Shared/Cortex/livrables/`), not in
  Git. `agents/forge.md` was rewritten on 2026-11-01 (D152): its entries of 28-31/10 are cited from
  `livrables/forge/journal-2026-10.md`, `journal-2026-10-31-fiche-archive.md` and
  `journal-2026-11-01-fiche-archive.md` (below "journal-10", "journal-31" and "journal-01"). S31 is
  `livrables/forge/2026-10-31_s31-beacon-validation/` ("S31/").
- Related: ADR 0011 (Part 3 hash tables, Part 5 the cohort cap, Part 8 the pause witness, Part 9 the gap cap),
  0012 (lock-free reference), 0013 (results through R2), 0014 (Consequences), 0015 (the 101 M budget), 0016
  (the 31 M run profile).

## Context

### What `implementation_hash` binds, and where it bites

- `ChallengeStore._service_implementation_v2` hashes the `ast.dump` of 48 owned names of `challenge/store.py`,
  the bytes of 12 files and the manifest's environment pin (`challenge/store.py:5127-5140`, `:5162-5229`) [V].
  The files are `aggregator/core.py`, `tape_v2.py`, `rollback_v2.py`, `capacity_worker.py`,
  `trainer/compress.py`, `config.py`, `model.py`, `determinism.py`, `models/registry.py`,
  `miner/island_launch.py`, `auditor/replay.py` and `protocol/env_pins.py`. Not hashed, among the files this
  record names: `protocol/messages_v2.py`, `aggregator/weighted_v2.py`, `challenge/admission.py`,
  `challenge/app.py`, `challenge/exit_v2.py`, `challenge/audit_cooldown_v2.py`, `auditor/worker.py`,
  `auditor/__main__.py` and `hypertrain/scripts/` [V].
- At `dc014508` it is `24fbe36d…c185ce` under the pin of the s100 TEST runs (`avx2`/`sm_103`/`math`) and
  `7af135ca…70be62` under the example pin (`avx2`/`none`); the three other pins give ADR 0016's values [V,
  `livrables/forge/2026-10-28_last-merges/bin/hash_consequences.py`, CPython 3.12.15]. `24fbe36d…` holds on
  twelve trees from `708414d8` to `dc014508`, one per hash-neutral delivery since then [V].
- It is in no manifest and no run id, and no route serves it [V]. Only a run with a `service-admission` record
  (only the tiny CPU profile can hold one, `store.py:5092-5108`) or a roster above 16 compares it
  (`:5523-5540`, `:5557-5562`). Such a run then fails 409 `SERVICE_PROFILE_AUTHORITY` and cannot take a new
  profile (`SERVICE_PROFILE_IMMUTABLE`, `hypertrain/tests/ledger_invariants/test_bounds_2.py:709-727`) [V]:
  ADR 0011 says these runs are recreated (`0011-audit-integrity.md:557-559`); ADR 0014's "until the run is
  re-bootstrapped" (`0014-escrow-operation-ids-and-relay-grants.md:476-477`) does not match the code [V].
- The s100 TEST runs hold no such record: `service_admission_rows` 0 for `0ef600b4` and `4d84aacc`
  (`S31/evidence/ident-0ef600b4-before.json:73`, `ident-newrun.json:45`) [A]. One test pins the value:
  `IMPLEMENTATION_HASH` of `hypertrain/tests/ops/s100/test_run_0ef600b4_golden.py:23`, recomputed from the
  tree at `:53-60` [V].

### Why one train

- D128 (31/10 02:25Z): an owned-body fix is not wrapped or rebound to keep the hash; it goes to the train
  (journal-31 `:503`, `:506`; `livrables/principal-sn100/journal-2026-10.md:213`) [A]. The
  v3-weights-hardening REPORT attributes the refusal of the rebinding to "founder; D128"
  (`…/2026-10-31_v3-weights-hardening/REPORT.md:269`); the texts found are Forge's relay answer and D128 [A].
- D120 (30/10 10:40Z): "un seul changement de hash, une seule ADR 0017, golden roots régénérés une fois"
  (journal-31 `:455`) [A].
- Each owned edit alone gives its own value: the entry gate of finding 4 gives `8235e5f5…84b5a8`; edits probed
  on a scratch copy gave `9047a425…` (the lock of finding 5), `0e547d6c…` (another `absolute`) and `ea4120c9…`
  (a third lane) [V, 2026-11-01]. The value of the train is known only once its content is frozen [H].
- F14 is the founder decision these findings feed (`livrables/principal-sn100/journal-2026-10.md:220`,
  `livrables/briefings/2026-11-01_briefing-sn100.md:47`). D156: no new node loan before it (`…:96`) [A].

### The eight findings at a glance

| # | Finding | Observed | Fix lives in | Needs the train |
| --- | --- | --- | --- | --- |
| 1 | trial window `challenge_rounds` = 100 | S17c: 4 trials; S31: 57 launches | `messages_v2.py`, `admission.py`, `miner/core.py` (`v3-trial-rounds`) | no (measured); new images |
| 2 | audit window, two auditors, two lanes | 3 rounds at gap 175-200 failed; 4 at gap 400 passed | `schedule_audits_v2`, `lease_v2` (owned) | yes, for more auditors or lanes |
| 3 | first audit window `d_audit + 200` | 4 of 4 gap-400 rounds | `schedule_audits_v2` (owned); the 500 in `app.py` | yes for the bound; no for the 500 |
| 4 | finalize gate judged after its replay | two 503 on `0ef600b4` round 0; a strict xfail | `finalize_v2` (owned) | yes (`8235e5f5…` alone) |
| 5 | `_service_charge_v2` guard read without the lock | two strict xfails; never in a run | `_service_charge_v2` (owned) | yes |
| 6 | PROBATION cohort cap Q/4 | S31 (a PROBATION-only round cannot open) | policy literals, `weighted_v2.py`, `admission.py` | no for the cap or threshold; a bootstrap path [?] |
| 7 | beacon push during a replay | once in two rounds | a push process apart; the replay out of process (owned) | only for the replay |
| 8 | node → R2 path; AUDIT_INFRASTRUCTURE each round | S31 | infrastructure; see finding 3 | no |

## Findings

### Finding 1. The trial window is fixed; the trial's time moves with the day's network path

The window is a schema literal: `AdmissionPolicyV2.challenge_rounds: Literal[100]`
(`protocol/messages_v2.py:665`), `JoinChallenge` requires `deadline_beacon = seed_beacon + 100` (`:604-607`)
and `Admission.challenge` signs `seed + 100` (`challenge/admission.py:442`), at 3 s a round
(`messages_v2.py:729`): about 297-300 s [V].

- **31,462,656 parameters, S17c, 2026-10-29**: four real trials, one cold and three warm, all
  `FULL_REFERENCE_MATCH`, no override. Challenge receipt to finalize 228, 225, 222 and 222 s; slack under the
  window 19, 6, 16 and 19 rounds (57, 18, 48 and 57 s); `T_ref` 136.454, 132.371, 132.962 and 131.932 s; the
  1,290,506,240-byte result uploaded to R2 in 35.2, 34.0, 34.4 and 32.8 s
  (`livrables/forge/2026-10-28_s17-run-101m/s17c/REPORT-S17C.md:44-50,61,73-76`) [A, four trials]. ADR 0016
  estimated 194 s; the margin under the window was 23.2-25.3 %, not 34.7 % (`:90`) [A]. Forge's journal has
  the first three as "228 / 225 / 222 s … 19 / 6 / 16 rounds" (journal-10 `:1183`) [A].
- **Same model, S31, 2026-10-31**, run `4d84aacc`, one lane: 57 launches, 44 clean, 13 lapses
  (`S31/REPORT-S31.md:93`) [A]. Reference uploads (executor journal): 34.191-37.839 s for the nine published
  attempts up to the one started at 14:11:23Z, then 59.255 s (14:16:37Z) and 47.688-85.996 s up to 15:53:58Z,
  with one `R2Error` and one `StoreError`: 25 uploads on that bucket, median 54.806 s (`:132-160`, `:203`)
  [A]. Forge's test at 14:55Z: 19-56 MB/s on eight streams of 64 MiB, 4.2-4.9 MB/s on one stream of 256 MiB,
  16.6 MB/s from s100 (`:113`; journal-01 `:154`) [A, one measure]. On 28/10 nodes A and B gave PUT 101-122
  and GET 140-144 MB/s on eight streams of 128 MiB, and 19.2-26.7 MB/s on one stream of 1 GiB
  (`livrables/forge/2026-10-28_transfer-measure/REPORT.md:8-11`) [A]; the sizes differ, so the drop is
  indicative [H].
- **Outcome by period**: 13:21-14:16Z 8 clean and 2 lapses (the cold first reference of each fresh node);
  14:16-15:58Z 8 clean and 10 lapses; 15:58-18:42Z, on an APAC-hinted bucket, 28 clean and 1 lapse
  (`REPORT-S31.md:95-99`) [A]. At 15:45Z Forge counted, since 14:16Z, 15 trials, 6 MATCH and 9 lapses (60 %),
  about 4.3 clean trials an hour (journal-01 `:160`; `REPORT-S31.md:67`) [A]. A late reference is published
  after the trial deadline and the proof gets 409 `REFERENCE_TRIAL_CLOSED` (`REPORT-S31.md:206`) [A].
- Forge's rule of thumb "un essai tient seulement si l'upload < ~62 s" (journal-01 `:160`) is not a sharp
  threshold in the data: trials matched with uploads of 64.356, 67.602 and 75.976 s and lapsed with 59.144 and
  59.581 s (`REPORT-S31.md:336-348`) [A]; the slack also depends on the miner side and on when the challenge
  arrives [H].
- **101 M** (100,676,352 parameters): 566 s warm and 629 s cold (`T_ref` 347 s plus a miner side of 219 or 282
  s) against the same window (`livrables/forge/2026-10-29_v3-trial-rounds/REPORT.md:219-227`) [A].

**Grade.** PROVED: the literal window [V]. OBSERVED: S17c, 4 of 4 trials inside the window with 18-57 s of
slack; S31, one afternoon on which the same trials went from 2 lapses in 10 to 10 in 18 as the upload rose
from 34-38 s to 48-86 s, then to 1 in 29 on another bucket [A]. INFERRED: the window that holds depends on the
day's path as much as on the model size (D147, journal-01 `:156`) [H].

**Done, hash-neutral, not merged.** `v3-trial-rounds` `f4f2bc2c` makes `challenge_rounds` an integer 100..400,
still serialised at 100; derives the TEST window with `challenge_rounds_for(T) = max(100, ceil(1.15·T/3))`;
refuses a window above 100 on a production ledger (builder, run creation 422, `Admission`); and has the miner
refuse a challenge its run's policy did not issue (`…/v3-trial-rounds/REPORT.md:29-44`) [A].
`implementation_hash` is unchanged under all five pins [V, measured] and no golden root moves (`:214`) [A];
`sources()` and the bundle move, so images ship before any run uses a wider window (`:251-252`) [A]. The
branch is pushed and its CI is green on `f4f2bc2c` (`gh run list`), although its REPORT says "not pushed" [V].
The F14 entry says option (a) changes "hash d'implémentation et golden roots"
(`livrables/principal-sn100/journal-2026-10.md:220`): measured, it changes neither [V].

**Left for the train.** Nothing owned. The merge rebuilds the server and GPU images (it edits
`miner/core.py`), which the train's image cycle can carry [H].

**Cost.** New images. A run that signs a window above 100 has a new admission policy digest, hence a new run
id; at 217 rounds a dead application, a held trial place and a reseed last about 11 minutes instead of 5
(`…/v3-trial-rounds/REPORT.md:242-248`) [A].

**Recommendation** (for the founder to weigh). Merge `v3-trial-rounds` in the train's image cycle, outside the
hash change. Keep production at 100 until a TEST run has measured trials at a derived window. Size the TEST
window from the warm-up of the run's own day, after measuring the node → R2 rate that day (finding 8).

### Finding 2. Five audits on two lanes do not fit the 600 s audit window

- **Code** [V]. `schedule_audits_v2` (owned) reserves each job with 2 attempts, a first window `absolute =
  min(d_final, d_audit + policy.absolute_rounds())` and the auditors `manifest.training.auditors[:2]`
  (`challenge/store.py:8382`, `:8425-8428`). The manifest's `auditors` is a list of at least two distinct keys
  (`protocol/messages.py:374`, `store.py:2717`), and only its first two serve. `lease_v2` (owned) gives no
  lease while two leases run in the store (`SELECT COUNT(*) … state='running'` `>= 2`, `:8547-8553`). The
  signed `max_attempts` and `max_running_jobs` are `Literal[2]` (`messages_v2.py:758-760`), and
  `max_running_jobs` is read nowhere in `src`. The brief's "allowlist of 2 auditors (`Literal[2]`)" is these
  two literals plus `auditors[:2]`.
- **Gap** [V]. The widest `d_final - d_audit` is `DEFAULT_GAP_CAP_ROUNDS` = 200 unless the audit policy signs
  `gap_cap_rounds` (`messages_v2.py:741-743`). `validate_training` refuses a cap above 200 outside a TEST
  ledger, and above `2·absolute_rounds() + reaudit_window` (500 at a lease and a re-audit window of 100)
  (`:790-797`). The D20 builder signs it from `HT_D20_AUDIT_GAP_ROUNDS` (`233edb3`;
  `hypertrain/scripts/b300/d20/build_test_manifest.sh:156-176,494-505`).
- **S17d**, run `f3901a09`, gap 175 (525 s): rounds 0 and 1 retired `INFEASIBLE`, 0 MATCH, 5 ×
  `AUDIT_INFRASTRUCTURE`, the execute calls answered 503 inside master beacon pauses; 171-188 s per audit from
  execute request to `exec_published`; "5 audits on 2 auditors need 3 waves of about 180-200 s … = 540-600 s,
  against a window of 525 s (gap 175) or 600 s (cap 200)"
  (`livrables/forge/2026-10-28_s17-run-101m/s17d/REPORT-S17D.md:5,45-49,55`) [A, two rounds].
- **S19**, `f3901a09` round 2, gap 200 (600 s): one wave, lease to complete, takes 195-200 s; three waves
  590-620 s against 600 s; beacon-lag 503s on `/lease` idled the auditors 200 and 273 s; 2 MATCH and 3
  `AUDIT_INFRASTRUCTURE` (`livrables/forge/2026-10-29_s19-swap-round2/REPORT-S19.md:192-194,269-279`) [A, one
  round].
- **Gap 400** (runs `0ef600b4` and `4d84aacc`, audit policy `86ac1ac3…`): 5/5 MATCH in all four rounds, 809,
  815, 796 and 793 s after `d_audit` [H, from
  `livrables/forge/2026-10-30_s21-s22-round-settled/reports/v3-run-31m-s21-REPORT-S21.md:28,34`,
  `…/s23-round1-evidence/ctl.log:8,40` and `REPORT-S31.md:78-80,86-87`]. Each is past the first window's 600
  s: the fifth audit passed in the one rearm window (`store.py:9019-9033`, not owned), which exists only
  because `d_final` lies past that window (finding 3) [V by reading]. S20: gap 400 "holds at most two
  200-round (600 s) windows back to back, not one 1200 s window"
  (`livrables/forge/2026-10-29_s20-run-31m-gap400/REPORT-S20.md:94-95`) [A].

**Grade.** PROVED: two auditors and two lanes are fixed in owned code [V]. OBSERVED: no five-member round at
31 M passed at gap 175 or 200 (three rounds); all four at gap 400 passed through the rearm window [A].
INFERRED: at 101 M, where ADR 0015 puts five audits at 718-1279 s on one host, the gap is wider [H].

**Done, hash-neutral.** The TEST gap cap (D88) and its builder knob, used by `0ef600b4` and `4d84aacc` [A].
Nothing for a production ledger [V].

**Left for the train.** (a) More auditors or lanes: `schedule_audits_v2` (`auditors[:2]`) and `lease_v2` (`>=
2`) are owned. Reading the signed `max_running_jobs` there and widening the schema literals (not hashed) gives
a run that signs another value a new audit policy digest and run id [V by reading; design not written]. (b) An
audit window derived from the run (audits, measured audit time, lanes), as finding 1 does for trials, as an
optional signed field omitted when absent [H]. The service branch of `open_round_v2` (owned) also hard-codes a
gap of 200 (`store.py:5724`, `:6017`) [V].

**Cost.** Both owned edits move the hash. A run that signs a new value gets a new run id; pinned runs and the
example manifest keep their bytes while the fields are omitted when absent [H].

**Recommendation.** Carry in the train the reading of `max_running_jobs` and of an auditor count from the
signed policy, with schema bounds, and one design for a derived audit window and the first window of finding
3. Keep production at 200 and two lanes until a TEST run measures the new geometry.

### Finding 3. The first audit window ends at `d_audit + 200`, whatever the gap

- **Code** [V]. `absolute = min(d_final, d_audit + 2·lease_rounds)` (`store.py:8382`; `absolute_rounds()`,
  `messages_v2.py:805-806`), with `lease_rounds` 100 at H ≤ 30 (`:733-738`); `store.py` never reads
  `gap_cap_rounds`. A lease ends at `min(now + lease_rounds, absolute)` (`lease_v2`, `store.py:8598`). An
  exhausted job may be rearmed once, at `min(now + absolute_rounds(), the commit's exp_drand)` with its
  attempts reset (`_rearm_audit_job_v2`, `:9019-9033`, not owned).
- **The 500** [V by reading]. When the lease ends during an execution, the server-side `LeaseGuard.check`
  raises `AuditInputError("audit lease expired or revoked")` (`auditor/worker.py:84-86`) inside
  `execute_audit_v2` (owned); the attempt is booked and the job requeued or exhausted (`store.py:9741-9792`,
  not owned). `challenge/app.py` registers no handler for `AuditInputError` (`:512-541`), so the route answers
  500; a probe app with the same handlers answered 500. The auditor CLI raises on it and exits 2, and
  `hypertrain/scripts/ops/auditors.sh` polls again 20 s later (`auditor/__main__.py:79-87,195-199`;
  `auditors.sh:47-50`).
- **`0ef600b4` round 0** (S21, 2026-10-30, gap 400): member m2 (`5C8UHbfA`), attempt 1 leased at 05:33:55Z,
  lease window `d_audit + 200` = 05:36:36Z; `execute` HTTP 500 at 05:36:49Z (traceback `store.py:9453
  execute_audit_v2` → `worker.py:86 AuditInputError`, unmapped), `AUDIT_INFRASTRUCTURE`, requeued; attempt 2
  MATCH at 05:40:04Z; 5/5 (S21 rows 4g and 4h, `…/reports/v3-run-31m-s21-REPORT-S21.md:33-34`;
  `…/s21-evidence/challenge-execute-500.raw:51-58`) [A]. The executor published 1.3 s after that deadline
  (`…/executor-journals/executor-state-s20-journal.jsonl:351-356`) [H, journal timestamps]. Line 9453 is that
  of the image that ran; `store.py` at `dc014508` raises nothing named `AuditInputError` [V]. Row 4g labels
  three other members differently from the S20 roster; m2 and m4 agree [A].
- **The other gap-400 rounds**: `0ef600b4` round 1, m3 (`5G9qNDdT`), execute 500 at 09:22:50Z; `4d84aacc`
  round 0, m4 (`5H6y5P4T`), execute 500 at 19:29:42Z; round 1, m0 (`5CvXtit3`), `AUDIT_INFRASTRUCTURE` at
  20:43:20Z with no 500 line in the watch (`…/s23-round1-evidence/ctl.log:31-38`; `REPORT-S31.md:80,87`;
  `S31/evidence/ctl-round0.log:43-49`, `ctl-round1.log:44-51`) [A, four rounds].
- **Timing** [H]. The fifth lease began 439, 460, 448 and 447 s after `d_audit`, leaving 161, 140, 152 and 153
  s to `d_audit + 200`, while lease to MATCH took 180-229 s in the same watches. The artifact URLs of every
  failing first attempt expired at `d_audit + 200` plus the executor's 30 s margin
  (`gpu_ops/artifact_transfer.py:90,540`; the executor journals of S21 and S31).

**Grade.** PROVED: the bound and the unmapped 500 [V]. OBSERVED in 4 of 4 gap-400 rounds: one first attempt
per round ended at `d_audit + 200` [A]. INFERRED: the third wave cannot finish in the first window at 31 M
with two lanes, and the "AUDIT_INFRASTRUCTURE at each round" of finding 8 is this bound [H].

**Done, hash-neutral.** Nothing on the bound; gap 400, TEST only, gives the rearm window the fifth audit uses
[A].

**Left for the train.** The bound: `schedule_audits_v2` is owned, and a first window read from a new optional
signed field (derived from the gap or from the audit count) needs the train [V for the hash; H for the
design]. Changing `absolute_rounds()` in `messages_v2.py` (not hashed) instead would change the meaning of
every signed audit policy under an unchanged hash, the objection D128 makes to rebinding [H]. Not the 500: a
handler in `app.py` is outside the hash [V]. F14 lists "`AuditInputError` → 500" in the train
(`livrables/principal-sn100/journal-2026-10.md:220`); by the code it does not need it.

**Cost.** As finding 2. The handler alone needs a server image only.

**Recommendation.** Put the first-window rule in the train with finding 2's audit window (one design, one
signed field family). Ship the `AuditInputError` handler (a 4xx naming the expired lease) hash-neutral before
the train, with a test of the status. Leave `absolute_rounds()` as it is.

### Finding 4. The finalize gate is judged after the finalize's own replay

- **Code at `dc014508`** [V]. `finalize_v2` (owned) replays the applied tape before its transaction
  (`_prime_finality_replay_v2`, `store.py:10877`) and judges the freshness gate inside the transaction, after
  that replay (`:10932`). The aggregate's gate is already at entry (`_aggregate_claim_v2`, `:10711`, before
  `_aggregate_compute_v2`).
- **Variant A** (hotfix 5, `ed5a6481`; on TEST since about 30/10 11:43Z, image `7b7613cc`, journal-31 `:471`;
  now in `3bb5db0f`). `_result_in_lease_v2` (not owned) tolerates the aggregate and the finalize of a round
  whose tape is stored and whose audits are all resolved, up to `RESULT_PAUSE_ROUNDS` = 50 more rounds behind
  (`MAX_LAG_ROUNDS` = 2; `challenge/exit_v2.py:24-31`) [V]; `implementation_hash` unchanged [V]. Residual,
  pinned by a strict xfail (`hypertrain/tests/e2e/test_finalize_freshness_v2_e2e.py:167`): a finalize whose
  own replay keeps the pusher out longer than the cap, over 52 rounds or about 150 s, still answers 503; a
  retry is safe, writes nothing and reuses the kept verdict within 120 s
  (`livrables/forge/2026-10-30_hotfix-finalize-freshness/REPORT.md:32,99`) [A]. That REPORT is titled "HOTFIX
  3"; the journal, Lens and the images call it hotfix 5 [A].
- **Variant B** (`c9e612b3`, branch `hotfix-finalize-entry-gate`, pushed, no CI run, parent `ed5a6481`, 30
  commits behind `dc014508`) [V]. `finalize_v2` judges `_fresh_v2` and the Finalize's expiry once, in a short
  transaction before the replay, when no finalize record exists; the gate after the replay becomes `assert now
  is not None` (`c9e612b3:…/store.py:10748-10763,10800`) [V]. Only `finalize_v2`'s AST changes;
  `implementation_hash` becomes `8235e5f5…84b5a8` (`avx2`/`sm_103`/`math`) and `2f63fb82…8c3220`
  (`avx2`/`none`) [V, measured]. Targeted suites 897 passed, 1 xfailed (the golden hash); mutants 5/5; broad
  suites not re-run (`…/hotfix-finalize-freshness/REPORT.md:68`) [A].
- **Lens on `c9e612b3`** (`livrables/lens/redteam-live/delta-verdicts.md:607`, 2026-10-30T11:24Z): "LEVÉ SOUS
  RÉSERVE", 0 B / 0 I / 3 m. m1: the round settles at the entry round, so `finalized_beacon` and the vesting
  are earlier than the real close by the replay's duration, not bounded by `RESULT_PAUSE_ROUNDS`; m2: the
  expiry and `_fresh_v2` run before the Finalize is authenticated (409/503 before 401); m3: the e2e and 3
  mutants not replayed [A]. Principal's terms (journal-31 `:469`): date the vesting at the round the replay
  ends, or bound the gap; expiry and gate after the authentication, never before; replay the e2e and the 3
  mutants in a Relay session [A]. The code shows both: `beacon=now` and `vesting = now + E_vest_rounds` with
  `now` judged at entry (`c9e612b3:…/store.py:10826,10862`); the gate at `:10760-10761` before
  `_authenticated_v2` at `:10790` [V].
- D120, step 2: before production, the gate at entry "même si implementation_hash change; un replay > 150 s en
  503 à chaque retry est inacceptable à 101 M+" (journal-31 `:455`) [A]. D120, Lens and the journal say
  "finalize_v2/aggregate_v2"; B edits `finalize_v2` only, and the aggregate gate is at entry already [V].
- **Measures.** S31 round 0: aggregate and finalize 19:33:22-19:38:16Z, 294 s (aggregate 200 at 19:36:50Z),
  one attempt; round 1: 20:49:52-20:54:33Z, 281 s, one attempt (`REPORT-S31.md:81,88`) [A, two rounds; the
  Principal's "4 min 55 s" for round 0 is the same call, `livrables/principal-sn100/journal-2026-10.md:241`].
  `0ef600b4` round 0 before hotfix 5: three attempts, two 503 (`REPORT-S31.md:284`;
  `S31/evidence/lag-baseline-0ef600b4.md:75-81`); round 1 after it: one attempt of 90 s
  (`…/s26-round1-final/ctl3.log:12-13`) [A]. At 101 M the finalize took 160-237 s outside the lock in
  store-level runs on synthetic real-size deltas, on a loaded host
  (`livrables/forge/2026-10-28_aggregate-lock-release/REPORT.md:40-57`) [A], above the 150 s that variant A
  tolerates [H].

**Grade.** PROVED: the gate's place, A's residual (strict xfail) and B's hash [V]. OBSERVED: two 503s before
hotfix 5 on `0ef600b4` round 0; a single attempt in each of the three finalizes since its swap [A]. INFERRED:
at 101 M a replay of 160-237 s outlasts A's cap, so the finalize would answer 503 on every retry [H].

**Done, hash-neutral.** Variant A, deployed [A].

**Left for the train.** B, rebased onto `v3`; between `ed5a6481` and `dc014508` no owned AST or hashed file
differs, so a clean rebase should keep `8235e5f5…` for B alone [H]. Then m1 and m2 in `finalize_v2` (owned),
and m3 replayed.

**Cost.** One owned name; the 0ef600b4 golden constant; A's strict xfail and B's e2e and mutants.

**Recommendation.** Include B with m1 and m2 closed (the vesting dated at the end of the replay; authentication
first) and m3 replayed. Bounding the gap and refusing beyond it is not recommended: at 101 M a replay of
160-237 s would exceed the bound and replay the 503 on every retry (Lens, 2026-11-01). D120 already asks for it
before production.

### Finding 5. `_service_charge_v2` reads its guard without the store lock

- **Code** [V]. `_service_charge_v2` (owned) tests `self._db.in_transaction` and raises 409
  `SERVICE_CHARGE_REQUIRES_COMMITTED_TRANSACTION` before it takes the store lock with `with self._tx():`
  (`store.py:5278-5280`; `_tx` takes the lock at `:627`). The store shares one connection across threads
  (`check_same_thread=False`, `:552`), so a transaction any other thread holds refuses the charge.
- **Path** [V]. `hypertrain/scripts/network_service_proof.py:2128` (`decoder_continue`) calls
  `_prepare_capacity_genesis_v2` (owned), which commits its `genesis-resource` record first, so a retry
  answers 409 `SERVICE_GENESIS_ATTEMPT_RECORDED` (`store.py:5458-5461`). `decoder_continue` refuses a `test`
  ledger: it needs a production-mode store, which the same process serves over HTTPS while the beacon pusher
  posts every 3 s; once `network_graph_started` is journaled, the orchestrator refuses a rerun, so one refusal
  ends the continuation (`livrables/forge/2026-10-31_v3-weights-hardening/REPORT.md:228-240`) [A].
- **Proof** [A]. Two strict xfails in `hypertrain/tests/challenge/test_service_charge_window_v2.py`
  (`b3af43f`): the weights-answer window and the D128 case of a genesis charge during a beacon push both fail
  with the 409 under `--runxfail`; with the guard and the `_tx` block under `with self._lock:` on a scratch
  copy the file gives 3 passed (`REPORT.md:242-263`). Caveat: in a child store the held lock makes the inner
  `_tx` non-outermost, so staged beacon rounds must be published before the deadline check (`:264-266`).
- Never observed in a run: the genesis path refuses the TEST ledger
  (`livrables/forge/runbooks/genesis-decoder-continue.md:39-41`) [A].

**Grade.** PROVED: the race, by the two strict xfails and the code [V]. OBSERVED: not in a run. INFERRED: on a
production-mode store the first genesis attempt is lost whenever a push transaction is open at that instant
[H].

**Done, hash-neutral.** No code (D128). Operator mitigation: pause the TEST beacon pusher during the genesis
call (`…/runbooks/genesis-decoder-continue.md`), rehearsed dry on 2026-10-31, 07:11:51-07:12:17Z: 26 s of
pause, lag 2 → 10, back to 2 four seconds after the unpause, `restarts=0`, no genesis and no open round, on
`b3af43f` with run `0ef600b4` (`:3,30-35`) [A, once]. It does not cover other requests on that store
(`…/v3-weights-hardening/REPORT.md:293`) [A].

**Left for the train.** Wrap the guard and the `with self._tx():` block in `with self._lock:` (re-entrant) and
publish staged rounds first in a child store (`REPORT.md:258-266`) [A]; one owned name.

**Cost.** The hash; the two strict xfails flip to plain tests.

**Recommendation.** Include it: the change is small and its red tests exist. Keep the runbook until the swap.

### Finding 6. The PROBATION cohort shares one Q/4 cap: no round before three ACTIVE members

- **Code** [V]. At open, `open_round_v2` (owned) runs `allocate_weights` with the run's aggregation policy
  (`store.py:6084-6090`). `weighted_v2.py` (not hashed) gives the whole PROBATION cohort one cap and each
  miner one cap (`_completion_capacity`, `aggregator/weighted_v2.py:44-54`); a roster that cannot carry Q
  raises `INSUFFICIENT_ELIGIBLE_WEIGHT` (`:95-104`), mapped to 409 `ROUND_INFEASIBLE`
  (`challenge/app.py:524-535`). Both caps are signed policy values pinned by the schema: `miner_cap_units` and
  `probation_cap_units` `Literal[4194304]`, Q = `weight_quantum` `Literal[16777216]`
  (`messages_v2.py:704-706`). ADR 0011 states the cohort cap (`0011-audit-integrity.md:945-947`). A tape needs
  at least four inputs (`aggregator/tape_v2.py:89`).
- **Eligibility** [V]. ACTIVE, or PROBATION with `clean_count >= 4` (a code literal, `admission.py:1274`);
  PROBATION → ACTIVE after `clean_finalizations` = 12 (`Literal[12]`, `messages_v2.py:656`) with canary blocks
  `(1, 2, 3)` (`admission.py:1293-1297`).
- **Check** [A]. `allocate_weights` called with the run's policy, five members
  (`S31/evidence/d2-admission-policy.md` §3): 0, 1 or 2 ACTIVE fail; 3 ACTIVE + 2 PROBATION pass (ACTIVE
  4,194,304 each, PROBATION 2,097,152 each); 4 + 1 and 5 + 0 pass. So D146's Plan B, five PROBATION members at
  ≥ 4 clean trials, could not open (`REPORT-S31.md:211`). 3 ACTIVE + 1 PROBATION fills Q arithmetically, not
  exercised (`:211`) [H]; D148 refused that 4-member roster as AC-CLIFF, one exclusion breaking the round
  (journal-01 `:159`) [A].
- **Cost in trials** [A]. A new run opens no round before 3 × 12 = 36 clean trials for its ACTIVE members; the
  5-member roster of S31 took 44 clean trials, 57 launches and 13 lapses in 5 h 18 min (`REPORT-S31.md:93`).
  The "5 h 20 for 52 trials" is S20's admission of `0ef600b4` (4 ACTIVE + 1 PROBATION): 52 clean, 54 launches,
  22:48:19Z → 04:08:03Z, 5.33 h (`…/2026-10-29_s20-run-31m-gap400/REPORT-S20.md:60`). Paces: S20 9.8-11 clean
  trials an hour; S31 about 11.5, then 6.5 and about 4.3 on the degraded path, then 10.3-10.4 after the bucket
  switch (`REPORT-S20.md:49-60`; `REPORT-S31.md:60,66,72,99`; journal-01 `:160`). D148: "≈ 36 essais ≈ 3-5 h"
  (journal-01 `:159`). 36 trials take 3.3-4.5 h at 11-8 an hour and about 8.4 h at 4.3 [H]. The brief's "~19 h
  elapsed at the degraded R2 path" is in no file: not measured.
- In TEST all identities trial in one lane on one GPU (`REPORT-S31.md:59,93`); with independent miners
  trialling side by side the wall time would be about that of one miner's twelve trials [H, not measured].

**Grade.** PROVED: the cap and the open-time check [V]; the 0/1/2 versus 3 ACTIVE outcomes by a direct call
[A]. OBSERVED: S31 waited for 36 + 8 clean trials before round 0 [A]. INFERRED: a network's first round waits
for three miners to reach 12 clean trials each [H].

**Done, hash-neutral.** Nothing; S31 used 3 ACTIVE + 2 PROBATION (D148).

**Options and what each needs.** (a) Another constant than Q/4: a schema literal (not hashed) and a signed
aggregation-policy value, so a new policy digest and new run ids for the runs that use it, no hash move [V by
reading]; it changes what PROBATION miners can earn, a product decision [H]. (b) Another graduation threshold
(12, the canary blocks, the literal 4): `messages_v2.py` and `admission.py`, not hashed, a new admission
policy digest [V by reading]; it changes how much of the canary set a miner has proved [H]. (c) A bootstrap
path, such as a first round with a smaller cohort: not designed; it would touch `open_round_v2` or
`_require_survivors_v2` (owned) or `weighted_v2.py` [?]. If (a) or (b) changes a builder default, the example
run ids pinned in tests move too.

**Recommendation.** Change neither the cap nor the threshold in this train. Plan 36-44 clean trials before a
TEST run's round 0, never a 4-member roster. Decide a bootstrap path (c) only with its own design, if a
network launch must open rounds sooner.

### Finding 7. The beacon push while a replay runs: observed once in two rounds

- **Done, hash-neutral** [A]. `922c61c` (`v3` since 30/10, TEST since 30/10 22:13Z): the root commits a round
  first and forwards it with `wait=False`; a busy child stages the verified round and returns; `_tx()` writes
  staged rounds at its outermost boundaries; the pause witness keeps the view callers were held at
  (`livrables/forge/2026-10-30_beacon-push-outside-lock/REPORT.md:71-86`); hash unchanged (`:186-194`).
- **The criterion was mis-specified** [A]. The lot's deployment note says: "During the transaction itself the
  child file is write-locked, so `lagloop.py` still shows the lag climbing until the release"
  (`REPORT.md:226`). Its publication note still named "lag <= 2 during open/aggregate/finalize" as the gain to
  validate (`:269`), and D146 took it as the success criterion
  (`livrables/principal-sn100/journal-2026-10.md:241`). A child file write-locked by a transaction cannot
  receive a round before the release, so that reading could not be met; Forge wrote it was "MAL POSE par moi"
  (journal-01 `:166`) and D149 kept the reading "0 échec pusher sur l'open, retour <= 2 au relâchement dans
  les 3 phases, finalize 1 tentative, témoin exact" (`:167`).
- **S31, lane stopped, 1 Hz** [A, two rounds]. Open: lag up to 19 (round 0) and 14 (round 1), rising by 1
  every 3 s during the transaction and back to 1 at the release (18:59:38Z, 20:12:44Z), with no catch-up tail.
  Aggregate 13 and 8, finalize 12 and 7. Lag at the release of open, aggregate and finalize: 1, 2, 1 in round
  0; 1, 3 (2 five seconds later), 2 in round 1 (`REPORT-S31.md:78,86,233-238,257-265,281`). Baseline
  `0ef600b4` before the fix: open 15, aggregate 11, finalize 9, the last two from an attempt whose finalize
  answered 503; the attempt that settled peaked at 6 (`S31/evidence/lag-baseline-0ef600b4.md:71-81`). Pusher
  failures during the open: 0 in both rounds, against 3 ReadTimeouts in S21's open (journal-01 `:166`).
- **ReadTimeouts** [A]. Round 0: six, at 19:35:04, 19:35:16, 19:36:30 and 19:36:44Z (aggregate) and 19:37:52
  and 19:38:07Z (finalize), each pair `failure 1/100`, `failure 2/100`, then recovered
  (`S31/evidence/pusher-errors-1855-1940.txt`; `S31/pusher-log/beacon-18:50-19:42.log:1012-1034`). Round 1:
  none (`S31/evidence/pusher-errors-2006-2056.txt` is empty). Forge counts two of the six in the lock-free
  replay, hence CPU or GIL, not determined (journal-01 `:166`); which two is not stated. At each of the six
  seconds the 1 Hz series shows the lag climbing by 1 every 3 s (7, 11, 7, 11, 5, 10), the pattern of a
  write-locked transaction (`S31/evidence/lag-round0.txt`) [H].
- **The lot's open ends** [A]. A run hosted by the root store still waits for the root's lock (the beacon in
  its own file is a storage change); if the finalize's slow pushes are the replay's pure-Python part, they
  need a process pool, and `_run_finality_replay_v2` is owned (`REPORT.md:213-214`). One pure-Python CPU
  thread slowed an idle push to 526-817 ms, four to 3,450-3,659 ms (`:57-66`).

**Grade.** PROVED: the push no longer waits for a child lock (13/13 mutants of the lot, `REPORT.md:132`) [A].
OBSERVED: no pusher failure in two opens; six ReadTimeouts in one aggregate and finalize out of two; cause not
determined [A]. INFERRED: at 101 M, with a 160-237 s finalize replay (finding 4) and a longer aggregate, the
exposure grows [H].

**Options for 101 M+.** (a) The beacon push in a process apart from the admin routes: `push_beacon` and
`_store_beacon_v2` are not owned, so no hash move [V], but a process and deployment change; it cannot write a
child file inside a write-locked transaction either (one writer per SQLite file, `REPORT.md:88-90`) [A]. (b)
The replay and the aggregate's compute out of process: `_run_finality_replay_v2` and `_aggregate_compute_v2`
are owned, so the train [V].

**Recommendation.** Before choosing, measure push latency and GIL hold during one aggregate and finalize on
the next TEST run, hash-neutral. If the timeouts recur, take (b) in the train: it also takes the replay's CPU
off the request threads. (a) can follow without the train. Write the D149 reading as the criterion.

### Finding 8. The node → R2 path, and AUDIT_INFRASTRUCTURE in every round

- **Location hint** [A, one measure]. One stream of 256 MiB, same minute (15:42-15:46Z): node A 9.4 MB/s on
  the unhinted bucket against 22.7 MB/s on an APAC-hinted one (2.4×), node B 2.8 against 21.8 MB/s (7.8×)
  (`REPORT-S31.md:116-121`). After the switch at 15:58:00Z, 37 uploads took 14.902-31.192 s (median 27.101 s),
  against 25 uploads of 34.191-85.996 s (median 54.806 s) before (`:203-204`). Bucket and token were healthy,
  the cause is the Hyderabad → R2 path, and Cloudflare reported no incident (`:113`, `:206`).
- The reference upload is already parallel (`storage/cas.py:353`, `REPORT-S31.md:114`) [A]. The brief's "60 s
  upload limit" is in no file. The 60 s in code is the executor's R2 reserve: R2 is skipped when a lease has
  less than `reserve_seconds` (60) plus `MIN_ISLAND_SECONDS` (30) left
  (`gpu_ops/artifact_transfer.py:79,89,524`) [V], logged three times in S31 as "skipped: lease too short for
  the reserve" (`REPORT-S31.md:131,147,149`) [A]. The trial-level rule is Forge's "~62 s" of finding 1.
- **AUDIT_INFRASTRUCTURE** on the first attempt of one member each round: "not investigated"
  (`REPORT-S31.md:458`) [A]. Its timing matches finding 3's bound in all four gap-400 rounds [H].
- The S31 nodes went back on 01/11 06:37Z, the R2 buckets were deleted and their tokens revoked
  (`agents/forge.md:46`) [A]; nothing more can be measured before F14 (D156).

**Grade.** OBSERVED once: the hinted bucket 2.4-7.8 times faster on one stream, and the uploads halved after
the switch [A]. INFERRED: the per-round AUDIT_INFRASTRUCTURE is finding 3 [H].

**Not a hash item.** **Recommendation.** Make the bucket's location hint and a measured node → R2 rate at a
real size a pre-flight step of every run (the 1 MiB `r2-check` "says nothing about the rate", ADR 0015
`0015-intermediate-dense-test-model.md:398-400`). Treat the per-round AUDIT_INFRASTRUCTURE as finding 3,
unless it persists once the first window is widened.

## Decision proposed to the founder: one train

The founder decides (F14). Proposed: one change of `implementation_hash` that carries only owned-body edits,
built on one integration branch and measured once. Everything that can ship without it ships in the same image
cycle, or before it.

### Content

| Item | Finding | Owned names edited | State | Size |
| --- | --- | --- | --- | --- |
| T1 | 4: the finalize gate at entry; m1 closed by dating the vesting at the end of the replay (not by bounding the gap and refusing beyond it: D157, D120); m2 closed (authentication first) | `finalize_v2` | written (`c9e612b3`); m1, m2 open; m3 to replay | to estimate |
| T2 | 5: the charge guard read under the store lock | `_service_charge_v2` | fix and red tests exist (`b3af43f`; a scratch copy gives 3 passed) | to estimate |
| T3 | 2 and 3: lanes, auditors and the first audit window read from the signed audit policy | `schedule_audits_v2`, `lease_v2`; `open_round_v2` if service runs need it | not designed | to estimate |
| T4, outside the train until designed (D157) | 7 (b): the replay and the aggregate's compute out of process | `_run_finality_replay_v2`, `_aggregate_compute_v2` and callers | not designed; measure first | to estimate |
| T5, outside the train until designed (D157) | 6 (c): a bootstrap path | `open_round_v2`, `_require_survivors_v2`, or none [?] | not designed | to estimate |

T1 and T2 edit `challenge/store.py` only and change no manifest default [V by reading `c9e612b3` and the
weights-hardening fix]. T3 and T5 can add signed fields omitted when absent, which changes no default either;
a default changes only if the founder chooses one (finding 6 (a) or (b), or a default window) [H].

### Cost of the single hash change

**What moves** [V].
- `implementation_hash` under every pin. `24fbe36d…c185ce` (the pin of the s100 runs) and `7af135ca…70be62`
  (the example pin) become values known only when T1-T5 are frozen. T1 alone gives `8235e5f5…84b5a8` and
  `2f63fb82…8c3220`, measured on `c9e612b3`.
- `challenge/store.py`, `sources()` and the bundle, as after any `src` edit: older qualification configs stop
  matching the new tree and validate only against their own (ADR 0007 `0007-sm-counts-per-arch.md:62-66`) [A].

**What is regenerated, once.**
- `IMPLEMENTATION_HASH` of `hypertrain/tests/ops/s100/test_run_0ef600b4_golden.py:23`, the only test that pins
  the hash [V]. `test_run_f3901a09_golden.py` pins that run's id, audit policy `1dd433fe…` and admission
  policy `7a881925…` (`:18-20`); none moves [V].
- The strict xfails the train closes: the two of `hypertrain/tests/challenge/test_service_charge_window_v2.py`
  (T2) and A's `test_a_finalize_whose_own_replay_outlasts_the_cap_settles`
  (`hypertrain/tests/e2e/test_finalize_freshness_v2_e2e.py:167`), which B replaces with an entry-round test [V
  on `c9e612b3`]. The mutant tables whose snippets follow the edited code; a moved snippet fails
  `test_infra_guards.py` until re-anchored, as the beacon lot found
  (`livrables/forge/2026-10-30_beacon-push-outside-lock/REPORT.md:27-29`) [A].
- This record's hash rows, measured on the integration branch [H].
- Only if a builder default changes: the example run id `aa39197d…`
  (`hypertrain/tests/protocol/test_model_arch.py:28`, `hypertrain/tests/trainer/test_config_v2.py:13`,
  `hypertrain/tests/protocol/test_od_fields.py:14`) and the example admission digest and run id
  (`hypertrain/tests/protocol/test_network_v2.py:138-139`) [V].

**What stays frozen.**
- The run ids, manifests, policies and golden checks of `f3901a09`, `0ef600b4` and `4d84aacc`: none holds
  `implementation_hash` [V]. Their manifests pin the GPU image `cb52fe37…`
  (`hypertrain/tests/ops/s100/fixtures/0ef600b4/manifest.json`), built from `667695be` (journal-10 `:1131`)
  [A].
- Their identity, not their code. They hold no `service-admission` record, so the new image does not refuse
  them (`store.py:5557-5562`) [H by reading; A for the 0 rows]. But one challenge image serves every hosted
  store (`f3901a09`, `0ef600b4`, `4d84aacc`, `07cb8c0d`, `535723b1`), so a later round of any of them runs the
  new owned bodies [H]. Their miners and executors keep `cb52fe37`'s code: an edit of one of the 12 hashed
  files the GPU side runs (the trainer, `auditor/replay.py`, `miner/island_launch.py`, `protocol/env_pins.py`)
  reaches them only through a new GPU image, a new manifest and a new run id (ADR 0007 `:72-74`, ADR 0015
  `0015-intermediate-dense-test-model.md:516-518`) [A]. T1-T5 as listed edit `store.py` only [H].
- A service-admitted run must be recreated (`SERVICE_PROFILE_IMMUTABLE`, see Context) [V]; TEST has none [A].

**Images and swap.**
- A push to `v3` that touches `hypertrain/**` runs `hypertrain-images.yml`: server and GPU images, tag
  `v3-sha-<sha>`, the commit in a label (so every rebuild has a new digest), provenance attested and verified
  (`.github/workflows/hypertrain-images.yml:9-16,42-49,105-149`) [V]. Build time: not measured in this lot.
- The TEST swap (`docs/s100-test-stack.md:38-50`) is gated on a sealed, recent seal of the previous epoch, and
  runs a backup, a rollback dry run (`paper`), the challenge then the beacon, and checks (`/version`, the
  image id, the weights replays of epochs 23272 and 23277, the hosted stores before and after) [A]. The last
  two swaps took 26 s and 23 s from the gate to the checks: 30/10 22:13:30-22:13:56Z (journal-31 `:497`;
  `livrables/forge/2026-10-30_beacon-push-outside-lock/swap-evidence/swap.txt`) and 31/10 12:53:09-12:53:32Z
  (`/var/lib/openbot/scratch-forge/s33/evidence/ctl4.log:7-13`, not copied to the share; two invocations ran
  at once, one stopped with rc 5 and the other swapped) [A].
- T1-T5 need the server image only [H]. `v3-trial-rounds` needs the GPU image too (finding 1), so a run that
  uses a wider window is a new run [H].

**Rollback** [A]. `rollback B` restores the backed-up compose and recreates the challenge and the beacon, and
fails unless the old image id runs again; `restore-db B` restores `challenge.db` and `server_keys.db`, not the
ledger journal, the child stores or `trials-v2/`; neither is gated (`docs/s100-test-stack.md:73,229-238`). The
image to return to is `sha256:3bb5db0f…` (code `f357e61`, on TEST since 31/10 12:53Z; `agents/forge.md:45`).
T1 and T2 add no table and no record kind [H]; a round finalized under T1 keeps its recorded vesting after a
rollback [H].

### What does not need the train

- Annex A, already on `v3` and on TEST.
- `v3-trial-rounds` (finding 1) and `v3-miner-fast-path` (hash-neutral, measured; a small gain at 31 M on a
  real node, journal-31 `:182`): new images only.
- The `AuditInputError` handler (finding 3); options (a) and (b) of finding 6; a push process apart (finding 7
  (a)) and its measurement; the node → R2 pre-flight (finding 8); the genesis runbook until the swap (finding
  5).

### Order and duration

Every duration is to estimate unless a file gives it.

1. F14: scope in principle T1 + T2 + T3 in one hash train, the hash measured once when the content is
   frozen; T4 and T5 stay outside the train until they are designed (D157, Principal SN100). T3 is designed
   first, in an ADR without code, read by Lens and validated by Principal SN100 before any code; a design that
   needs a new protocol constant or another policy field goes back to the founder. The founder also decides
   whether it ships before the next TEST run.
2. One lot per item, red test first, Lens on each: T1 (rebase `c9e612b3`, close m1 and m2, replay m3; Lens
   could not replay B's e2e under load, `delta-verdicts.md:607`), T2, T3 (design first). To estimate.
3. One integration branch: the hash measured once, the golden constant updated once, the strict xfails flipped
   once; Lens on the whole; fast-forward of `v3` on Principal SN100's word. To estimate.
4. CI images and attestation (build time not measured here); the gated TEST swap (23-26 s for the last two).
5. A new 31 M TEST run on the new image: an admission of 5 h 18 min (44 clean trials, S31) to 5 h 20 min (52,
   S20), and a round in about 40 min (S31 round 0, open 18:57:54Z to settled 19:38:16Z, `REPORT-S31.md:78,81`)
   [A]. It needs a node loan, which D156 holds until F14 [A].
6. 101 M: not measured; it needs the derived windows of findings 1-3 and the measure of finding 7.

## Rejected

- **One hash change per finding**: each regenerates the same pins and invalidates service-admitted runs again;
  D120 asks for one.
- **Rebinding or wrapping an owned method** to keep the hash (D128).
- **Editing `AuditPolicyV2.absolute_rounds()` in place** (finding 3): signed policies would change meaning
  under an unchanged hash.
- **Merging `c9e612b3` as it stands**: m1-m3 open, its parent 30 commits behind `v3`, no CI run.
- **A production gap cap above 200 alone**: the first window stays `d_audit + 200` (finding 3).
- **A 4-member roster to open sooner**: AC-CLIFF (D148).

## Not verified

- **31 M only.** No 101 M round has run; every 101 M figure here is a store-level synthetic run, a scaling or
  one sample.
- **TEST only.** No production ledger; the genesis path of finding 5 never ran.
- **Finding 7 observed once in two rounds**, cause not determined; which two ReadTimeouts were lock-free is
  not stated.
- The per-round AUDIT_INFRASTRUCTURE as finding 3: timing only; no run widened the first window.
- The train's own `implementation_hash`: unknown until its content is frozen; the rebase of `c9e612b3` was not
  tried.
- The hash values come from CPython 3.12.15; the image's interpreter was not checked, and `ast.dump` output
  depends on it.
- The three unmerged lots together (two drafts numbered 0017, two edits of `miner/core.py`): no merge tried.
- Nothing in this lot ran on s100, a node, a GPU or R2.

## Annex A. Already delivered without the train

Every commit below is an ancestor of `dc014508` [V, `git merge-base --is-ancestor`], and the whole set is on
TEST since 31/10 12:53Z (server `sha256:3bb5db0f…`, code `f357e61`; `agents/forge.md:45`) [A].
`implementation_hash` is `24fbe36d…c185ce` under the pins of both s100 runs on twelve trees, `708414d8`,
`2b3f90bf`, `e5900c3b`, `2774630f`, `ed5a6481`, `922c61c`, `8893863`, `b3af43f`, `a3fc635`, `f357e61`,
`8e62065` and `dc014508` [V, measured 2026-11-01]. The hotfix numbers are those of
`livrables/forge/2026-10-30_preuve-hors-chaine.md:17`.

| Delivery | Commits | What it changed | Hash evidence of its own report |
| --- | --- | --- | --- |
| hotfix 1 (`v3-audit-freshness-pause`) | `02308845`, `9e610c04`, `2b3f90bf` | an audit result and its completion pass the freshness gate up to `RESULT_PAUSE_ROUNDS` (50) more rounds behind while their lease runs; the auditor CLI re-posts a 503 every 3 s until the lease deadline | REPORT row, read and not recomputed according to Lens (journal-31 `:196`); measured unchanged here |
| hotfix 2a (`hotfix-lease-pause`) | `e71da1f`, `0efeaa0`, `eb6e8b4`, `870c2fa`, `e5900c3b` | `_fresh_v2` judges a tolerated view at `max(now, emitted - 2)`; the lease and the audit execute are tolerated within the same cap, on a running run without service admission | 48 owned ASTs identical (`livrables/forge/2026-10-30_hotfix-lease-pause/REPORT.md:193-197`); `24fbe36d…` live after the swap (`…/REPORT-S21.md:24`) |
| beacon ops (`v3-ops-beacon-image`) | `e603224`, `adf745a`, `1198de3`, merged at `2774630` | `challenge_image.sh beacon-image`: the swap also moves the TEST beacon, whose old image refilled the oldest round first | no `src` change (`hotfix-lease-pause/REPORT.md:42-43`) |
| hotfix 4 (`hotfix-weights-retired-child`) | `b1f6e06`, `6489127`, `b2c05bd`, `dae1510`, `31a0002`, `a6981ce`, merged at `2774630` | `get_weights` skips a retired hosted run (`weights-skipped`) instead of a 409, and commits an epoch answer in every hosted store or in none; `31a00026` adds the 0ef600b4 golden | `24fbe36d…` unchanged (`…/reports/hotfix-weights-retired-child-2-REPORT.md:163`) |
| hotfix 5, variant A (`hotfix-finalize-freshness`) | `54bbc9c`, `e34d513`, `ed5a648` | finding 4, variant A | `24fbe36d…`, owned set untouched (`…/2026-10-30_hotfix-finalize-freshness/REPORT.md:73-76`) |
| beacon push outside the lock | `19ae76e`, `922c61c`, then `0faef73`, `8893863` (tests) | finding 7: a busy child stages a verified round instead of waiting for its lock | 48 owned ASTs identical, `24fbe36d…` (`…/2026-10-30_beacon-push-outside-lock/REPORT.md:186-194`) |
| v3-weights-hardening | `b0182a7`, `04b250c`, `651c64e`, `f3d6711`, `05dbd89`, `fc287a2`, `433effe`, `b3af43f` | `_tx` rolls back a transaction whose COMMIT raised (`store.py:637-644`); pins of hotfix 4 and of the beacon lot; the two strict xfails of finding 5 | owned set and ASTs identical, `24fbe36d…` (`…/2026-10-31_v3-weights-hardening/REPORT.md:414-423`) |
| pause witness without built services | `fadf237`, `f3eb24e`, `730b5ee`, `130117b`, `7fb4e53`, `f357e61`, `7d63ed1`, `8e62065` | `push_beacon` witnesses the pause of every v2 run, its services built or not; after a restart a pause used to leave no `beacon-pause` record and silent members were booked MINER instead of WAIVED. ADR 0011 Part 1 updated; the rule is Part 8 decision 5 (`0011-audit-integrity.md:152-156,1400-1403`) | `24fbe36d…` at `b3af43f`, `a3fc635`, `8e62065` (`…/2026-10-31_beacon-pause-witness-explain/REPORT.md:160-169`) |
| v3-ledger-small-fixes | `35a8a22`, `6378ec2`, `ccee05f`, `416318d`, `b3aede9`, `bc306dc`, `a3fc635` | the operator tool's gap cap pinned at the production default; ES-07 `UNKNOWN_RECIPIENT`; RL-01 `ESCROW_QUOTA`, the owner checked first; TRN-05, a budget of objects and bytes per hotkey and window (413 `OBJECT_QUOTA`); ADR 0014 Part 5 | `7af135ca…`, the example pin, unchanged (ADR 0014 `:493-512`); `24fbe36d…` measured at `a3fc635` here. Its REPORT names `b92f3b9`, `8facda7` and `c0bbdfb`, rebuilt before the merge; `a3fc635`'s tree equals `c0bbdfb`'s |

## Annex B. Backlog, not in the train

1. **An unreadable manifest is not cached** (Lens minor on `f357e61`,
   `livrables/lens/redteam-live/delta-verdicts.md:689`; backlog by D141, journal-01 `:133`).
   `_witness_unbuilt_v2` caches a run's beacon clock only on success or on a `ChallengeError`, and
   `MalformedEnvelope` is a `ValueError`, so a permanently malformed manifest is re-read on every push, about
   every 3 s, in a `BEGIN IMMEDIATE` transaction that is opened with the journal left unchanged
   (`store.py:7301-7308`, `:7353-7360`; commit
   `130117b`) [V by reading]. The cost is bounded and untested. The names involved are not owned, so the fix
   is hash-neutral [V]. The brief calls it m-1 and gives the commit as "130fd…": it is `130117b`.
2. **TRN-05 is only an anti-abuse ceiling on the pinned runs.** Without `max_refs` and `max_parts` the budget
   is 4,194,816 objects and about 4 TiB per hotkey and window (ADR 0014 `:333-350`; Lens's reserve,
   `delta-verdicts.md:686`) [A]. The s100 runs pin admission policy `7a881925…` without the bounds [V, the
   fixtures load with both `None`], and S31 was built without the knobs because the GPU image `cb52fe37`,
   built before TRN-05, refuses the new fields (`extra="forbid"`; journal-01 `:138`) [A]. A disk quota that
   bites needs new miner and server images and a new run; it is not an owned body.
3. **ADR 0015's R2 assumption is to revise.** Its budget takes R2 from a node at 20 MB/s on one stream and
   100-144 MB/s on eight (`0015-intermediate-dense-test-model.md:188`, prerequisite f at `:346`). The 28/10
   source gives PUT 101-122 and GET 140-144 MB/s on eight streams and 19.2-26.7 MB/s on one; S31 measured
   4.2-4.9 and 19-56 MB/s on the unhinted bucket and 21.8-22.7 MB/s on one stream with the APAC hint (finding
   8) [A]. D147 asks for the revision (journal-01 `:156`); this record does not edit ADR 0015.
4. **Seen in S31, Forge's backlog**: `beacon-pause` records keep growing by about 10 a minute per store while
   nothing runs, and the admission route answers 503 "beacon infrastructure paused" now and then while idle
   (`REPORT-S31.md:295,457`) [A]. Not investigated here.
