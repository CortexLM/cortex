# ADR 0012: executor performance: CUDA compute cache, beacon cadence, lock-free remote reference

- Status: draft for branch `v3-executor-perf`, 2026-10-27 (Principal SN100 D40, split in two by D42).
  **Part A** (sections 1 and 3) was implemented on that branch. **Part B** (section 2) and the head-first
  pusher of section 3 are implemented on branch `v3-executor-lock-release`, 2026-10-28 (D68), on top of
  `v3-audit-integrity`, which Part B waited for; sections 2 and 3 are rewritten for it. Forge decides
  promotion to `v3`. No protocol constant, manifest field, wire format, production host or pin changes.
- Order: brief `v3-executor-perf`, then `v3-executor-lock-release` (D68, from the s15 replay of
  2026-10-27). Evidence: the S13 REPORT (branch `v3-d34-s100-gpu`) and the s15 replay (run `fea7e44f`,
  branch `v3-replay-s100-gpu`).
- Acceptance: the brief asks that a 13-60 s lock "never causes 503 storms". **Part A did not meet that.**
  It made the lock safe (no skipped round, no dead pusher) and shorter, and the 503 window after each lock
  was longer than before. The s15 replay measured it [A]: a cold 57 s reference held the lock, the pusher
  fell more than 2 rounds behind, `POST /join/proof` answered 503 "beacon infrastructure paused" for
  100-150 s per trial, the first trial lost its 100-round window and the 52-trial admission took 2 h 14 min.
  Part B removes the cause: the lock is held for the claim and for the settlement of a reference, not for
  the launch (section 2, measured below). The head-first pusher shortens what any other long lock leaves
  behind (section 3). Neither has run on a GPU host or on s100; the replay with this change is a later run.
- Labels: **[V]** verified in code or by a command on the date above, **[A]** reported by a cited
  document or session, **[H]** hypothesis or model output, **[?]** unknown.
- This record was reviewed by three independent agents (2026-10-27); their confirmed findings are in it. Part B
  and the head-first pusher were reviewed by four independent agents, a skeptic per finding (2026-10-28); what
  they confirmed is in the text, what they did not is not.

## Context

S13 ran the generic CUDA executor (ADR 0008) from the s100 TEST challenge against one B300 host and
completed 28 probation trials (all `MATCH`, bitwise equal across two hosts) but not a round. Measured
there [A, S13 REPORT]:

| Step | Seconds |
| --- | --- |
| remote reference, fresh `--rm` container | 57-64 (n=7) |
| remote reference, CUDA compute cache kept between containers | 12-19, usually 13 (n=21; the first one 43) |
| first answer of `proof-v2` after the reference | 20 (cache) / 65-71 (no cache), then 503 "beacon paused" retries |
| one trial, one driver: no cache / cache | about 125 / 70-90 |
| two drivers with the cache | 40-50 per trial (the drivers only overlap the miner side) |

Three separate costs follow, each with its own cause:

1. **Cold CUDA start.** A bare `torch` start in the image is 6-15 s. The first use of the CUDA kernels in
   every fresh process costs 25-45 s of CPU (fp32 matmul +6.4 s, bf16 matmul +9.6 s, AdamW +4.7 s on
   node A). The second process of the same container needs 0.7-2 s. `--rm` throws away `/root/.nv`
   [A, S13, 4 runs]. Why a driver compute cache helps while the image pins `CUDA_DISABLE_PTX_JIT=1`
   (`hypertrain/docker/Dockerfile.gpu:46`, "only precompiled SASS runs", `hypertrain/docs/protocol.md:136`)
   is **not established** [?]: S13 measured the effect, not the mechanism.
2. **The remote reference held a store lock** (the tree of Part A; section 2 removes it).
   `ChallengeStore.trial_reference_v2` took `self._lock`
   at `challenge/store.py:2972` and keeps it through the gate, the launch (`_role_launch`, :3000),
   `prepare_reference` and the commit (to :3035) [V]. Which store's lock depends on where the run lives
   (`ChallengeStore._children`, :516-518) [V]:
   - **run in the root store** (the first run of a state directory): `latest_beacon` (:757-758) and
     `push_beacon` (:712 `_tx`, after a BLS verify that runs before the lock) need that same lock, so no
     round can land and no other request of the run is served;
   - **run in a child store** (any later run; S13's was one [A]): the child has a lock of its own. The root
     answers `latest_beacon` at once and keeps the round a POST brings, then `push_beacon` hands the round to
     the child (:753-754), which waits. The pusher's POST times out while the root already holds the round,
     and the child, whose table the gate and the seeds read, gets none until the lock ends.

   Either way the pusher fails once per 10 s timeout, and the hosting store gets no round meanwhile.
3. **The pusher cannot catch up fast, and can leave holes.** The gate refuses with 503 "beacon
   infrastructure paused" when `now < emitted - 2` (`store.py:2185`, and :2207 on the test ledger) [V].
   `beacon.push.Pusher` pushed rounds oldest first (the pusher of Part A; section 3 changes it), at most
   `--catch-up` of them after the highest round the service holds (`push.py:53-55,94-108` of that tree), one
   HTTP call at a time with a 10 s timeout
   (`beacon/__main__.py:86`) [V]. A round it skips is never filled: the service reports only its
   highest round, so `plan()` cannot see a gap [V]. Each round costs about 2.4 s on s100 (pusher
   BLS verify 1.2 s, service verify 1.2 s) against a 3 s drand period [A, `docs/s100-test-stack.md`,
   S7: one measurement, taken with one store]. The child's second verify of the same round is a cache
   hit (`drand._pairing_ok` is an `lru_cache`) [V by reading], so a child should not raise the cost [H].

Exact rounds are read, so a hole is a defect, not a cosmetic gap. `Admission.challenge` reads
`_beacon(seed_beacon)` (`admission.py:241`) and the seed is `now + 1` after a join (:197), after a
trial is finalized (:895) and after a resume (:1047) [V]. A seed that falls in a hole answers 500
`BeaconUnavailable: round N not pushed` until the window (100 rounds) is over, then 409
`CHALLENGE_EXPIRED` with the identity still in its state and no way to a new seed (nothing but join,
finalize and resume moves `seed_beacon`; since 2026-10-28 the operator's reseed does too, ADR 0010 point 5) [V by a reviewer's reading]. That is not the S13 suspension:
`SUSPENDED / CHALLENGE_EXPIRED` is written only for a trial already OPEN whose deadline passed
(`admission.py:222-224`), which is what the five-minute backfill pause did to miner 3. The two S13 losses
had two causes, both traced to the beacon. That a hole strands an identity without a second pusher is
**[H]**: the old `--catch-up 3` leaves holes after any lock over about 9 s, but a hole strands only
when a finalize's `now + 1` falls inside it (another driver's lock right after a finalize), and S13
ran 28 locks with the single `--catch-up 3` pusher and reported no stranded seed [A]. Whether
`beacon_gaps.py`'s question was ever asked of that database before 12:04Z is unknown [?].

