# ADR 0018: T3, the audit lanes and the first audit window read from the signed audit policy

- Status: draft, 2026-11-01 (brief `adr-0017-t3-design`; D157 and D159, Principal SN100). Design only ("ADR, sans
  code", D157, `agents/principal-sn100.md:39`): Lens reads it, then Principal SN100 validates it before any code is
  written (`livrables/forge/briefs/adr-0017-t3-design.md:3`). Written on branch `adr-0017-t3-design`, started from
  `adr-0017-consolidated` `51800d9`, against origin/v3 `dc014508` (still the tip of `v3` by `git ls-remote` on
  2026-11-01 at 17:37Z). No file under `hypertrain/` changes, so no hash moves in this lot [V].
- Number: 0018 is free on every branch tip of origin, checked on 2026-11-01 [V]. Besides ADR 0017 itself
  (`adr-0017-consolidated`), two unmerged drafts are numbered 0017, `0017-admission-trial-window.md`
  (`v3-trial-rounds` `f4f2bc2c`) and `0017-miner-fast-path.md` (`v3-miner-fast-path` `f6b9ce02`); none holds a
  0018 [V]. If one of them is renumbered later, it takes 0019 or above; Forge decides.
- Labels, paths and abbreviations are those of ADR 0017 (`0017-single-implementation-hash-train.md:19-31`).
  **[V]** proved: read in code at `dc014508`, run as a command, or pinned by a test. **[A]** reported by a cited
  document. **[H]** inferred, or arithmetic on cited figures. **[?]** unknown. Code paths are relative to
  `hypertrain/src/hypertrain/`. "S31/" is `livrables/forge/2026-10-31_s31-beacon-validation/` and "settled/" is
  `livrables/forge/2026-10-30_s21-s22-round-settled/`.
- **The probe.** A scratch prototype of this design, made on a `git archive` of `dc014508` in
  `/var/lib/openbot/scratch-forge/adr-0017-t3-design/`. It is not delivered, and it is deleted at the end of the
  session. Part 4 gives its edits and Part 7 its tests. The session's folder
  `livrables/forge/2026-11-01_adr-0017-t3-design/` keeps its diff (`probe/src.diff`, `probe/tests.diff`), its
  scripts and mutant results (`probe/mutants.txt`), and its `REPORT.md` the commands and outputs. It shows
  that the design can be built and what it moves. It is not the code lot: that lot is written after validation,
  red test first.
- Related: ADR 0017 findings 2, 3 and 6 and its train table (`:176-292`, `:409-469`, `:565-577`). ADR 0011 Part 2
  (audit liveness and the rearm, decision 8; its rejected "new `AuditPolicyV2` field for the lengths",
  `0011-audit-integrity.md:203-262`), Part 8 (the exit planner) and Part 9, decision 9 (`reaudit_rounds`, the
  precedent for an optional field omitted when absent, `:2080-2113`) and decision 12 (the gap cap). ADR 0008
  (one executor job per host, `0008-generic-cuda-executor.md:212-213`).

## Decision in brief

T3 reads two values from the signed audit policy, and proposes both as TEST-only for now (open question 7):
- **Lanes: the existing `max_running_jobs`.** It is widened from `Literal[2]` to `Positive` (1 to 2^53 - 1), with
  the cap `min(32, listed auditors)` enforced by `validate_training` only (the schema itself accepts up to
  2^53 - 1, so 33 passes the schema and is refused there), and `lease_v2` reads it in place of its literal `>= 2`.
- **First window: a new optional field, `first_window_rounds`.** It is omitted from the canonical body when it is
  absent, and absent means today's `absolute_rounds()`. `schedule_audits_v2` reads it, and so does the unowned
  code that copies the window: the exit planner's `Schedule.absolute` (with `audits_end`, which assumes it) and
  `_retire_round_v2`. The rearm keeps at least the job's stored first-window end, so a signed window is never cut
  short by a rearm.

Every lease keeps the two-lease bound that `AuditJobV2` already enforces. No auditor-count field is added: the
signed `training.auditors` list already says who may serve. `manifest.training.auditors[:2]` only fills a record
that no code reads by kind (the rollback digest hashes it, Part 2.3).

The design edits two owned names, `schedule_audits_v2` and `lease_v2`, and proposes to leave `open_round_v2`
untouched (open question 6). It needs no new protocol constant, no changed default and no policy field outside
the lanes / auditors / first window family. While the new values are absent, every pinned run keeps its bytes and
its run id.

T3 does not buy executor capacity, and it does not fix 101 M audits: see "Back to the founder".

## Context

### What T3 is

ADR 0017 scopes one hash train to T1 + T2 + T3 (D157). T3 is the row "2 and 3: lanes, auditors and the first
audit window read from the signed audit policy", with the owned names `schedule_audits_v2`, `lease_v2` and
`open_round_v2` "if service runs need it" (`0017-single-implementation-hash-train.md:569`). Finding 2 recommends
reading `max_running_jobs` from the signed policy and one design for the first window (`:233-236`). Finding 3
recommends a first-window rule in the train, the `AuditInputError` handler shipped hash-neutral before it, and
`absolute_rounds()` left as it is (`:289-291`).

### The premises, re-read on `dc014508`

| Premise (brief, ADR 0017, Lens) | Survives? | Evidence |
| --- | --- | --- |
| "The auditor count" is `manifest.training.auditors[:2]` | **No** | `[:2]` only fills the `audit-reservation` record (`challenge/store.py:8420-8430`, the slice at `:8428`), which no code reads by kind; the generic `_rollback_snapshot_v2` hashes it with every other record (`:11970-11976`) [V]. Any listed auditor may lease (`:8441-8442`) [V]. |
| What bounds concurrent leases is `lease_v2`: `>= 2` running leases and one live lease per auditor | **Yes** | `:8547-8553` and `:8543-8546` [V]. Both counts have no `run_id`, but every run after the first lives in its own child store with its own `audit_leases_v2` (`:607-620`, `:736-738`, `:2685-2695`), so in practice the count is per run [V]. It also has no `w`: rounds `w` and `w+1` share the lanes [V]. A third bound is outside the store: the CUDA executor runs one job per host (Part 1.8). |
| `max_running_jobs` is read nowhere and written as a literal 2 in `round-resource` (`:5926`, `:6021`) | Yes, with a limit | Both writes are in `open_round_v2`'s service paths (`:5918-5929`, `:6019-6023`) and no code reads those keys [V]. A service-admitted run never reaches a lease or a schedule: `heavy=True` answers 503 first (`:8535`, `:8353`) [V]. Part 6 (a). |
| The first window is `absolute` in `schedule_audits_v2` | **Partly** | The same `min(d_final, d_audit + 2·lease_rounds)` is computed again outside the owned set: in `exit_v2.Schedule.absolute` (`challenge/exit_v2.py:83-85`, used at `:104`, `:107`, `:284`) and in `_retire_round_v2` (`challenge/store.py:7215`). `audits_end = d_audit + 4·lease_rounds` (`exit_v2.py:87-90`) assumes it; it feeds `:106` and, through `reaudit_end` (`:93-96`), `Wait.DISPUTE` (`:102`) and `Wait.REAUDIT` (`:108`) [V]. |
| (not stated) | New | `AuditJobV2._bounds` refuses `absolute_deadline > created_beacon + 2·audit_lease_rounds(H, 16.0)`, which is 200 rounds at H ≤ 30 (`protocol/messages_v2.py:449-455`) [V]. `lease_v2` sets `created_beacon = now` (`challenge/store.py:8623`). A first window that simply ends later than 200 rounds after a lease makes that lease answer 409 (Part 1.6). |
| The service branch hard-codes a gap of 200 (`:5724`, `:6017`, `:6025`) | Yes, for 5724 and 6017 | `:6025` is the `d_final - d_audit < 2` check of every open outside the prepared service path, which has its own at `:5723`; neither is a 200 [V]. |
| `_prime_finality_replay_v2` keeps its verdict 120 s after the replay ends (`:2944`, `FINALITY_VERDICT_SECONDS` `:361`): a prompt retry does not replay (Lens) | Yes | `:2944` sets the expiry once the replay has returned, and `:2863` reads it [V]. T3 does not touch it, and this record makes no claim of a 503 at every retry. |

## Part 1. Current behaviour at `dc014508`

### 1.1 `schedule_audits_v2` (owned, `challenge/store.py:8276-8432`)

- It counts the queued and running jobs of the store (`:8363-8365`, no `run_id`). It answers 503 when they and
  the jobs to add would pass `policy.max_queued_jobs` (32) (`:8380-8381`) [V].
- **The first window**: `absolute = min(body.d_final, body.d_audit + policy.absolute_rounds())` (`:8382`). It
  answers 503 "audit absolute horizon exhausted" when `absolute <= now + 1` (`:8383-8384`) [V].
- Each job gets an `AuditChallengeV2` with `serve_deadline = absolute` (`:8396`), sealed until `absolute`
  (`:8400-8402`). It is inserted with `created = d_audit` and `absolute` (`:8404-8419`) [V].
- It writes the `audit-reservation` record, with `"attempts": 2` (`:8425`) and `"auditors":
  list(manifest.training.auditors[:2])` (`:8428`) [V].

### 1.2 `lease_v2` (owned, `:8434-8632`)

- Only a listed auditor may lease (`:8441-8442`, 403) [V]. In a service-admitted run, the service branch ends in
  `_service_boundary_v2(..., heavy=True)` (`:8535`), which answers 503 whenever a service estimate exists
  (`:5241-5244`) [V].
- Generic path:
  - It expires leases (`:8539`) and refuses a cooled-down auditor (`:8540-8542`).
  - It refuses 409 "one live lease per auditor" (`:8543-8546`).
  - It returns None, which the route answers as 204 (`challenge/app.py:954-957`), while
    `SELECT COUNT(*) … state='running'` is `>= 2` (`:8547-8553`) [V].
  - It skips the jobs that this auditor already tried and the job of its own hotkey (`:8560-8564`).
  - It reads `lease_rounds` from the signed policy (`:8595-8597`) and sets `expires = min(now + lease_rounds,
    row["absolute"])` (`:8598`) [V].
  - It builds `AuditJobV2` with `absolute_deadline=row["absolute"]` (`:8611`) and `created_beacon=now` (`:8623`)
    [V].
- **The lane test today** is `test_an_auditor_holds_one_lease_and_the_store_at_most_two`
  (`hypertrain/tests/ledger_invariants/test_audit.py:99`). Its fixture lists three auditors
  (`hypertrain/tests/ledger_invariants/support_audit.py:147`), and the third gets 204 (`test_audit.py:110`). The
  mutant `aud-lease-store-cap-of-three` (`hypertrain/tests/ledger_invariants/mutants_audit.py:119`) names it [V].
  The other fixtures list two auditors (`hypertrain/tests/challenge/test_service_network_v2.py:61`), so one lease
  per auditor caps them at two already [V].

### 1.3 Expiry, retire and the rearm (not owned)

- `_expire_leases_v2` (`:8724-8738`): a running lease past its end is retired. A queued job at
  `now >= absolute` is exhausted as `AUDIT_INFRASTRUCTURE` [V].
- `_retire_audit_attempt_v2` (`:8764-8806`): it requeues the job, or exhausts it if `attempts >= 2` (`:8773`), if
  `now >= absolute`, or if no untried auditor is left (`_auditors_left_v2`, `:8833-8836`) [V]. Only an
  `AUDITOR_SILENT` attempt cools its auditor down. An auditor whose execution is still `REQUESTED` when its lease
  ends is `EXECUTOR_INTERRUPTED` and is not cooled down; one that never asked for the execution, or whose verdict
  was stored (`EXECUTED`) and left unsigned, is `AUDITOR_SILENT` (`:8754-8762`, `:8795-8801`) [V].
- `_exhaust_audit_job_v2` (`:8965-8996`) and `_rearm_audit_job_v2` (`:8998-9043`):
  - A job gets at most `policy.max_attempts` windows (`:9020`).
  - The second window is `min(now + absolute_rounds(), commit.exp_drand)` (`:9025`), and none is opened when that
    leaves fewer than two rounds (`absolute <= now + 1`, `:9026-9027`).
  - The rearm resets `attempts`, `tried` and `created` (`:9032-9036`). A service-admitted run never rearms
    (`:9006-9010`) [V].
- An honest miner seals its commit until `d_final` (`miner/core.py:1436`), so a job that is exhausted at
  `d_final` gets no rearm [V].

### 1.4 `absolute_rounds()` and its copies

- `AuditPolicyV2.absolute_rounds()` is `2·lease_rounds` (`protocol/messages_v2.py:805-806`). `lease_rounds` must
  equal `audit_lease_rounds(H, step bound)` (`:784-785`), which is 100 for H ≤ 30 (`:733-738`) [V]. Every pinned
  run has H = 4 and a 100-round lease (fixtures `hypertrain/tests/ops/s100/fixtures/{0ef600b4,f3901a09}/`;
  4d84aacc by its identical policy, `S31/evidence/golden-newrun.txt:4-5`) [V; A for 4d84aacc].
- The same first window is computed again in `exit_v2.Schedule.absolute` (`challenge/exit_v2.py:83-85`). That
  property feeds `Wait.LEAVES` (`:104`), `Wait.AUDIT_WINDOW` (`:107`) and the planner's `AUDIT_WINDOW_MISSED`
  (`:284`). `audits_end = d_audit + 4·lease_rounds` (`:87-90`) assumes two windows of two leases. Through
  `reaudit_end` (`:93-96`) it sets `Wait.DISPUTE` (`:102`), which `_contest_hold_v2` reads to end the hold of a
  contested NO_SHOW (`challenge/store.py:7705`) [V].
- `_retire_round_v2` refuses `AUDIT_WINDOW_MISSED` while `now < min(d_final, d_audit + absolute_rounds()) - 1`
  (`challenge/store.py:7215`) [V]. `_reap_round_v2` (`:7414-7474`) and `_contest_hold_v2` (`:7647-7707`) build the
  `Schedule` (`:7448-7456`, `:7680-7688`). None of these three functions is owned. `_contest_hold_v2` is the only
  reader of `BOUNDS` in `src`, for `COMMIT`, `LEAVES`, `DELTA` and `DISPUTE` (`:7689-7705`) [V].
- `absolute_rounds()` is also the cooldown cap (`:8957`, `challenge/audit_cooldown_v2.py:89-93`) and the rearm
  window (`:9025`). Those are other meanings. T3 keeps the cooldown cap, and keeps `absolute_rounds()` as the
  rearm's length, but no longer lets a rearm end before the job's first window (Part 2.4).

### 1.5 The schema literals and `validate_training`

- `AuditPolicyV2` (`protocol/messages_v2.py:755-815`) has `max_attempts: Literal[2]` (`:758`),
  `max_concurrent_per_auditor: Literal[1]` (`:759`), `max_running_jobs: Literal[2]` (`:760`) and
  `max_queued_jobs: Literal[32]` (`:761`). It also has two optional fields that are omitted when absent,
  `reaudit_rounds` (`:766-768`) and `gap_cap_rounds` (`:772-774`) [V].
- `max_concurrent_per_auditor` and `max_running_jobs` are read nowhere in `src`. `lease_v2` hard-codes both
  values [V].
- `WireModel` is `extra="forbid"` (`:65`). `digest()` is the sha256 of the JCS bytes of `body()` (`:83-87`). An
  older reader therefore refuses a policy that carries a new key, even as `null`, and refuses a
  `max_running_jobs` other than 2 (`literal_error`; probe) [V].
- `validate_training` (`:776-797`) checks:
  - the step budget (`:782-783`) and the lease (`:784-785`);
  - one lease ≤ re-audit window ≤ two leases (`:787-788`);
  - `gap_cap() ≤ 2·absolute_rounds() + reaudit_window()` (`:792-793`);
  - a gap cap above 200 only on a TEST ledger, where a ledger mode is passed (`:794-797`).
- It is called at run creation with the economics policy's ledger mode (`challenge/store.py:2697-2713`) and in
  the builders (`ops/manifest_v2.py:90`, `hypertrain/scripts/b300/d20/build_test_manifest.sh:591`). It is called
  without a ledger mode by `hypertrain/scripts/ops/manifest/od_large_manifest.py:507` and, again, in the service
  branch of `schedule_audits_v2` (`challenge/store.py:8352`), where a ledger-mode rule does not run [V].
- Run creation also requires two distinct auditors (`:2717`). The manifest allows at most 128 unique ones
  (`protocol/messages_v2.py:213-214`) [V].

### 1.6 The structural bound of `AuditJobV2`

- `AuditJobV2._bounds` (`protocol/messages_v2.py:441-479`) requires `created_beacon < lease_expires ≤
  absolute_deadline` (`:447`). It refuses `lease_expires > created_beacon + lease` and `absolute_deadline >
  created_beacon + 2·lease`, with `lease = audit_lease_rounds(H, MAX_STEP_SECONDS_BOUND)` (`:451-455`), and an
  `absolute_deadline` past the challenge's `serve_deadline` (`:469`) [V].
- At H ≤ 30 the bound is 200 rounds after the lease round. A probe of `AuditJobV2` at H = 4 got past the bound
  at 200 (and stopped at a later check on its placeholder assignment), and was refused at the bound at 201 and
  400 [V]. The probe's store test below leases exactly 200 rounds ahead.
- A second probe made `schedule_audits_v2` seal `absolute = d_audit + 250`: every `/worker/lease` then answered
  409 until `now >= absolute - 200` [V].
- The auditor client re-validates the job (`auditor/__main__.py:120-128`). Its only deadline is the round before
  `min(lease_expires, absolute_deadline)`, in wall time (`:131-135`), and it seals its execute receipt and verdict
  until `lease_expires` (`:138`, `:147`) [V].

### 1.7 `open_round_v2`: the service branch, its gap and its literals (owned, `:5690-6133`)

- The prepared open of a service run (w = 0) refuses `d_final > d_audit + 200` (`:5724`). It writes
  `round-resource` with `"max_attempts": 2`, `"max_running_jobs": 2` and
  `"memory_reservation_bytes": 1 << 30` (`:5918-5929`) [V].
- The generic open's service block refuses `d_final > d_audit + 200` (`:6017-6018`) and writes the same three
  literals (`:6019-6023`). That block is not reachable in normal flow: a service-admitted run returns at `:5969`,
  is refused inside the prepared path (for example `:5732`), or meets the heavy 503 at `:5700` or `:5970`
  (`:5241-5244`) [V by reading]. Every open outside the prepared path checks `d_final - d_audit >= 2`
  (`:6025-6026`; the prepared path at `:5723`) [V].
- No code in `src`, tests or scripts reads those three keys. `deadline` is read (`:5295-5297`, `:10484-10487`).
  `_rollback_snapshot_v2` digests the record's bytes with every other record (`:11970-11976`) [V].
- A run without a service-admission record, as every pinned TEST run is (`service_admission_rows` 0,
  `S31/evidence/ident-0ef600b4-before.json:73`, `ident-newrun.json:45`;
  `livrables/forge/2026-10-29_s19-swap-round2/evidence/ident-before.json.txt:123` for `f3901a09`), never writes
  `round-resource` [V for the code, A for the rows].
- For the tiny service profile, the 200 equals `absolute_rounds()` (H = 2, a 100-round lease). The code does not
  derive it from that: it is a literal [V].

### 1.8 What runs an audit, and how many at once

- The auditor CLI holds one lease at a time: lease, then `/execute` (it blocks for the replay), then `/complete`
  (`auditor/__main__.py:68-153`). The TEST containers loop on it with `sleep 20` (two of them,
  `hypertrain/scripts/ops/s100_test/test_auditors.sh:14,36-42`). They run the server image, because the replay
  runs in the challenge (`:5-6`) [V].
- `execute_audit_v2` hands the island to the configured CUDA executor (`challenge/store.py:9637-9656`). The
  executor keeps one lock per host label (`gpu_ops/cuda_executor.py:622`, "a host runs one attempt at a time",
  `:628`). It takes the first free host and waits for a busy one until the job's deadline (`:646-662`) [V].
  ADR 0008 states the same: one job per host (`0008-generic-cuda-executor.md:212-213`) [V].
- Every TEST run built from S17d to S31 audited on two hosts, a and b, with two signed auditors (S17d
  `REPORT-S17D.md:14,17,56`; S20 `REPORT-S20.md:36,40,42`; S31 `REPORT-S31.md:45,50,54`) [A].
- The three executor journals show audits in parallel only as one per host, and 0 overlaps on the same host (the
  `exec_attempt` / `exec_published` pairs of
  `settled/executor-journals/executor-state-{real31m,s20}-journal.jsonl` and `S31/evidence/exec-journal-s31.jsonl`)
  [A, paired in this session].
- The D20 builder signs exactly two auditors (`build_test_manifest.sh:289`, `:432`) and its dry run requires two
  (`:660`) [V].

## Part 2. Target semantics

### 2.1 The fields

| Field | Lives in | Type and bounds | Absent or default | Validated by | What moves when it is signed |
| --- | --- | --- | --- | --- | --- |
| `max_running_jobs` (lanes, existing) | `AuditPolicyV2` (`messages_v2.py:760`) | `Positive` (`StrictInt` 1 to 2^53 - 1, `:46`, as the policy's other integers; JCS refuses more), was `Literal[2]`. In `validate_training`: above 2 it needs at least as many distinct `training.auditors`, and at most `max_queued_jobs` (32). A value other than 2 is for a TEST ledger only | Required and always serialised: 2 serialises as `2`, as today, so a policy that signs 2 keeps its bytes | `validate_training` with the ledger mode, at run creation and in the builders | `audit_policy_hash`, hence the run id (`RunManifestV2.run_id()` is the digest of the whole manifest, whose `network` carries the policy hashes, `messages_v2.py:256-286`). Lanes above 2 also need more listed auditors, so the training and the run id move with them |
| `first_window_rounds` (new) | `AuditPolicyV2` | `Annotated[StrictInt, Field(ge=2, le=2**53-1)] \| None`, `exclude_if=None`, as `reaudit_rounds`. When present: `absolute_rounds() ≤ value ≤ gap_cap()`, on a TEST ledger only | Omitted: `first_window()` returns `absolute_rounds()`, today's window. An explicit `null` is dropped: same bytes, same digest (probe) | `validate_training`, as above | Same as above |
| `training.auditors` (who may serve, existing) | the training manifest (`messages.py:374`) | 2 to 128 distinct keys (`messages_v2.py:213-214`, `store.py:2717`) | Unchanged | Unchanged | No new field. Listing more keys already changes the training, hence the run id |
| `max_concurrent_per_auditor` (existing) | `AuditPolicyV2` (`:759`) | `Literal[1]`, unchanged | Unchanged | Unchanged | Nothing: still unread, and `lease_v2` keeps one lease per auditor (`store.py:8543-8546`) |

`max_attempts` (`Literal[2]`), `max_queued_jobs` (`Literal[32]`), `lease_rounds` and `absolute_rounds()` do not
change.

### 2.2 Lanes

- `lease_v2` returns None (204) while the store's running leases number `policy.max_running_jobs` or more, where
  `policy` is the signed audit policy of the run that asks. The count stays store-wide, as today. Because each
  run after the first has its own child store, that means per run, and shared by the rounds of the run (the
  premises table, Context) [V by reading].
- The policy is now read before the count, not after a job is found (`:8595-8597`). A poll that would answer 204
  (lanes full, `:8547-8553`, or no leasable job, `:8592-8593`) therefore fails instead if the policy object cannot
  be read. That object is pinned at run creation and read by every later audit path, so this is an edge case [V
  by reading; H].
- The number of audits that can run at once is the least of three things: the lanes, the listed auditors that
  poll (one lease each), and the executor hosts (one job each; Part 1.8). The lanes and the auditor list are
  signed; how many of the listed auditors poll, and how many hosts the executor has, are operational, and the
  run's builder and runbook must provide them. `validate_training` checks lanes against the listed auditors, but
  it cannot see the hosts [V by reading; H for the operating rule].
- A lane count of 1 is allowed on a TEST ledger. It serialises the audits, which is useful to measure one replay
  alone [H].

### 2.3 The auditors that serve

- Who may serve is the signed `training.auditors` list. How many serve at once is set by `max_running_jobs` and
  one lease per auditor. T3 adds no auditor field [V by reading].
- The `audit-reservation` record keeps `auditors[:2]` and `"attempts": 2`. Nothing reads it by kind, but
  `_rollback_snapshot_v2` digests every record (`store.py:11970-11976`). Changing its content would change the
  rollback revision digest of every later round of a pinned run under unchanged inputs, so T3 leaves it as it is
  (open question 4) [V by reading].
- More lanes need more listed auditors, and the TEST tooling makes two. The D20 builder (`:289`, `:432`, `:660`)
  and `test_auditors.sh` (`:14`, `:36`) need a count. That tooling is hash-neutral; a new count is a new
  training, hence a new run id [V by reading].

### 2.4 The first window

- **Rule.** `absolute = min(d_final, d_audit + policy.first_window())` everywhere the first window is used:
  `schedule_audits_v2` (`:8382`), `exit_v2.Schedule.absolute` (`:83-85`) and `_retire_round_v2` (`:7215`).
  `Schedule.audits_end` becomes `d_audit + first_window + 2·lease_rounds` (`:87-90`), the latest end of a rearm
  window opened at the end of the first. `Schedule` gets a field `first_window_rounds` with no default, as
  `reaudit_rounds` has none (`:81`). `_reap_round_v2` and `_contest_hold_v2` pass `audit.first_window()`. With
  the field absent, every one of these values is today's [V by reading; probe].
- **Each lease keeps the two-lease bound.** `lease_v2` gives the job `absolute_deadline = min(row["absolute"],
  now + policy.absolute_rounds())`. Its `expires = min(now + lease_rounds, row["absolute"])` is unchanged.
  - Since `lease_rounds` ≤ `audit_lease_rounds(H, 16.0)`, the job always satisfies `AuditJobV2._bounds` (Part 1.6).
    The structural bound and the auditor client therefore need no change [V by reading; probe].
  - With the field absent, every row's `absolute` is at most `now + absolute_rounds()` at lease time. For the
    first window `now ≥ d_audit`; for a rearm `created = now`; for a re-audit, `until = min(commit_exp,
    max(detected + reaudit_window(), absolute))` (`exit_v2.py:167-172`, written at `store.py:8890-8898`), with
    `detected ≤ now` (`:8730-8731`, `:8870`), `reaudit_window() ≤ absolute_rounds()` (`messages_v2.py:787-788`)
    and no rearm after it (`store.py:8986-8988`). So the `min` returns `row["absolute"]`, today's value [V by
    reading; the probe's suites].
  - The service paths that bind `absolute_deadline` to the row (`execute_audit_v2` `:9337`, `complete_audit_v2`
    `:9901`, `verified_inputs_v2` `:10198`, and the execution descriptor `:9401` / `:10315`) are not reached,
    since service runs never lease (Part 1.2). The `min` could not bind there anyway: a service round's gap is at
    most 200 (`:5724`, `:6017-6018`) and it has no rearm (`:9006-9010`) or re-audit (`:8845-8856`) [V by
    reading].
- **The rearm never ends before the first window.** `_rearm_audit_job_v2` (not owned) sets `absolute =
  min(max(now + policy.absolute_rounds(), row["absolute"]), sealed.exp_drand)` instead of `absolute = min(now +
  policy.absolute_rounds(), sealed.exp_drand)` (`:9025`; `sealed` is the parsed commit envelope, `:9022-9024`).
  - With the field absent this is today's value: the row's `absolute` is at most `d_audit + absolute_rounds() ≤
    now + absolute_rounds()` [V by reading; probe].
  - With F signed, a job exhausted early (two fast executor failures, `_fail_audit_attempt_v2` `:9753-9792`, or no
    untried auditor left, `:8771-8777`) would otherwise be rearmed until `now + absolute_rounds()`, before
    `d_audit + F`. In the probe, two failures at `d_audit + 5` and `+ 8` rearmed the job until `d_audit + 208`
    against a signed `d_audit + 400`; with the `max` it keeps `d_audit + 400` [V, probe test and mutant].
  - The rearm still moves the job to the back of the queue (`created = now`, `:9032-9036`; `ORDER BY created,id`,
    `:8557`). The per-lease `min` keeps `AuditJobV2`'s bound, and the window stays inside `audits_end`
    (`now ≤ d_audit + F`) [V by reading].
  - A first window that reaches `d_final` leaves no rearm (Part 1.3). One shorter than the gap keeps up to
    `min(absolute_rounds(), gap - first_window)` rounds of rearm at its end [V by reading; H].
- **What a signed window delays.** Two things resolve at the end of the first window, so they move from `d_audit +
  absolute_rounds()` to `d_audit + F`: a member that never posted its leaves (dropped NO_SHOW only when its queued
  job is exhausted at `absolute`, `:8566-8573`, `:8732-8738`, `:8970-8985`) and `AUDIT_WINDOW_MISSED`
  (`exit_v2.py:284`). Until then `unresolved_audits` stays above 0 and the finalize refuses
  (`challenge/finality_v2.py:68-76`), so such a round settles up to `F - absolute_rounds()` rounds later. In the
  probe, a member without leaves stayed queued at `d_audit + 399` and was exhausted at `d_audit + 400` under
  F = 400, against `d_audit + 200` without the field [V, probe test].
  - A queued job with leaves that nobody leases is rearmed at the end of the first window, as today. It resolves at
    `min(d_final, d_audit + F + absolute_rounds())` instead of `min(d_final, d_audit + 2·absolute_rounds())`: no
    later at a gap of 400 or less, and later by at most `min(F + absolute_rounds(), gap) - 2·absolute_rounds()`
    rounds above it (two reviewers' scratch runs on the probe at gap 400: exhausted at `d_audit + 400` with F
    absent, 254 and 400) [V by reading; A for those runs].
  - `Wait.LEAVES` and `Wait.AUDIT_WINDOW` follow `Schedule.absolute`, so `_contest_hold_v2` counts a contest of a
    leaves-stage NO_SHOW opened before `d_audit + F` (`store.py:7690-7694`), where the job itself now exhausts. A
    contested NO_SHOW is also held up to `F - absolute_rounds()` rounds longer, because `Wait.DISPUTE` moves with
    `audits_end` (Part 1.4) [V by reading].
  - Only an audit that a lane reaches resolves earlier (Part 8).
- **Bounds.** It is never shorter than today (`≥ absolute_rounds()`) and never longer than the widest gap the
  operator tool may open (`≤ gap_cap()`, at most `2·absolute_rounds() + reaudit_window()`, 500 rounds at a lease
  and a re-audit window of 100, `messages_v2.py:792-793`). Effective bound: `min(d_final, d_audit +
  first_window)`. Each lease is still bound to `created + 2·lease` [V by reading].
  - On a production ledger the gap cap is 200 (`:794-797`), which is `absolute_rounds()` at H ≤ 30 and below it
    above H = 30. A first window there could only equal today's (H ≤ 30) or have no valid value at all (H > 30),
    so the field is refused outright outside TEST rather than allowed as a no-op [H].
  - The operator tool checks only `2 ≤ gap ≤ gap_cap()` and the re-audit fit
    (`hypertrain/scripts/ops/live_operator.py:130-155`, `:170-190`, `:213-215`). A round opened at a gap below F
    silently gets `d_final` as its first window and no rearm. T3 adds a hash-neutral refusal there: a gap below
    `first_window_rounds`, when the field is set [V by reading; design].
  - The existing gap rule `gap_cap() ≤ 2·absolute_rounds() + reaudit_window()` stays. With a wider first window
    it is stricter than the new `audits_end` requires, which is safe [H].
- **Derivation, by the publisher.** As with `reaudit_rounds` (ADR 0011 Part 9 decision 9), the value is measured,
  not derived from the manifest: the verifier checks its range, and the signature binds it into the run id. T3
  adds a pure helper next to `reaudit_rounds_for` (`messages_v2.py:746-752`; not hashed):

  `first_window_rounds_for(audits, lanes, cycle_seconds, lease_rounds) = max(2·lease_rounds, (ceil(audits /
  lanes) - 1)·ceil(cycle_seconds / 3) + lease_rounds)`

  - The last wave starts after `ceil(audits / lanes) - 1` lane cycles and needs one full lease.
  - `cycle_seconds` is the largest measured lease-to-complete of the run's model, measured at the signed lane
    count on as many hosts. `lanes` is the parallelism the run really has: the least of the lanes, the polling
    auditors and the hosts (2.2). `audits` is the planned roster.
  - Cycles are counted from `d_audit`; the first lease came 12-38 s after it in the gap-400 rounds (Part 8). At
    31 M the full last lease absorbs that delay and the 0-8 s between waves: 100 rounds against a 77-round cycle
    leave 23 rounds (69 s), against at most 38 + 2·8 = 54 s for three waves. A model whose cycle nears the lease
    would need the delays added [H].
  - Worked [H]: at 31 M the gap-400 watches give 181-229 s lease to complete (190-229 s paired, 181-206 s alone;
    Part 8). Five audits, two lanes and 229 s give 3 waves and 77-round cycles: 254 rounds (762 s), which needs a
    gap of at least 254 and leaves a rearm window of 146 rounds at a gap of 400. Five audits on three lanes give
    177, floored to 200: the field is not needed. One lane gives 408, which needs `gap_cap_rounds` ≥ 408 (at most
    500) and rounds at that gap.
  - The fifth audit of each gap-400 round needed 613-666 s from `d_audit` to finish its first attempt (Part 8),
    205-222 rounds, and at most about 230 rounds (689 s) with the latest start (460 s) and the slowest paired
    cycle (229 s). 254 rounds (762 s) cover it [H].

### 2.5 TEST-only, and production

`validate_training` refuses a `max_running_jobs` other than 2 and a present `first_window_rounds` when a ledger
mode other than `test` is passed. That is the same door, and the same `None` exemption at re-check, as the gap cap
(`messages_v2.py:794-797`).

A production ledger therefore keeps two lanes and a first window of `absolute_rounds()` (200 rounds at H ≤ 30)
until a TEST run measures the new geometry (ADR 0017 finding 2's recommendation). Lifting the lane rule later edits
`messages_v2.py` only. A production first window above 200 also needs the gap-cap TEST rule lifted, in
`messages_v2.py:794-797` and in the operator tool (`hypertrain/scripts/ops/live_operator.py:158-167`). None of
these files is hashed; each change ships with a server image (open question 8). [V by reading, design]

### 2.6 Who reads the new values, and the image order

- Only the master, the builders and the operator tools parse `AuditPolicyV2` (`ops/manifest_v2.py:38,80-90,233`;
  `hypertrain/scripts/ops/live_operator.py:209`; the D20 builder `:583-597`) [V]. The auditor client, the miner
  and the GPU-side island worker never import it. They see only `audit_policy_hash` in the manifest
  (`miner/island_worker.py:39`, `gpu_ops/cuda_executor_runner.py:346`) [V].
- `AuditJobV2` keeps its schema and its bounds, so auditors on any image accept T3's jobs [V by reading].
  `AuditPolicyV2`, `AuditJobV2` and `AuditChallengeV2` are identical at `e5900c3b` (hotfix 2a, the source of the
  server image `9be7d51d…` the TEST auditors ran in S31, `S31/evidence/auditors-real.txt:2`), at `667695be` (the
  source of the GPU image `cb52fe37`) and at `dc014508`: `messages_v2.py` changed in that range only in
  `ArtifactLimits` and `WorkProof` [V, `git diff`; A for which commit built each image].
- A run that signs either field can only be created on, and served by, a server image that holds T3. An older
  master refuses its policy (`literal_error`, `extra_forbidden`) [V, probe]. So the images ship before such a run
  is built, as for `reaudit_rounds` (`0011-audit-integrity.md:1944-1950`) [A].
- A rollback below T3 breaks every run that signs either field, because the old image cannot parse its policy.
  It is worse than that run alone. On the old image, each beacon push expires the run's leases outside any guard
  (`challenge/store.py:1017`), and an expiry that cools its auditor down, opens a re-audit or rearms a job parses
  the policy (`:8929`, `:8867`, `:9012`). The error then leaves that child's `push_beacon`, and the root forwards
  pushes to its children in a plain loop (`:959-960`), so the children after it stop getting beacons. No round of
  such a run can be planned or retired on the old image either: retirement comes only from the reaper, and
  `_reap_round_v2` parses the policy (`:7423`) before any exit or retirement; the failure is only journaled
  (`:7331-7336`), and `:7208` is one case behind it. The precondition of a rollback is therefore operational, on
  the T3 image: every round of every such run closed (finalized or retired), no queued or running
  `audit_leases_v2` row left, and no new round opened. This is not the `RunRetired` state (`:375-376`), which the
  old image would not apply to such a run (Part 9) [H by reading].

## Part 3. Hash neutrality while the fields are absent

| Run | Audit policy | Without the new fields | Run id | Pinned by | Shown by |
| --- | --- | --- | --- | --- | --- |
| `f3901a09` (S17d, S19) | `1dd433fe…` (no gap cap) | `max_running_jobs` 2 serialises as `2`; no `first_window_rounds` key | `f3901a09…` unchanged | `hypertrain/tests/ops/s100/test_run_f3901a09_golden.py:18-20,33`; builder `hypertrain/tests/test_b300_d20_builder.py:701` (the policy hash only) | probe schema: digest sets identical; golden passes on the probe [V] |
| `0ef600b4` (S20-S26) | `86ac1ac3…` (`gap_cap_rounds` 400, `reaudit_rounds` 100) | same | `0ef600b4…` unchanged | `test_run_0ef600b4_golden.py:19-21,50`; builder `test_b300_d20_builder.py:877` (the policy bytes; that build's run id is `178dc81e…`) | same [V]. Its `IMPLEMENTATION_HASH` (`:23`) moves, as for any owned edit |
| `4d84aacc` (S31) | `86ac1ac3…`, byte-identical to `0ef600b4`'s (`S31/evidence/golden-newrun.txt:4-5,16`) | same | `4d84aacc…` (`golden-newrun.txt:2`) | no fixture in Git (`git grep 4d84aacc` at `dc014508` is empty) [V] | through the `0ef600b4` policy bytes [V for the bytes; A for the identity of the two policies] |
| Example, v2 audit policy | `8243bd06…`, run id `e0f4c68d…` (`hypertrain/tests/protocol/test_network_v2.py:1207-1208`) | same | unchanged | `test_network_v2.py:1231-1240`, `:1271-1293` | probe [V] |
| Example, v1 manifest | none (`aa39197d…`, `test_model_arch.py:28`, `test_config_v2.py:13`, `test_od_fields.py:14`) | no audit policy at all | unchanged | same | `hash_consequences.py` on the probe: `example_manifest` equal [V] |

**Commands run** (the REPORT keeps the full outputs). Every pytest run was `cd <tree>/hypertrain &&
PYTHONPATH=<tree>/hypertrain/src TMPDIR=<scratch>/t PYTHONDONTWRITEBYTECODE=1 <scratch>/venv/bin/python -m pytest
-p no:cacheprovider --basetemp=<scratch>/pt/<label> -q <files>`, in a venv made by `uv sync --frozen --offline
--extra trainer` (CPython 3.12.15). `<tree>` is the `git archive` of `dc014508` or the probe.
- On `dc014508`, the pin set (the two goldens, `test_model_arch.py`, `test_config_v2.py`, `test_od_fields.py` and
  `test_network_v2.py`): 192 passed.
- On the probe:
  - The same pin set plus `challenge/test_service_network_v2.py`, `ops/test_manifest_v2.py` and
    `ops/test_live_operator.py`, all markers: 408 passed and 1 failed, the golden `IMPLEMENTATION_HASH`, as
    intended (`d4694fa9…` against `24fbe36d…`).
  - `test_b300_d20_builder.py`: 103 passed. The probe leaves the builder unchanged, so this shows that today's
    builder still gives `1dd433fe…` (`:701`) and the `0ef600b4` policy bytes (`:877`) under the widened schema.
    In the same run `audit_cheaters/test_mutation_runner.py:79` passed and the ledger snippet guard failed on the
    two re-anchors of Part 7.
- The schema probe (this session's reader, a scratch `probe.py` on a copy of `messages_v2.py` with both changes):
  digests and run ids are byte-identical for the two fixtures and the example ("DIGEST-SETS-IDENTICAL"). On the
  `f3901a09` policy dict, the schema alone accepts `max_running_jobs` 3 (digest `83d5c114…`) and
  `first_window_rounds` 400 (digest `5e426ac7…`), both of which the pristine schema refuses. That policy has no
  gap cap, so `validate_training` would refuse F = 400 on it; the reader's field bound for the lanes was
  `le=32`, not `Positive`. [V]
- `hash_consequences.py --src <tree>` on both trees: the v1 `example_manifest` is equal, and the hash values are
  those of Part 4 [V].

**For a run that signs them.** Its audit policy digest is new, hence its run id is new. Lanes above 2 also need as
many listed `training.auditors`; with today's two-auditor tooling the training block, and with it the run id,
moves too (2.3). Nothing else in its manifest moves. `implementation_hash` does not depend on the policy; it
moves with the train's code (Part 4). The run needs the T3 server image, as above. [V by reading; probe for the
digests]

## Part 4. The edits

### Owned names (they move `implementation_hash`)

1. **`schedule_audits_v2`**, one expression: `absolute = min(body.d_final, body.d_audit + policy.first_window())`
   (`:8382`).
2. **`lease_v2`**, three points:
   - Read the signed policy once, just before the running count. That moves the read at `:8595-8597` up and
     leaves `lease_rounds = policy.lease_rounds` in its place, so the `expires` line `:8598` is unchanged.
   - `>= 2` becomes `>= policy.max_running_jobs` (`:8551`).
   - `absolute_deadline=min(row["absolute"], now + policy.absolute_rounds())` (`:8611`).

Probe measurement [V], with `hash_consequences.py` (`livrables/forge/2026-10-28_last-merges/bin/`):
- Only the ASTs of these two owned names move. The unowned edits below cannot move the value:
  `implementation_hash` reads only the owned ASTs and twelve paths, none of them `exit_v2.py` or
  `messages_v2.py`, and the probe gives the same value before and after its rearm edit.
- `implementation_hash` becomes `d4694fa9…72b4da` under the s100 pin (`avx2`/`sm_103`/`math`, today `24fbe36d…`)
  and `2d500ffa…ef00c3d` under the example pin (`avx2`/`none`, today `7af135ca…`). `sources()` and the bundle move
  too, as after any `src` edit.
- This is T3 alone, not the train: the train's value is known only when T1 + T2 + T3 are frozen
  (`0017-single-implementation-hash-train.md:77-80`).

### Not owned, hash-neutral (the code lot, unless marked "tooling lot")

- `protocol/messages_v2.py`: `max_running_jobs: Positive`, the field `first_window_rounds`, `first_window()`,
  the three `validate_training` rules and `first_window_rounds_for`.
- `challenge/exit_v2.py`: `Schedule.first_window_rounds`, `absolute` and `audits_end`.
- `challenge/store.py`, unowned: `_retire_round_v2` (`:7215`), the `Schedule` builds of `_reap_round_v2`
  (`:7448-7456`) and `_contest_hold_v2` (`:7680-7688`), and the rearm's `max` in `_rearm_audit_job_v2` (`:9025`).
- `hypertrain/scripts/ops/live_operator.py`: refuse a gap below `first_window_rounds` when the field is set (next
  to `check_open_shape`, `:130-155`, called at `:213-215`).
- Tooling lot: the D20 builder's lanes, first-window and auditor-count knobs, signed only when set, with their
  dry-run checks. The runbook's step 7 (`docs/runbooks/intermediate-model-replay.md:264-284`) pops only
  `reaudit_rounds` and `gap_cap_rounds` today, and asserts that the warm-up has the same training, so the warm-up
  must list the same auditors as the real run.
- Tooling lot: `test_auditors.sh`, N containers.
- The documentation that states the audit policy or the first window:
  - `hypertrain/docs/network-v2.md:752-755,763-764,805-819,830-831`;
  - `hypertrain/docs/operator.md:265`;
  - `hypertrain/docs/prod-run-signing.md:81-87,104-106`;
  - `docs/s100-test-stack.md:410-421,425-444`;
  - `docs/runbooks/intermediate-model-replay.md:26-28,90-92,326-328,333-338,345-346,389-392`, besides its step 7
    (tooling lot).

### Not touched

- `open_round_v2`: no edit. Its literals stay, its 200s stay, and it gains no policy read (Part 6 (a)).
- `absolute_rounds()`: its meaning stays, as finding 3 recommends.
- `_expire_leases_v2`, `_retire_audit_attempt_v2`, the cooldown, the re-audit window, `execute_audit_v2`,
  `complete_audit_v2` and `finalize_v2`.
- `AuditJobV2` and `AuditChallengeV2`.
- `max_attempts`, `max_concurrent_per_auditor` and `max_queued_jobs`.
- The `audit-reservation` record.
- The `AuditInputError` handler: a 4xx in `challenge/app.py`, outside the hash. It is a separate hash-neutral
  item that can ship before the train (finding 3), and T3 neither needs nor replaces it.

## Part 5. Interaction with T1 and T2

- **No shared edited region.** On `dc014508`, T1 edits only `finalize_v2` (`:10872-11042`) [V]. T2 is planned
  in `_service_charge_v2` only (`:5258-5333`; `livrables/forge/2026-10-31_v3-weights-hardening/REPORT.md:257-266`),
  but it is not written, and neither is its publication of staged rounds in a child store (ADR 0017 `:568`; the
  REPORT's caveat, `:264-266`, states only what it must do) [A]. T3 edits the owned `schedule_audits_v2`
  (`:8276-8432`) and `lease_v2` (`:8434-8632`), and the unowned `_retire_round_v2`, `_reap_round_v2`,
  `_contest_hold_v2`, `_rearm_audit_job_v2`, `exit_v2.py` and `messages_v2.py` (Part 4); none of them is in T1's
  or T2's region. With `c9e612b3`'s own commit applied to a copy of `dc014508`, only `finalize_v2`'s AST differs,
  and `_fresh_v2`, `_result_in_lease_v2`, `lease_v2` and `schedule_audits_v2` are identical [V, this session].
- **Couplings, without a shared edit.**
  - `finalize_v2` hands each member's `audit_leases_v2.challenge` to the reward ledger (`:10962-10970`). The
    ledger accepts it at the row's `created` (`:2764-2777`). A later `serve_deadline` therefore changes nothing in
    T1's code [V by reading].
  - The lag tolerance of an audit result is bounded by the lease's `min(expires, absolute)` (`:2651-2660`). Neither
    T1 nor T2 touches it, and T3 changes neither column's meaning for a lease [V by reading].
  - T2 reads `round-resource.deadline` only (`:5295-5297`), which T3 does not write [V by reading].
- **Files where the lots meet.**
  - `hypertrain/tests/ops/s100/test_run_0ef600b4_golden.py:23` and `CHANGELOG.md`: resolve once, on the
    integration branch.
  - `mutants_expiry.py`: T1 edits `:37-44` and inserts entries before `:333`; T3 re-anchors `:372`. They are not
    adjacent.
  - `mutants_gaps_finality.py` (T1 re-anchors `:354`) and `mutants_bounds.py` (T2's charge entries, `:658-783`)
    hold mutants of T3's owned names that still match after T3; T3 edits them only if it adds its new mutants
    there [V, snippet check on the probe].
- **Order.** T1 by cherry-pick of its own commit. The two-tree diff `dc014508..c9e612b3` would revert the 30 `v3`
  commits since its parent `ed5a6481`: seven in unowned `store.py` methods, `bc306dcb` (admission, `app.py`,
  `messages_v2.py`) and `6378ec26` (`escrow_v2.py`), with their tests and docs [V]. Then T2, then T3. The hash is
  a canonical map of per-name ASTs, so it does not depend on the order; it is measured once on the frozen content
  [V by reading].
- **Timing.** At a TEST gap of 400, a wider first window makes an audit that a lane reaches resolve earlier
  (Part 8). A job that resolves only at the window's end resolves later, and its round settles later (2.4).
  `finalize_v2` reads no audit window (none in `:10872-11042`, nor in the aggregate path `:10557-10871`). It
  refuses to settle while audits are unresolved (`challenge/finality_v2.py:68-76`). [V by reading; H for the
  timing]

## Part 6. The literals D159 names, and the gap 200

| Literal | Where (dc014508) | What it does | Classification | Recommendation |
| --- | --- | --- | --- | --- |
| (a) `max_running_jobs` 2 (and `max_attempts` 2, `memory_reservation_bytes` 1 GiB) in `round-resource` | `challenge/store.py:5925-5927` (prepared open, w = 0) and `:6020-6022` (generic open, service block), both in owned `open_round_v2` | No code reads these three keys (`:5295-5297`, `:5672-5687`, `:6000-6014`, `:10484-10487` read other keys; `_rollback_snapshot_v2`, `:11970-11976`, only digests the record's bytes) [V]. It is written only for a service-admitted run, which can neither schedule nor lease an audit (503 at `:8353`, `:8535`, `:9536`, `:9950`) [V]. The `:6021` block is not reachable in normal flow (an admitted run stops at `:5700` or `:5970`, is refused inside the prepared branch, or returns at `:5969`) [V by reading]. A one-line mutant at `:5926` or `:6021` moves the hash [V, mutant] | **Stays outside the train** (not edited; no lot of T1-T3 edits `open_round_v2`) | Leave both writes. The record cannot disagree with a running lease, because no lease runs where it is written. If the service heavy gate is ever lifted, that lot must write the signed value or refuse lanes other than 2 at service admission (open question 6). Writing the signed value now would edit owned `open_round_v2` (444 lines, among the largest owned functions) twice, for a key nobody reads |
| (b) the literal 4: a PROBATION member alive at `clean_count >= 4` | owned `_unfit_cause_v2` `challenge/store.py:8225`; `challenge/trust_v2.py:135-136` (`live`); `challenge/admission.py:1274` (`eligible`) and a fourth copy at `:1631` (`_standing`, bond release). `trust_v2.py:84` is another 4, the audit-sampling ladder (1,000,000 ppm below 4 clean finalizations) | Eligibility, influence, bond release and audit probability [V]. `trust_v2.py` and `admission.py` are not hashed; the mutant `< 4 -> < 5` at `:8225` moves the hash to `451ecfed…` under the s100 pin [V, mutant] | **Stays outside the train** | Change nothing. It is finding 6's family (admission and trust), not lanes, auditors or first window. If the founder chooses to change it, it needs the train for `:8225` and a coordinated edit of the four copies |
| (c) the per-miner Q/4 | owned `open_round_v2` `challenge/store.py:5867` (the prepared service open; ADR 0017 `:419-420` and `:457-458` attribute this refusal to `require_roster_v2`, whose Q/4 is only the `1 << 22` at `:5612`) and `:6058-6059` (every generic open, pinned runs included); owned `require_roster_v2` `:5612` (`weight_units` `1 << 22` in the byte-size tape shell); `WeightedEntryV2.weight_units le=Q//4` (`protocol/messages_v2.py:846`); the cohort cap `WeightAllocationV2` (`:862-863`) and `AggregationPolicyV2.miner_cap_units` / `probation_cap_units` `Literal[4194304]` (`:705-706`); `aggregator/weighted_v2.py:78,85,110` | The ceiling of one miner's weight, and of the PROBATION cohort [V]. Mutants at `:5867`, `:6058` and `:5612` move the hash [V, mutant] | **Stays outside the train** (the per-miner part would need it; the cohort cap would not) | Change nothing. Principal SN100 already put the cohort cap and the 12 outside (D159). Finding 6 recommends no change, and T3 needs neither |
| The service gap 200 | `:5724` (prepared open), `:6017` (service block); `:6025` is the generic `>= 2` | Bounds a service round's `d_final - d_audit` at 200 = `absolute_rounds()` of the tiny profile [V] | **Stays** (not edited) | Keep it. A service run cannot audit, and its first window is `d_final` whatever T3 signs. A service manifest that signs `gap_cap_rounds` above 200 is still capped at 200 there [V by reading] |

None of (a)-(c) is needed by the design, so none goes back to the founder on T3's account. (b) and (c) remain
finding 6's open decision.

## Part 7. Test plan (red test first, for each behaviour)

Each lot starts with a test that fails on `dc014508` for the reason named, then the code. "Fails on `dc014508`"
assumes the fixture change below is applied first: without it, the network fixture's explicit `max_running_jobs=2`
meets the override with a `TypeError` (`hypertrain/tests/challenge/test_service_network_v2.py:686,690`) [V].

The probe ran 15 scratch tests: those of the rows marked "probe", the schema part of the `validate_training` row
with the absent-and-null digest check, and a two-lane control with four auditors (a store-level twin of
`test_audit.py:99`). 11 failed on `dc014508` (`literal_error`, `extra_forbidden`, `TypeError`, or no `first_window`
/ `first_window_rounds_for`) and 4 passed as controls; all 15 passed on the probe. Ten probe mutants were each
killed by the test named (`probe/mutants.txt`) [V]. The probe did not run the builder knobs, the operator check,
`_retire_round_v2`'s gate (edited, but no probe test runs it), the `AUDIT_WINDOW_MISSED` timing, the builder and
service refusals, the `validate_training` rule mutants or the re-anchors: the code lot does.

| Behaviour | Test, and the existing file it joins | Fails on `dc014508` because | Mutant it must kill |
| --- | --- | --- | --- |
| First window relayed by the reapers (Lens m-1): a store-level test that reaps a round through `_reap_round_v2` and holds a contest through `_contest_hold_v2` with F > 2 leases and a signed F, asserting the `Schedule` they build carries `audit.first_window()` | store level, by the reaper, not a hand-built `Schedule` (the "planner follows" test cannot kill them) | `Schedule.absolute` stays at `absolute_rounds()` | Lens mutants M17 and M18 (drop the relay in `_reap_round_v2` / `_contest_hold_v2`) |
| Lanes = 3 with four listed auditors: three leases run, the fourth auditor gets 204 (probe) | store level with `hypertrain/tests/challenge/audit_lease_fixture_v2.py`, with fresh auditor keys (a first draft of the probe reused the fixture's referee and relay seeds, `test_service_network_v2.py:62-63`) | `literal_error`: 3 is not 2 | `lease_v2` keeps a literal `>= 2` (killed) |
| Lanes = 1: the second auditor gets 204 (probe) | same, parametrised | `literal_error` | same (killed) |
| Lanes = 2, as every current policy signs: the third auditor gets 204 | the existing `hypertrain/tests/ledger_invariants/test_audit.py:99` | passes on both (the control) | `aud-lease-store-cap-of-three` (`mutants_audit.py:119`), re-anchored to `>= policy.max_running_jobs`, still killed by `>= 3` |
| Auditors N: lanes above the listed auditors are refused; 32 lanes with 33 auditors pass, 33 lanes do not (probe) | `hypertrain/tests/protocol/test_network_v2.py` next to `:1255-1309` | `literal_error` | the `max_queued_jobs` term dropped (killed in the probe); the auditor term dropped (not among the probe's mutants; a reviewer's rerun killed it) |
| First window signed (F = 300 at gap 400): the row's `absolute` is `d_audit + 300`; a lease at `d_audit` gets `absolute_deadline = d_audit + 200` (no 409); a lease at `d_audit + 150` gets `lease_expires = d_audit + 250` (probe) | store level, `AUDIT_POLICY_EXTRA` as in `test_audit_reaudit_gap_v2.py:67-70` | `extra_forbidden` | `schedule_audits_v2` keeps `absolute_rounds()`; `lease_v2` drops its `min`, so the lease answers 409 (both killed) |
| First window absent at gap 400: a lease at `d_audit + 150` is cut at `d_audit + 200` (probe) | same (control) | passes on both | `first_window()` defaulting to `gap_cap()` (killed) |
| Early exhaustion under F = 400: two executor failures at `d_audit + 5` and `+ 8` rearm the job until `d_audit + 400`; without F, until `d_audit + 208` (probe) | store level, next to the rearm tests of `test_audit_liveness_v2.py:439-535` | `extra_forbidden` (the control passes on both) | the rearm without its `max` (killed) |
| A member without leaves is dropped at the end of the first window: `d_audit + 200` without F, `d_audit + 400` with F = 400 (probe) | store level, next to `challenge/test_no_show_v2.py:128` | `extra_forbidden` (the control passes on both) | `_expire_leases_v2`'s horizon read as anything but the row's `absolute` (`mutants_audit.py:612`, `mutants_exit_roster.py:156` already cover it at 200) |
| First window derived: `first_window_rounds_for(5, 2, 229, 100) = 254`, `(5, 3, 229, 100) = 200`, `(5, 1, 229, 100) = 408`; 408 refused under a gap cap of 400 and accepted under 500 (probe) | `test_network_v2.py`, next to the `reaudit_rounds_for` test (`:1315`) | no such function | the two-lease floor dropped; `ceil` turned into `floor` (both killed) |
| The planner follows: `Schedule.absolute`, `audits_end`, `Wait.AUDIT_WINDOW` and `Wait.DISPUTE` (probe); `AUDIT_WINDOW_MISSED` at `F - 1`; `_retire_round_v2` refusing until `F - 1`; `_contest_hold_v2` counting a leaves contest opened before `d_audit + F` | `hypertrain/tests/challenge/test_exit_plan_v2.py` (its `Schedule` at `:13-15` gains the field); `test_retire_round_v2.py:449` | `TypeError` for the planner; `extra_forbidden` for `_retire_round_v2` | `Schedule.absolute` keeps `2·lease_rounds` (killed); `_retire_round_v2` keeps `absolute_rounds()` |
| Absent fields give today's bytes | the goldens, `test_network_v2.py:1231-1240,1271-1293`, `test_b300_d20_builder.py:701,877`, plus a new "absent, null and set" test in `test_network_v2.py` | the new test fails (`first_window` unknown); the pins pass | `first_window_rounds` always serialised: killed in the probe by four pins, the two `test_network_v2.py` tests and the two manifest goldens, as `scripts/audit_mutants_table.py:408,643` do for the other two optional fields |
| `validate_training` refusals: lanes other than 2 in production; F < `absolute_rounds()`; F > `gap_cap()`; F present in production; the type refusals (0, `True`, 2.5) (probe, schema part) | `test_network_v2.py` next to `:1255-1309`; builder and service refusals in `hypertrain/tests/ops/test_manifest_v2.py:343-437` | each test first accepts a value (1 or 3) that `dc014508` refuses, or calls `first_window()`, which it lacks; the type refusals alone pass on both | each rule dropped, and `Positive` loosened (one mutant each, in `scripts/audit_mutants_table.py`, as for the gap cap at `:652`, `:666`) |
| The operator tool refuses a round opened at a gap below `first_window_rounds` | `hypertrain/tests/ops/test_live_operator.py` next to `:189-245` | the check does not exist | the check dropped |
| Builder knobs: unset gives `1dd433fe…` and the `0ef600b4` policy bytes; set moves only `audit_policy_hash` (and the training, for the auditor count); the first window comes from the helper and a measured cycle and is re-checked by the dry run, as `reaudit_rounds_for` is (`build_test_manifest.sh:493`, `:609`); bad values refused before the first stage | `hypertrain/tests/test_b300_d20_builder.py` next to `:701`, `:754`, `:877` | the knobs do not exist | a knob signed while unset |

- **Re-anchors.** `mutants_expiry.py:372` (`exp-audit-window-drops-d-final`, the `:8382` line) and
  `mutants_audit.py:119` are the only two mutant snippets the probe lost (`test_infra_guards.py:871` failed on
  exactly them) [V]. No mutant covers the rearm line `:9025` today, so the new `max` gets one.
- **Fixture changes.** `test_service_network_v2.py:686-690` takes `max_running_jobs` from `AUDIT_POLICY_EXTRA`,
  as the probe did. `test_exit_plan_v2.py:13-15`, the only test that builds an `exit_v2.Schedule`, gains the
  field.
- **The golden constant** `test_run_0ef600b4_golden.py:23` changes once, on the integration branch.
- **Suites that stay green with the fields absent**, run on the probe [V]:
  - `ledger_invariants/test_audit.py`, `test_audit_2.py`, `test_expiry.py`, `test_gaps_finality.py`,
    `test_exit_roster.py`, `test_exit_roster_2.py`, `test_credit.py` and `test_bounds_2.py` (`-m "not od"`): 218
    passed and 13 xfailed;
  - `challenge/test_audit_*_v2.py`, `test_exit_*_v2.py`, `test_retire_round_v2.py`, `test_no_show_v2.py`,
    `test_service_capacity_v2.py`, `test_service_charge_window_v2.py`, `e2e/test_late_finalize_v2.py` and
    `e2e/test_reaudit_failed_v2_e2e.py`, with seven of the probe's store tests (`-m "not od"`): 346 passed, 1
    skipped and 2 xfailed;
  - the pin set with `challenge/test_service_network_v2.py`, `ops/test_manifest_v2.py` and
    `ops/test_live_operator.py`: 408 passed, and the golden constant failed (Part 3);
  - `test_b300_d20_builder.py`: 103 passed.
- The probe did not run the whole CI suite. The code lot runs it: 8 shards with `-m "not od"` and the slow job
  (`.github/workflows/hypertrain-ci.yml:73-152`).
- CI runs the snippet guards of the two mutant tables but not their runners; the only mutant runner in CI is
  `check_elastic_mutants.py` (`.github/workflows/hypertrain-ci.yml:177`) [V]. The new mutants are run by hand
  with `scripts/check_audit_mutants.py` and `scripts/check_ledger_invariant_mutants.py`. The probe used a scratch
  one-snippet script, which, unlike those runners, does not refuse a named test that fails without the mutant.

## Part 8. Does this fix the observed failures?

The figures come from the cited reports and journals. The arithmetic is [H]. Nothing here is measured with T3.

### The rounds, by gap

| Rounds | What failed | What T3 changes | Conditions |
| --- | --- | --- | --- |
| S17d r0 and r1 (`f3901a09`, gap 175 and 200) | Wave 1 was refused 503 inside beacon pauses ("an outage, not WINDOW"), the retries were cut at `d_final`, and 3 of the 5 jobs were never leased (`livrables/forge/2026-10-28_s17-run-101m/s17d/REPORT-S17D.md:5,42-55`) [A] | At gap 175-200 the first window is already `d_final`, so the field changes nothing. From a first lease 12-38 s after `d_audit` (the gap-400 rounds), with a paired cycle of 190-229 s and 0-8 s between waves: 3 lanes give 2 waves in about 392-504 s, 4 lanes about 383-481 s, and 5 lanes one wave in about 202-267 s, against 525 or 600 s [H]. S17d's first leases came at +66 and +69 s in round 0 (gap 175) and +114 s in round 1 (gap 200) (lease expiry minus 100 rounds; `s17d/evidence/round0-leases.txt`, `round1-attempts.txt`, `round1-open.txt`): at +66 s, 3 lanes give 446-532 s, over the 525 s of gap 175 [H] | ≥ 3 listed auditors, ≥ 3 executor hosts, audits called at `d_audit`, and no beacon-pause 503 beyond the lease tolerance. Cycles with 3 or more concurrent audits are not measured |
| S19 (`f3901a09` r2, gap 200) | One wave, lease to complete, takes 195-200 s; three waves 590-620 s against 600 s; 503s on `/lease` idled the auditors 200 and 273 s; 2 MATCH, 3 `AUDIT_INFRASTRUCTURE` (`livrables/forge/2026-10-29_s19-swap-round2/REPORT-S19.md:56,141,192-194,269-279`) [A] | Same as S17d. Since hotfix 2a, a lease is tolerated up to `RESULT_PAUSE_ROUNDS` (50) more rounds behind on a running run without service admission (`challenge/store.py:2605-2643`; `exit_v2.py:31`), so S19's lag (at most 11) would no longer idle the auditors [H by reading] | as above |
| Gap 400, 4 rounds (`0ef600b4` r0 and r1, `4d84aacc` r0 and r1) | 5/5 MATCH, but only through the rearm. The fifth lease began 439-460 s after `d_audit` by the first watch sample showing it (431-452 s by the sample before), with 140-169 s left to `d_audit + 200`. Lease to complete was 190-229 s for paired audits and 181-206 s for a lone one. The first attempt was cut, with an execute 500 in 3 rounds. MATCH at 793-815 s (finding 3; watch files `settled/s21-evidence/round0-audit-watch.txt`, `settled/s23-round1-evidence/round1-audit-watch.txt`, `S31/evidence/round{0,1}-audit-watch.txt`) [A; the cycles H, sample to sample] | With 3 lanes the fifth audit runs in wave 2 and is done by about 392-504 s, inside the 600 s first window: no cut, no rearm, no 500. With 2 lanes and `first_window_rounds` = 254, the fifth attempt gets a full lease and MATCHes at about 613-666 s (its start plus that round's lone cycle) instead of 793-815 s [H] | 3 lanes need 3 hosts; with 2 hosts the third lane waits for a host with its lease clock running (Part 1.8) |
| 101 M | Never run | **Nothing.** The lease is 100 rounds (300 s) at H = 4 (`messages_v2.py:733-738`; the builder's H = J = 4, `build_test_manifest.sh:32,380`). The 101 M reference replay took 331.4 and 346.2 s from `exec_attempt` to `exec_published` (`livrables/forge/2026-10-28_s17-run-101m/s17b/REPORT-S17B.md:8-9,43`) [A]. At 31 M an audit's executor time, 131-164 s from `exec_attempt` to `exec_published` (the S20 and S31 executor journals of Part 1.8), is 0.96-1.25 times the 31 M reference of 131.9-136.5 s (`livrables/forge/2026-10-28_s17-run-101m/s17c/REPORT-S17C.md:44-50`) [A; the ratio H]. So a 101 M audit attempt would end with its own lease, whatever the lanes or the window [H] | "Back to the founder", item 2 |

### Two corrections to ADR 0017's figures

- **Three waves take longer than its arithmetic says.** ADR 0017's grade line says three waves need 540-620 s
  (`0017-single-implementation-hash-train.md:217`). At gap 400 the fifth lease began between 431 and 460 s (two
  watch samples bracket it) and a lone cycle took 181-206 s, so three waves take 613-666 s, round by round [H,
  from the watches].
- **The time left is a lower bound.** ADR 0017 writes "leaving 161, 140, 152 and 153 s" (`:267-268`). "439, 460,
  448 and 447 s" are the first watch samples that show the fifth lease running, so the time left to `d_audit +
  200` was 161-169, 140-148, 152-160 and 153-161 s [H, from the watch samples].

This record does not edit ADR 0017; the REPORT lists both corrections.

### What a TEST run must measure after the train (not claimed to work before it)

1. Per audit: lease, `exec_attempt`, `exec_published` and complete, for N = 3 and N = 5 lanes on N hosts. Only two
   concurrent replays have ever been measured: paired 190-229 s lease to complete, alone 181-206 s [H, sample to
   sample in the four gap-400 watch files].
2. For each round: the end of the last MATCH against 600 s at gap 200, the number of rearms, the number of
   `AUDIT_INFRASTRUCTURE` attempts and execute 500s.
3. The wait of a lane for an executor host. Run N lanes on N hosts first; lanes > hosts only to measure that wait.
4. s100 load as N grows: download and landing (paired +6 s and +3 s medians against alone; n = 24 and 5, two-host
   data only, computed from the three executor journals in this session [H]), disk reserve per download
   (`gpu_ops/artifact_transfer.py:385-388`), and beacon lag during N concurrent preparations under the store lock
   (`challenge/store.py:9306-9565`).
5. At gap 400 with two lanes, `first_window_rounds` from `first_window_rounds_for` against the same build
   without it.
6. Production keeps 2 lanes and a first window of `absolute_rounds()` (2.5) until these measures exist.

## Part 9. Cost

- **Owned names:** `schedule_audits_v2` and `lease_v2` (Part 4). `open_round_v2` is not edited. The probe measured
  the T3 hash (Part 4); the train's value is measured once, when T1 + T2 + T3 are frozen.
- **Golden constant:** `IMPLEMENTATION_HASH` of `test_run_0ef600b4_golden.py:23`, once, on the integration
  branch. The run ids and policy hashes of `f3901a09`, `0ef600b4`, `4d84aacc` and the example do not move
  (Part 3).
- **Lots:** one T3 code lot (schema, store, planner, the operator tool's gap refusal, the documentation of the
  window, tests and mutants; to estimate) and one hash-neutral tooling lot (the D20 builder's knobs and auditor
  count, `test_auditors.sh`, the runbook's step 7; to estimate), as Part 4 marks them. No file gives a duration
  for either. Then Lens on each, the integration branch, and the image cycle (push to attested images took 29-48
  min in the last three CI runs, `0017-single-implementation-hash-train.md:625-627`) [A].
- **Images:** a server image only. The auditors and the GPU image need nothing for T3 (Part 2.6). A run that signs
  the new fields needs the T3 server image before it is built.
- **Rollback:** the image to return to stays `3bb5db0f…` (ADR 0017 `:641-646`). T3 adds no table and no record
  kind. A run that signs the new fields cannot be served by an older image, and while it has an audit in flight
  it would stop the beacon forwarding to the child stores after it (2.6). Closing every round of every such run,
  on the T3 image (2.6), is a precondition of a rollback [H by reading].
- **What a production ledger keeps:** two lanes, `absolute_rounds()` (200 rounds at H ≤ 30) as the first window,
  and the gap cap of 200, until a TEST run measures the new geometry (2.5, Part 8). TEST runs need ≥ N listed
  auditors, N auditor containers and N executor hosts for N lanes, and D156 holds node loans until F14
  (`0017-single-implementation-hash-train.md:84`) [A].

## Open questions

For Lens:
1. Should the lane count stay store-wide (today's query, which is per run because of the child stores), or become
   explicitly per run (`WHERE run_id=?`)? Recommendation: store-wide. It is the smaller edit, it gives today's
   result in every store, and the root store holds a single legacy run (`challenge/store.py:607`).
2. Should the per-lease `min` be `now + policy.absolute_rounds()` or `now + 2·audit_lease_rounds(H, 16.0)`, the
   structural bound itself? Recommendation: `absolute_rounds()`. It is never above the bound, and it keeps each
   lease as long as a rearm window.
3. Is the `audits_end` change (`d_audit + first_window + 2·lease`) sufficient for every planner wait?
   `AUDIT_LEASE` and `REAUDIT` are read only by tests (`test_exit_plan_v2.py:44-64`). `DISPUTE` is read by
   `_contest_hold_v2` (`store.py:7705`) and moves with it (2.4). `MEMBER_STATE` is `d_final` and does not move.
   Recommendation: yes, with the planner test of Part 7 asserting `DISPUTE`.
4. Should the `audit-reservation` record list all auditors? Recommendation: no. It is unread, and its bytes enter
   the rollback digest of every later round of a pinned run.
5. Should the operator tool refuse, or only warn, when a round is opened at a gap below `first_window_rounds`?
   Recommendation: refuse. F is the time the planned audits need from `d_audit`; a narrower gap ends the first
   window at `d_final`, before F, without a word. A gap of exactly F leaves no rearm either, so the runbook asks
   for F plus the wanted rearm room (146 rounds at F = 254 and a gap of 400).

For Principal SN100:

6. Classification of D159 (a): leave the `round-resource` literals, and require whichever lot lifts the service
   heavy gate to reconcile them. Recommendation: yes (Part 6).
7. Should the lanes and the first window be TEST-only for now (2.5)? Recommendation: yes. It is fail-closed, and
   it can be lifted later without a hash move: in `messages_v2.py` for the lanes, and also in the operator tool's
   gap-cap check for a first window above 200 (2.5).
8. Which measure lifts the TEST-only rule for production lanes? Recommendation: a TEST run at gap 200 with 3 lanes
   on 3 hosts, five members, two rounds, last MATCH under 540 s (60 s of slack before 600 s) and no rearm.
   The thresholds are proposed here; no file holds them.
9. Should the first window go into T3, or only the lanes? Recommendation: both. With three lanes the window is not
   needed at 31 M (2.4), but two hosts is what TEST has had. The window costs two owned expressions
   (`schedule_audits_v2` `:8382` and the per-lease `min` in `lease_v2` `:8611`), the unowned copies of Part 4, and
   no hash beyond the train's. It also delays the rounds whose jobs resolve only at its end (2.4).

## Back to the founder

The design itself needs none of the three triggers: no new protocol constant, no changed default, no policy field
outside lanes / auditors / first window. Two limits are outside T3's scope, and the founder decides both:

1. **Executor capacity.** One executor host runs one audit at a time (`gpu_ops/cuda_executor.py:622`, `:646-662`;
   ADR 0008), and TEST has run on two (Part 1.8). Signed lanes beyond the hosts do not run in parallel: they wait
   for a host while their lease runs [V by reading and a scratch dispatch probe this session]. The options, which
   are resource or infrastructure decisions, not policy fields:
   - (a) For the measuring TEST run, lend at least three executor hosts (D156 holds loans until F14). Production
     auditing would need as many hosts as lanes.
   - (b) A separate design for per-GPU executor slots. `gpu_ops/cuda_executor.py` is not hashed, but the same-host
     slowdown has never been measured, so measure it first.
   - (c) Keep two lanes. T3 then helps only TEST rounds wider than 200 rounds, through the first window.

   Recommendation: (a) for the TEST measurement, (b) studied apart. T3 is useful under all three.
2. **101 M audits.** A 101 M attempt would not fit one 100-round lease [H, Part 8; no 101 M audit has run]. The
   lease is `audit_lease_rounds(H, step bound)`, fixed at 100 for H ≤ 30 (`messages_v2.py:733-738`).
   `validate_training` requires it (`:784-785`) and `AuditJobV2` bounds it (`:451-453`). Changing it is a
   lease-family change: a new field or a new rule for H ≤ 30, with the structural bound. The options:
   - (a) Leave 101 M out of the proof for now. That matches the current bound of "~31 M"
     (`livrables/briefings/2026-11-01_briefing-sn100.md:47`).
   - (b) A measured lease field, in a later train, designed after a first measured 101 M audit.

   Recommendation: (a), and measure one 101 M audit before designing (b).

## Rejected

- **A new lane field next to `max_running_jobs`.** It would duplicate a signed field. Widening the literal changes
  no byte.
- **An auditor-count field, or widening `max_concurrent_per_auditor`.** `training.auditors` already says who
  serves. The CLI holds one lease at a time, and a host runs one job anyway.
- **Deriving the first window from `gap_cap_rounds`.** `0ef600b4` and `4d84aacc` sign a cap of 400. Their later
  rounds would get another first window under an unchanged digest: the D128 objection.
- **Deriving it in the store from the roster and the lanes.** The measured lane cycle is in no policy, and the
  store would change the window of pinned runs.
- **Editing `absolute_rounds()`.** It would change the meaning of every signed policy, as finding 3 says.
- **Widening `AuditJobV2`'s bound.** That is a protocol change. The auditors re-validate the job (Part 1.6), so
  they would need new images.
- **Refusing a lease that cannot finish** ("lease only if `absolute - now` ≥ one cycle"). At gap 400 the fifth
  audit would wait to `d_audit + 200`, then be rearmed. It would MATCH about when it does today (781-831 s against
  793-815 s) [H]: no gain in time, and the rule would change pinned runs.
- **Writing the signed lane count into `round-resource`** (D159 (a), option a1): Part 6.
- **Leaving the rearm as it is** (`min(now + policy.absolute_rounds(), sealed.exp_drand)`, `:9025`). Under a
  signed window, a job exhausted early would be rearmed short of its first window (2.4).

## Not verified

- No round ran with more than two lanes, or with a first window other than `absolute_rounds()`. Every timing in
  Part 8 is arithmetic on two-lane rounds.
- The probe is scratch code. The code lot rewrites it under review, so its hash values hold for the probe only.
- The executor host count of each run comes from the reports. The executor configs (`cuda-executor-s20.json`,
  `-s31.json`) are not in Git.
- 4d84aacc has no fixture in Git. Its policy is taken as `0ef600b4`'s from S31's printed evidence.
- The 101 M audit time is an estimate from the reference replay. No 101 M audit has run.
- `_retire_round_v2`'s gate is edited in the probe, but no probe test runs it. The operator-tool check, the
  builder knobs, the builder and service refusals, the planner's `AUDIT_WINDOW_MISSED` branch
  (`exit_v2.py:284`) and the `validate_training` rule mutants are designed, not probed.
- Nothing ran on s100, a node, a GPU, R2 or on-chain.