## Decision

### 1. A persistent compute cache in every executor container (Part A, implemented)

- `ExecutorConfig.compute_cache` (default `true`). When true, each container gets exactly one bind mount:
  `--mount type=bind,src=<host dir>,dst=/root/.nv`. `--mount` is used instead of `-v` because it fails when
  the source is missing, where `-v` would make a root-owned directory on the daemon's say. Nothing else is
  mounted and no flag is added: no `--privileged`, `--cap-add`, `--device`, `-e`, `--pid` or host path.
  `false` restores the ADR 0008 command exactly.
- The host directory is `<root>/<first 32 hex of the manifest image digest>`. The root is
  `$HOME/.cortex-cuda-executor/compute-cache` of the ssh user, or `hosts[].compute_cache_dir` (an absolute path
  of plain characters, for hosts whose home is small; Lium B300 hosts have a large `/mnt/nvme`). One directory
  per image digest keeps a cache made by another image from serving this one and gives a new image a clean
  start; it does **not** make a result independent of the driver or of the host's history.
- The executor creates the directory itself, over ssh, before the container starts, as the ssh user and
  never through `sudo`: `umask 077`, `mkdir -p`, a refusal (with a printed reason) of a link in place of the
  leaf and of a directory the ssh user does not own, then `chmod 0700`. The ownership test matters when the ssh
  user is root, as it often is on rented hosts: root's `chmod` would make another user's directory private and
  leave it writable by its owner. It reads back the path it was given and refuses a report that is not the
  path it asked for. The directory is remembered per host and image and forgotten after a failed container
  run, so a directory removed by hand costs one attempt, not the job. Everything the
  executor writes on the host is this directory and the parents `mkdir -p` creates for it.
- **If the directory cannot be prepared the attempt runs without the cache**, with `compute_cache: false`
  in its provenance and journal entry and an `exec_cache_unavailable` journal line carrying the reason
  (read-only home, odd `$HOME`, a link in the way). A speed-up must not fail a job the host can run (the
  review reproduced three failed attempts for a trailing-slash `$HOME`). A lost host (ssh down, timeout) is
  unchanged: it is skipped, not quarantined.
- The provenance record gets `compute_cache: true|false` and the journal's `exec_attempt` the same field. It
  records that the mount was **requested** for the attempt; it does not say the cache was warm or effective.
- **Rejected: a durable container per host.** `docker exec` into a long-lived container would also keep the
  cache, but the container fixes `--gpus device=...` at creation (the executor picks the idle GPUs per job),
  it is a resident process on a machine the validator cycles, it needs a lifecycle (stale, replaced image,
  orphaned after a challenge restart) and it breaks ADR 0008's "nothing persists on the host" far more than a
  directory does. The mount keeps `--rm`, per-job devices and the existing cleanup.
- **Rejected: a named Docker volume** (`--mount type=volume,src=ht-exec-nv-<digest>,...`): Docker creates it,
  so there would be no ssh call, no `$HOME` or path rules and no link question, and `docker volume rm` removes
  it. It was set aside because the brief asks for a directory the executor creates under its own directory on
  the host, a volume lives in Docker's own area (its disk placement is not the operator's choice per host) and
  `docker system prune --volumes` deletes it without a word. It is the first alternative to try if the bind
  mount misbehaves under a daemon with user-namespace remapping or a rootless setup, which this record has not
  examined [?].
- Determinism: the cache holds binaries the driver built, keyed by the driver for the device [A, NVIDIA
  documentation of `CUDA_CACHE_PATH`, not re-read here]. S13's 28 `MATCH` span references run without the cache
  (the first 7) and with it (21); whether the miner side was cached for the first 7 is not in the report [A].
  That is evidence, not proof [?]: the replay plan below compares `publication_sha256` of a cold and a warm run
  of the same job.
- Disk and ownership: the driver bounds what it writes (`CUDA_CACHE_MAXSIZE`, not set by the executor) [A];
  nothing prunes old image directories; the files belong to root inside a directory owned by the ssh user, so
  the operator removes them with `sudo rm -rf` of the root. On a host where the ssh user is not root-equivalent
  (here it is: `docker` or `sudo -n docker`) a container root could leave setuid files there.
- An old wrapper in `hosts[].docker` that adds `-v ...:/root/.nv` (S13) gives two mounts on one target, which
  docker is expected to refuse [A, from memory, not run]: it must be removed (runbook 6.8, executor docs).

### 2. The remote reference does not hold the store lock (Part B, implemented)

`ChallengeStore.trial_reference_v2` now follows the three phases `execute_audit_v2` has for audits, and
`Admission.prepare_reference` is split to match: `claim_reference`, `run_reference`, `settle_reference`.
`prepare_reference` stays as their composition with one `now`, for every caller that has no lock to
release. The review of the design (2026-10-27) found the hazards below; each has a test. Names, not line
numbers: the line numbers of the Context are those of the Part A tree.

1. **Claim, under `self._lock`.** The gate (`_fresh_v2`, called outside a `_tx`, so its pins and pauses are
   written as before) and the admission row. The flight keyed `(admission_id, challenge.nonce,
   retain_shadow_custody)` is then looked up in `_reference_flights_v2` **first**, from the row's challenge, so
   that a repeated request does not stage the job again under the lock:
   - one: this call is a waiter. It takes one of the `REFERENCE_WAITERS` (2) places of a semaphore the root
     shares with its child stores (one `asyncio.to_thread` pool serves them all), waits at most
     `REFERENCE_WAIT_SECONDS` (330 s; a trial window is 300 s) and returns the leader's commitment **or
     raises the leader's exception** (the same object, as `Future.result` does). Past the budget the call
     answers 503 at once and holds no worker; a waiter that gives up answers 503. `cpu_miners.py` retries a
     503 and stops on an unknown 409, which is why neither case is a 409. A waiter never relaunches: N
     retries of a failing reference would otherwise cost N launches. A request for custody
     (`retain_shadow_custody`, test mode only) has a flight of its own, so one trial can launch twice then and
     the executor answers the second from the first publication;
   - none: this call is the leader. It reads the open trial, the stored commitment (returned as it is when
     there is one), `trial_samples`, `stage_trial_v2`, the challenge receipt and the hotkey of the executor's
     context into plain values (`ReferenceClaim`, and the `trusted` closure), makes a
     `concurrent.futures.Future` and, with a role launch, a `LeaseGuard`, and registers both before it lets
     go of the lock.
2. **Run, without the lock.** `run_reference`: `screen_work` with the launch, then the four `put_large` of
   the artifacts. It reads no database row (a test swaps the connection for one that fails on use). The two
   reads the old code did here moved: the challenge receipt into the claim, and `qualified(screen)` into the
   settlement (it needs the screen, hence the run, and reads `records_v2`; an unqualified reference only leaves
   content-addressed objects behind). The guard is registered in `_lease_guards_v2` as
   `reference:<admission>:<nonce>`, `advance`d by every pushed round, and cancels the launch at the deadline
   round or at the wall cutoff. A reference without a role launch (the CPU path) registers none and cannot be
   cancelled, as before.
3. **Settle, under `self._lock`.** The gate again (`_fresh_v2`, outside a `_tx`, so a 503 keeps its pause): a
   beacon that went stale during the launch must not carry the commitment, whose round and deadline test would
   be stale with it. A 503 loses the finished result, but the retry finds the executor's published directory
   (`Executor.published` for the CUDA executor, the restart rule of `launch_island` on the CPU path) and costs a
   revalidation and the puts, not a launch [V by reading, not run on a host]. A reviewer proposed dropping the
   gate to keep the result; Forge's Lens asked for it, and it is the safe side. Then the guard is checked
   **before** the write, then `settle_reference` runs in one `escrow.tx()`: the result is bound to the
   claimed challenge (proof and screen name the admission, the challenge hash and the nonce; on the production
   path this holds by construction and is kept so that a later change of the run cannot commit another
   challenge's result); the row is what the claim read (challenge, trial epoch, admission state, dispute flag,
   no proof); the trial is still OPEN and `now <= deadline_beacon`; `qualified(screen)`; then the write. A
   stored commitment equal to ours is returned (custody is still retained if it was asked for), a different one
   refused. Whatever else happened (`REFERENCE_STATE_CHANGED`, `REFERENCE_TRIAL_CLOSED`,
   `REFERENCE_RESULT_BINDING`, `UNQUALIFIED_REFERENCE`) nothing is written. The flight and the guard are dropped
   in a `finally`, each by identity, and the waiters are told.
4. **The points the review asked to settle.** (a) `self._lock` and `escrow.mutex` are one `RLock`: a caller that
   already holds it would keep it through the launch, so `trial_reference_v2` raises `RuntimeError` when
   `_is_owned()`. (b) `push_beacon` snapshots `_lease_guards_v2` under the lock, as the IPC thread does.
   (c) `now`: the claim's `now` serves the gate, the guard and `screen_work`'s window check; the
   settlement's own `now` (what its gate returns, `MAX(round)`) is `reference_beacon` (the round the commitment is
   made at, which `proof()` compares with), the window check and `retain_trial_publication`. The lock no longer freezes `MAX(round)`, so they
   differ by the rounds that passed during the launch. (d) A child store's guard gets its rounds through
   `root.push_beacon` then `child.push_beacon`; the tests run both layouts (`child_network` hosts the run in a
   child store). (e) `stage_trial_v2` raced `export_trial_v2` (`GET .../job`): it rewrote the inputs in place, so
   a launch reading `start_state` could read it truncated, and a write that died halfway left a torn file that
   every later call refused as "immutable trial inputs changed". It now writes an input once, in a
   temporary directory beside it, and renames it; an existing file is not touched. (g) `run_reference` puts
   the artifacts without the lock, and `LocalFSStore.put` shared one temporary file per process: two puts of
   one content at once (two references, or a reference and an export) could fail with `FileNotFoundError`
   (found by the review of 2026-10-28, reproduced on the primitive). It writes one temporary per thread now
   (`tests/data/test_store.py`). The earlier text of this section said the object store "is not the problem";
   it was.
   (f) **The window of a slow reference** (Forge's question of 2026-10-28: what happens to an honest miner whose
   reference ends late). The window is the trial's. The lease guard cancels the launch when the verified beacon
   reaches `deadline_beacon`, as the wall-clock timer did before (that round is emitted at the second of the
   guard's wall cutoff); a proof may still arrive in the deadline round, a reference that is not done by then is
   not committed, and a launch the lease cancelled (the guard, or the wall past the cutoff) answers 409
   `REFERENCE_TRIAL_CLOSED` whatever the launch or the screening raised (`IslandFailure`, `WorkScreenError`,
   `AuditInputError`), so that a client does not retry a server error for a closed trial. `settle_reference` adds the backstop `now > deadline_beacon` (refused as
   `REFERENCE_TRIAL_CLOSED`, nothing written); it matters where no guard exists (the CPU path) or in the instant
   between a pushed round and the guard's `advance`, and it accepts the deadline round itself. A reference that
   ends late is therefore dropped, the reaper closes the trial `EXPIRED` as bookkeeping, and the miner's standing
   (state, strikes, dispute flag) is untouched. That is the outcome of the old code, which cancelled at the same
   cutoff; what changed is that the window is no longer eaten by the lock: a 57 s reference costs 19 rounds of
   the 100, not the whole window. After the merge of `v3` (2026-10-28) an identity whose trial lapsed has a way back:
   the operator's `reseed` (`Admission.reseed`, ADR 0010 decision 2 point 5) writes a new seed once the trial is
   closed, and refuses while it is OPEN, so it cannot meet a running reference; without it `challenge()` refuses
   after `seed + challenge_rounds`, so "not stranded" holds for the standing and, with the operator, for the
   next trial. Tests:
   `the_deadline_round_cancels_a_launch_that_is_still_running`, `…ends_one_round_before_the_deadline_is_committed`,
   `…ends_after_the_window_commits_nothing_and_sanctions_nobody` (through `POST /join/proof`), and the
   settlement boundary in `test_reference_settle_v2.py`.

| Hazard | Handled by | Test (`test_reference_lock_v2.py`, both layouts, unless noted) |
| --- | --- | --- |
| the launch holds the lock | claim / run / settle | `latest_beacon`, `push_beacon`, another trial's export, status and reference are served meanwhile; `a_five_second_reference_holds_the_store_lock_for_less_than_a_second` |
| double launch | flight keyed (admission, nonce, custody), looked up before anything is staged | `two_calls_for_one_trial_launch_once…` (one launch, one staging), `…refused_past_the_waiter_budget`, `a_waiter_gives_up…` |
| crash between claim and settle | the claim commits nothing to the admission tables (the gate's pins and the staged inputs stay, and a retry reuses them); flight and guard dropped in `finally` | `a_launch_that_dies_between_claim_and_settle…` (the property held on the parent too; the test reads the new flight table and kills the mutant that keeps it) |
| reaper or expiry during the run | guard cancel, then the settlement's re-validation | `the_beacon_passing_the_deadline…` (executor stops / ignores the event), `a_lease_cancelled_while_the_result_waits_for_the_lock…`; `test_reference_settle_v2.py` for each refusal |
| executor error, then retry | flight dropped, waiters get the failure, a retry launches | `a_failed_launch_fails_its_waiters_alike…` |
| result for a stale claim | row compared at settlement, nonce in the key | `a_result_for_a_stale_claim_is_dropped…`, `test_reference_settle_v2.py` |
| two puts of one content | one temporary file per thread | `two_threads_putting_the_same_content_at_once_both_succeed` (`tests/data/test_store.py`) |
| beacon stale at the settlement | the gate again before the write | `a_beacon_that_went_stale_during_the_launch_commits_nothing_and_the_retry_does` |
| custody and no custody for one trial | the flag is part of the flight key; a second launch finds the first publication | `a_request_for_custody_beside_one_without_has_its_own_flight_and_retains_it` |
| `now` frozen by the lock | settlement's own round | `the_commitment_is_made_at_the_round_of_the_settlement…` |
| the real route | `POST /join/proof` runs the reference through `trial_reference_v2` | `the_proof_route_runs_the_reference_while_the_beacon_goes_on_and_accepts_the_proof` (a beacon round is pushed over HTTP during the launch and answers in under a second; the proof is then accepted, `clean_count` 1) |
| lock already held by the caller; guards read unlocked; inputs rewritten | `_is_owned` check; snapshot under the lock; atomic staging | `a_caller_that_holds_the_store_lock_is_refused`, `a_beacon_push_reads_the_registered_guards…`, `exporting_a_trial_never_rewrites…`, `test_trial_export.py` |

**Measured** [V, 2026-10-28, this host, fake executor that sleeps 5 s and then runs the real reference on the
CPU, 4.5 s; the whole call takes about 11 s]. On the parent a thread asking for the store lock got it after
11.05 s (run in the root store) and 10.31 s (child store), and `push_beacon` returned after 9.57 s and 8.83 s.
With Part B, 537 and 534 requests for the lock, one every 20 ms, never waited as much as a millisecond, the
longest hold taken by the reference was 5.9 ms (root) and 4.1 ms (child) and `push_beacon` returned in
0.7 ms and 1.2 ms (after the merge of `v3` the longest hold by the reference on the fixture was 7 ms in the root
layout). (A scratch script took these numbers and is not in the tree; the tree pins `< 1 s` in
`a_five_second_reference_holds_the_store_lock_for_less_than_a_second`.) This is the lock, not the gate: the pusher
also needs its rounds verified (2.4 s of BLS in all) and the replay must show the gate staying open [?].

**Limits found by the review** (2026-10-28), not removed here:
- The claim stages the job under the lock, and staging is O(model). Measured here [V, 2026-10-28, a timing
  proxy on the store lock, `TrainConfig.from_manifest_v2` patched to a larger model, the call then fails fast
  at the screening so only the claim is timed; a scratch script, not in the tree, run twice at different host
  loads]: the longest hold of the lock by the call was 7 and 13 ms at the 824-parameter fixture, 10 and 133 ms
  at 24.7 k parameters, **2.1 and 5.7 s at 66.3 M (8 layers, d=512), 13.4 s at 124.7 M (8 layers, d=768) and 4.9
  and 6.0 s at 162.4 M (12 layers, d=768)**. A reviewer measured 0.9 s and 3.3 s at the 66 M and 162 M shapes on a
  less loaded host. The time is the CPU initialisation of the parameters and moves with the load of the host.
  So "milliseconds" holds for the admitted tiny shape only. For a model of the 100 M class the claim is the lock
  hold that is left (the launch itself, 57 s cold, is out of the lock either way): longer than the 6 s freshness
  bound of the gate and than the pusher's 10 s timeout at the high end. The fix is to read the database under
  the lock and stage without it, which is not done here. `export_trial_v2` costs the same, and a waiter no
  longer pays it.
- A resume, a suspension or a new challenge does not cancel a stale launch: audit leases revoke their guard
  when they are retired, this guard is cancelled only at the deadline round or the wall cutoff. The stale result
  is dropped at the settlement, but the launch keeps its host until then.
- The gap between registering the flight and entering the `try` is closed against synchronous exceptions only.
  The callers are `asyncio.to_thread` workers, which receive no asynchronous exception (a reviewer reproduced the
  leak by injecting a `KeyboardInterrupt` with a trace hook).

Alternatives not taken: a lock per trial instead of per store (a wider change to every `with self._lock`
user); answering 202 and polling (an API change the miner tools do not expect); doing nothing and relying on
section 3 (the cadence only limits the damage). Rejected inside Part B: `qualified()` called from the run under
a short lock (the run would touch the database after all); a waiter that claims again when the leader fails
(each would relaunch, one after the other); a `threading.Lock` per key instead of the `Future` (no way to hand
the failure to the waiters, and each waiter would still hold a worker without a bound).

Hash consequences of Part B: see "Hash consequences of Part B" below.

### 3. Beacon cadence (Part A: settings; D68: the head-first pusher)

Part A could not change `challenge/store.py` or the pusher's code, so it shipped settings, a model and a report tool. The
pusher change (D68, last bullets) is on top of them; the settings, their sizing and the first four columns of the table
describe the ascending pusher of Part A, which the model keeps as `AscendingPusher`.

- `challenge_image.sh beacon` now writes `--interval 0.2 --catch-up 60 --max-failures 100`. It takes the stock
  compose (`3 / 20`) and the cadence the previous version of the script wrote (`0.2 / 3`), which is what the TEST
  stack runs [A]; it raises a failure budget below 100 and keeps a higher one; it refuses, with nothing
  changed, a compose file where the result is not exactly that. Sizing rules, from the model:
  - `--catch-up` must cover every round a lock can hide: about `lock / 3 + 10`. 60 covers 150 s; the old `3`
    skipped rounds after any lock over about 9 s, the stock `20` after one over about 50 s. A challenge
    restart of the same length is covered the same way. (Head first keeps this cover: see "Coverage" below.)
  - `--max-failures` must outlast the failures of the longest outage: one per 11.4 s for a lock (10 s timeout
    plus the relay fetch), one per 1.4 s for a refused connection. 100 covers 140 s of refusals and over 1,100
    s of lock timeouts. At the budget S13 reported (10) the pusher exits in a lock over about 110 s; the
    sandbox compose file has 5. The pusher is stateless and the compose file restarts it (`restart:
    unless-stopped` in the fixture; the live file is not in this repository [?]), so an exit is noisy, not
    harmful.
  - `--interval 0.2` is kept (the production twin's value; idle lag 1-2 rounds, measured 2026-10-26 [A]).
- `scripts/ops/s100_test/beacon_gaps.py` (read-only, run inside the TEST challenge, takes the run id) lists
  rounds missing from the `beacon` table of the store that hosts the run, the lag and the gate, a dead pusher,
  and any identity in a state that asks for a challenge whose `seed_beacon` is one of the missing rounds.
- **Model** [H]: a virtual clock around the real, unmodified `Pusher.run` (Part A: the ascending pusher, now
  `AscendingPusher`; D68: the production `Pusher`; `tests/ops/s100/beacon_model.py`): a lock held for T seconds, both layouts of the context, 10 s client
  timeout, 1.2 s pusher verify, 1.2 s service verify before the lock, a timed-out POST still applied (checked by
  a reviewer with uvicorn and a thread pool), the gate at `now >= emitted - 2`. Eight lock start phases per cell
  (offsets 0, 0.7, 1.4, 2.1, 2.8, 3.5, 5, 7 s into a drand period). Two checks against S13 [V]: a 60 s lock
  gives 5 pusher timeouts ("5 of its 10 allowed failures per 60 s of lock"); `--catch-up 3` after a 13 s lock
  closes the gate for 11-16 s (measured: first answer 20 s after the reference). Run as
  `tests/ops/s100/test_beacon_lock_tolerance.py` against the values the script writes.

| Lock T | `--catch-up 3`, budget 10 (previous): holes avg / max, gate closed after the lock (root) | `--catch-up 60` (shipped): holes, gate closed avg / max (root) | same, run in a child store | head first (D68, same cadence): holes, gate closed avg / max, root / child |
| --- | --- | --- | --- | --- |
| 13 s (cache) | 1.1 / 2, 16 s | 0, 32 s / 39 s | 0, 16 s / 18 s | 0, 2.0 s / 3.5 s; 0.6 s / 1.3 s |
| 19 s (cache) | 3.0 / 4, 18 s | 0, 58 s / 66 s | 0, 37 s / 42 s | 0, 6.2 s / 6.4 s; 4.7 s / 5.3 s |
| 43 s (first run) | 11.0 / 12, 18 s | 0, 160 s / 167 s | 0, 114 s / 120 s | 0, 6.5 s / 6.6 s; 5.4 s / 5.6 s |
| 60 s (no cache) | 16.6 / 17, 19 s | 0, 231 s / 238 s | 0, 175 s / 185 s | 0, 2.4 s / 3.5 s; 0.6 s / 0.9 s |

  So the ascending pusher with the shipped profile trades speed for safety: never a hole, but the gate stays closed
  after a lock for up to about `4T + 10` seconds (run in the root store; about `3T` in a child). **That figure is very sensitive to the
  cost of one round.** With the shipped cadence and a 19 s lock the gate stays closed for 58 s at 2.4 s a round,
  103 s at 2.6 s, 266 s at 2.8 s, and at 2.9 s the pusher never catches up (a test pins the 2.6 s and 2.9 s
  cases). 2.4 s is one S7 measurement; the idle lag with a child run present has not been measured [?]. The head-first
  column moves with it much less (see "Cost of a round" below). With the cache (T <= 19 s) the window of the ascending pusher is about a minute; **without it a 60 s
  reference closes the gate for about four minutes (three in a child), close to the 100-round (300 s) trial
  window.** The cache was therefore a precondition of the shipped profile, and the first (cold, 43-64 s)
  reference of a new image had to run alone, before any second driver started; with Part B and head first neither
  rule is needed for the gate any more (the replay must show it [?]).
- **Head first, implemented by D68 (`beacon/push.py`; the production twin runs the same code, so the promotion
  carries it).** `Pusher.tick` keeps a sorted list of the rounds still to push (`pending`) and posts the latest
  round first, then one older pending round, the oldest, per tick; with `--once`, which has no next tick, all of
  them. A round leaves the list on a 2xx and never otherwise. `plan()` is unchanged and still bounds what is due
  when a gap is first seen (after the highest round the service holds, at most the last `--catch-up`); what it
  returned stays due until the service has it, so nothing is dropped while the process lives. Verification is
  unchanged: `DrandQuicknet` verifies every round before it is posted, the service verifies it again on ingest.
  The tick's JSON line gains `pending`, the number of rounds still on the list. A naive "newest first, then the
  whole backlog" closes the gate again during the backfill; one older round per tick does not. Last column of the
  table: no hole in any cell and a gate that reopens within 7 s of the lock (model [H], the costs of the other
  columns). An earlier version of this change dropped a pending round when the window slid past it; the review of
  2026-10-28 showed that skips rounds the ascending pusher pushed, silently, and it was removed.
  - **Coverage.** The cut is the plan window, `--catch-up` rounds: 60 rounds are 180 s. Model [H], shipped
    cadence: no hole in a lock of 13 to 150 s in either layout, the gate closed for at most 7.7 s after it; a
    lock of 200 s leaves 5 holes in the root store at `--catch-up 60` and none at 100. So `challenge_image.sh
    beacon` writing 60 keeps its 150 s cover.
  - **Draining takes minutes.** A tick that pushes the head and one older round costs about 5 s while the clock
    adds 1.7 rounds, so the list shrinks by about a third of a round per tick: roughly 15 s per round, 12 minutes
    for the 50 rounds of a 150 s lock [H]. The gate is open meanwhile and the holes are the rounds still on the
    list: `beacon_gaps.py` shows them (exit 3) and they close by themselves; a pusher restarted in that time
    loses the list and the rest stay missing.
  - **Cost of a round.** The gate-closed time after a 19 s lock is 5.3 s at 2.0 s a round, 6.2 s at 2.4 s, 8.7 s
    at 2.6 s; at 2.9 s neither pusher keeps up (head first: 48 s and rounds still on the list at the horizon;
    ascending: 676 s). A long `--interval` lets the list grow for good; head first is meant for the 0.2 s of the
    TEST cadence.
  - **Gate predicate.** The model uses the test ledger's gate (`now >= emitted - 2`, the highest round held).
    The production gate (`FreshnessGate.refresh`) also reads the exact round `emitted - 2`, which head first may
    not have pushed yet, since the rounds between two heads are refilled oldest first and the newest holes last:
    there head first does not shorten the closed gate, and it is not worse than the ascending pusher either [H].
    The stack the replay runs on is the test ledger.
  - **Restart; an unfetchable round.** The oldest missing round, `known + 1` of the moment the lock began, is
    refilled first, and a seed set just before the lock points at it [H]; a round inside the lock is read only by
    a draw that falls in it. An old pending round the relays cannot serve stops the refill, each tick then fails,
    and `--max-failures` ends the process (the ascending pusher stalled on it in the same way). A fix would add
    the missing rounds to `GET /v1/beacon/latest` (a `store.py` change) or keep the list in a file.
- **Freshness bound:** the bound of 2 rounds is hard-coded in the store (`store.py:2185,2207,2238`). A TEST-only
  larger bound needs a `store.py` edit and is not proposed: it would only hide the lock. Part B removes the cause for references.

## Hash consequences

Measured with `network_qualification.sources()`/`bundle()` and `ChallengeStore._service_implementation_v2`
over a `git archive` of each commit [V], base `33b50656` and this branch's tip:

| Value | Base `33b50656` | This branch (tree of `ee8ee5bd`) |
| --- | --- | --- |
| `implementation_hash` | `02e208a3…0189cc` | unchanged |
| `store.py` sha256 | `b5bc9904…aaec88` | unchanged |
| `network_qualification.py` sha256 | `dab7a6ab…7bda15` | unchanged |
| `cuda_executor_runner.py` sha256 (the `runner_sha256` of every record) | `a4eaa094…60ca5b` | unchanged |
| `cuda_executor.py` sha256 | `62c9d0df…69bad4` | `8c4cb625…2a9f4e` |
| `sources()` map: files, canonical sha256 | 151, `e0df98e6…507e8b` | 151, `a4ab9a36…58ce17` |
| `bundle()` | 2,396,160 bytes, `e78f33d1…eb70ba` | 2,406,400 bytes, `9e4c9c0e…60f8bb` |

Reading: `implementation_hash` hashes the `owned` functions of `store.py` and ten named files, none touched, so
it does not move; `cuda_executor_runner.py` is not edited, so `runner_sha256` is unchanged; no run id, golden
root, manifest field or constant moves. `sources()` hashes every `src/hypertrain/**/*.py`, so the executor
module changes the map and the reviewed bundle: older qualification configs no longer validate against a tree
at this commit (`qualification_source_or_profile_changed`) and the dispatch source pins must be recomputed, as
for ADR 0008. **Images:** `docker/Dockerfile.gpu` copies `src` and installs it, so a rebuilt GPU image from
this tree has a new digest and a manifest that pins it has a new run id. Nothing here requires that rebuild:
the executor runs in the server image and the runner is shipped per call, so the existing GPU image works
unchanged [H, not run]. The server image digest changes with any build. `v3-audit-integrity` edits the same
module (EXE-01) and will move the same hashes: the two batches' values are not additive, only the merged
tree's are.

### Part B and the head-first pusher (D68)

Measured the same way (`measure.py` over an export of the tree, 2026-10-28) [V]: `origin/v3` at `4234e8ff`
(the base after the merge) and this branch's tip.

| Value | `v3` `4234e8ff` | This branch |
| --- | --- | --- |
| `implementation_hash` | `fc9e87b0…e4c8fe` | unchanged |
| the 35 owned functions of `store.py` and the ten named files | | none changed |
| `store.py` sha256 | `0ef3d258…93ff42` | `fccf87b7…cff123` |
| `admission.py` sha256 | `d9f158aa…b6b3c7` | `985b2e0a…5cb2ce` |
| `beacon/push.py` sha256 | `1e40246d…dfbb45` | `7153c2a4…1634c9` |
| `network_qualification.py`, `cuda_executor.py`, `cuda_executor_runner.py` sha256 | `dab7a6ab…7bda15`, `f342234d…7db619`, `5930be17…4dff90` | unchanged |
| `sources()` map: files, canonical sha256 | 155, `2443f974…30feea` | 155, `7ca59f58…9f607d` |
| `bundle()` | 2,560,000 bytes, `cd4e5285…5d0ee2` | 2,570,240 bytes, `c94141f0…58a083` |

Reading: the work edits `trial_reference_v2`, two new private methods and two module constants, `push_beacon`,
`stage_trial_v2`, `_open_child` and `__init__` in `store.py`. None of them is among the 35 `owned` functions of
`_service_implementation_v2`, `Admission.prepare_reference` is not owned either, and neither `admission.py`,
`beacon/push.py` nor `data/store.py` (the `put` fix) is one of its ten named files, so `implementation_hash`
does not move: the prediction of Part A holds by measurement, not only by reading. (Its value is `fc9e87b0…`,
not the `02e208a3…` printed above: `v3-admission-hardening`, `v3-audit-integrity` and the later `v3` batches added
owned definitions before this work.) `sources()` and `bundle()` hash every `src/hypertrain/**/*.py`, so they
move, with the consequences described above for qualification configs and dispatch source pins; the server image
digest changes with any build. The GPU image is not read by anything here; a rebuilt one has a new digest, as
before. The ADR text is not hashed.

## Consequences

- A warm reference should take 12-19 s instead of 57-64 s [A, S13, through a wrapper]. S13's trials went from about
  125 s to 70-90 s with one driver and 40-50 s with two, **measured with `--catch-up 3`**, and s15's took 145 s
  with a gate that stayed closed 100-150 s after each reference [A]. With Part B no reference closes the gate
  (the lock is held for milliseconds), so most of that wait should go; how much a trial then takes is for the
  replay to measure [?].
- Several requests share one reference: a repeated `POST /join/proof` waits for the running one (at most two
  waiters in the process, then 503) and gets its commitment or its failure; a launch that failed is launched
  again by the next request, not by the waiters.
- A host gains one persistent directory per image digest; the executor creates it and nothing else; a host that
  cannot make it runs uncached and says so in the journal.
- Within the model, the TEST beacon (head first, `--catch-up 60`) skips no round in a lock of up to 150 s and
  reopens the gate within 8 s of it, but the older rounds take minutes to arrive (about 15 s each). The pusher's
  failure budget of 100 covers 140 s of refused connections by arithmetic (100 x 1.4 s), which the model does not
  exercise. The model has a one-round-wide edge at its limit and is not a measurement on the stack;
  `beacon_gaps.py` can show a hole afterwards, not prove there will be none.
- ADR 0008's "Container" and "Data in and out" rows carry the one exception, the compute cache directory.

## Rejected alternatives

1. A durable container per host; a named Docker volume (see section 1).
2. `CUDA_CACHE_PATH` set by the runner to a path in the container: the cache still has to leave the container.
3. `-v` instead of `--mount`: creates a root-owned directory when the source is missing.
4. One shared cache directory for every image: a stale cache outliving its image is the case the per-digest
   directory removes for free.
5. Baking a warm cache into the GPU image: device- and driver-specific, not reproducible, and a rebuild changes
   the image digest every manifest pins.
6. `--catch-up 3` plus a second pusher (what S13 tried): the second pusher moved `known` past rounds the first
   would have filled. Never run two pushers.
7. A larger freshness bound on the TEST ledger: edits `store.py`, hides the lock instead of removing it.
8. Raising the pusher's HTTP timeout: not reachable from the compose file, and a longer timeout only delays the
   failure count.
9. A paced gap-filler in `scripts/ops/` that pushes only rounds below the highest one held (`beacon_gaps.py`
   already finds them): it cannot move `known`, so it would not repeat S13's mistake, and with it `--catch-up 3`
   would be viable. Deferred: it adds a second writer to the beacon table for an operator to run, and the
   head-first pusher makes it unnecessary.

## Replay plan (nodes A and B, then s100 TEST)

Measure, do not assume. (0) Build and attest the server image that carries this `cuda_executor.py`, swap it on
the TEST challenge (runbook section 1) and take a `beacon_gaps.py` baseline with `$RID` before changing the
cadence. (1) Remove the S13 `dockerwrap` from `cuda-executor.json` (`["sudo","-n","docker"]` again) and run
`cuda_executor_check.py` on A. (2) First reference alone: `exec_attempt` to `exec_published` in the journal
(expect 43-64 s cold), then two more (expect 12-19 s) with `compute_cache` true; look for
`exec_cache_unavailable`. (3) Determinism: the same job staged in two directories, once with `compute_cache:
false` and once warm, compare `publication_sha256` in `role-dispatch.json`. (4) `challenge_image.sh beacon`
(it takes the `0.2 / 3` file the stack has), `beacon_gaps.py` after every ten trials (must print no missing
round and no stranded seed) and the `lag` line, and the idle lag with the child run present. (5) Trials with two
drivers, recording the gate-closed time per reference against the table above. A hole, a stranded seed, an
unexplained `MISMATCH` or an idle lag that does not hold stops the run and is reported, not worked around.
(6) For D68: swap in the server image and the pusher of this change, then one trial with one driver on a cold
image. The reference still takes 43-64 s (`exec_attempt` to `exec_published`), but `POST /join/proof` must not
answer 503 meanwhile (s15: 100-150 s of them per trial), the idle `lag` stays 1-2 rounds during the reference,
`beacon_gaps.py` prints nothing missing, and a second request for the same trial returns the same commitment
without a second `exec_attempt`. A 503 storm, a hole or two `exec_attempt` lines for one trial stops the run.

## Sources

`hypertrain/src/hypertrain/gpu_ops/cuda_executor.py`, `beacon/push.py`, `beacon/__main__.py`,
`beacon/drand.py:47`, `challenge/store.py:516-518,707-712,753-754,757-758,2179-2244,2584,2669,2701-2775,
2966-3036,4280-4312,6897-7336` (the tree of Part A; for Part B read `trial_reference_v2`, `_end_reference_v2`,
`stage_trial_v2`, `push_beacon`), `challenge/admission.py:197,222-224,236-241,289-365,895,1047` (and
`claim_reference`, `run_reference`, `settle_reference`), `tests/challenge/test_reference_lock_v2.py`,
`test_reference_settle_v2.py`, `child_run_fixture_v2.py`, `tests/ops/s100/beacon_model.py`,
`hypertrain/docker/Dockerfile.gpu:28-52`, `scripts/ops/s100_test/challenge_image.sh`,
`docs/s100-test-stack.md` (sections 1 and 6.8), `docs/adr/0008-generic-cuda-executor.md`, the S13 REPORT, all
read 2026-10-27; Part B and D68 read and measured 2026-10-28.

## Not verified

No GPU run in this batch: the cache effect (57-64 s to 12-19 s) is S13's measurement on one B300 host through a
wrapper and the mechanism is unknown [?]. No docker daemon was available: the single mount, the refusal of two
mounts on one target and the behaviour under user-namespace remapping are from the fake host and from memory.
A challenge restart while a child store is locked can leave rounds in the root that the child never got; the
pusher reads the root's highest round and cannot refill them, and a seed in one of them is stranded. Reasoned
from the code, not reproduced; `beacon_gaps.py` with the run id on the child store would show it.
The beacon numbers are a model validated on two S13 observations, not a measurement on the s100 stack, and they
depend on a round costing 2.4 s.
Part B ran only against a fake executor (the real reference on the CPU behind a gate the test controls), in the root
and in a child store: no GPU host, no s100, no real `asyncio.to_thread` pool under load, no second process on the
same database (two challenge processes would each launch once; the settlement then finds the other's commitment,
equal or refused, but that path is only unit-tested). The unlocked reads of `execute_audit_v2` (`_record_v2`
between claim and settle) were found by the review and are not changed here. The measured lock hold is the lock's,
not the gate's. Head first is a model plus unit tests of the real `Pusher`; it was not run against a live relay.
