# ADR 0012: executor performance: CUDA compute cache, beacon cadence, lock-free remote reference

- Status: draft for branch `v3-executor-perf`, 2026-10-27 (Principal SN100 D40, split in two by D42).
  **Part A** (sections 1 and 3) is implemented on this branch. **Part B** (section 2) is designed here
  and its code waits for `v3-audit-integrity` to be merged. Forge decides promotion to `v3`. No
  protocol constant, manifest field, wire format, production host or pin changes.
- Order: brief `v3-executor-perf`. Evidence: the S13 REPORT (branch `v3-d34-s100-gpu`, 2026-10-27).
- Acceptance: the brief asks that a 13-60 s lock "never causes 503 storms". **Part A does not meet
  that.** It makes the lock safe (no skipped round, no dead pusher) and shortens the lock; the 503
  window after each lock is longer than before until Part B or the pusher change of section 3 lands.
- Labels: **[V]** verified in code or by a command on the date above, **[A]** reported by a cited
  document or session, **[H]** hypothesis or model output, **[?]** unknown.
- This record was reviewed by three independent agents (2026-10-27); their confirmed findings are in it.

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
2. **The remote reference holds a store lock.** `ChallengeStore.trial_reference_v2` takes `self._lock`
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
   `beacon.push.Pusher` pushes rounds oldest first, at most `--catch-up` of them after the highest
   round the service holds (`push.py:53-55,94-108`), one HTTP call at a time with a 10 s timeout
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
finalize and resume moves `seed_beacon`) [V by a reviewer's reading]. That is not the S13 suspension:
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

### 2. The remote reference must not hold the store lock (Part B, designed, not implemented)

Template: `execute_audit_v2` (`store.py:6897-7336`) already does it for audits: claim under the lock
(`_tx`, :6907-7172), run unlocked with a `LeaseGuard` registered in `_lease_guards_v2` (:7148-7149, :7173-7274),
then settle in a second `_tx()` that re-validates (:7275-7334). `trial_reference_v2` follows the same three
phases. The review of this section found the hazards below; the design is not complete until each has a test.

1. **Claim (locked).** Today's gate and reads: `_fresh_v2`, the admission row and its challenge, return the stored
   `reference_commitment` when there is one. Otherwise register a single-flight entry keyed
   `(admission_id, challenge.nonce)`. A second concurrent call for the same key **waits for the first and returns
   the same result** (it does not start a second launch and does not answer a new 409: `cpu_miners.py` treats an
   unknown 409 as a lost window and stops). The waiters hold `asyncio.to_thread` workers (`app.py:898-899`, about
   `min(32, cpu + 4)`): bound them, or retries and timed-out POSTs can exhaust the pool.
2. **Run (unlocked).** `stage_reference`, the launch, `screen_work`, the object puts. The object store is
   file-based and is not the problem; the shared SQLite connection is. These calls read it today, under the
   lock: `_beacon` -> `_beacon_v2` (`admission.py:300,314-316`), `qualified` -> `_record_v2` (:332,
   `store.py:2594`), the `trusted` closure (`store.py:3010`, `admission.store.db`), and `stage_trial_v2`
   (`store.py:2701,2713`). An unlocked read on that connection can see another thread's uncommitted
   `BEGIN IMMEDIATE` rows; `execute_audit_v2` itself calls `_record_v2` unlocked (:7253). Phase 2 therefore
   starts with a locked staging step that reads everything it needs into plain values, then touches no
   database. Staging also races `export_trial_v2` (`store.py:2756-2775`, `GET .../job`): it holds the lock and
   calls the same `stage_trial_v2`, which compares then rewrites `trials-v2/<nonce>/*` (:2732-2736), so a miner
   fetching the job during a reference could read a half-written file and get a spurious 409 "immutable trial
   inputs changed"; single-flight on `(admission_id, nonce)` does not cover that route, so staging writes
   atomically or under its own lock.
3. **Commit (locked, `_tx`).** Re-validate against the state read in phase 1: challenge unchanged, `proof_digest
   is None`, the trial still `OPEN`, the admission still in a state that asks for a reference, no commitment
   yet, and `_fresh_v2` again. Today's commit (`admission.py:346-351`) checks only the challenge and
   `proof_digest`, and `guard.check()` runs after it (`store.py:3033-3034`): phase 3 needs both checks **before**
   the write, which means splitting `prepare_reference` in `admission.py` (a file this ADR now says is edited).
   The flight entry is always released in `finally`. `_fresh_v2` under `_tx` rolls back its own pause and pin
   writes on a 503 (`FreshnessGate` expects the caller to commit them): keep it outside the write.
4. **Other points to settle with the code.** (a) `self._lock` and `escrow.mutex` are one `RLock`
   (`store.py:2584`): a caller that already holds it silently keeps the lock through phase 2, so assert
   `_is_owned()` is false and test it. (b) `push_beacon` iterates `self._lease_guards_v2.values()` with no lock
   (:725) and phase 3's unlocked `finally` pops from it: snapshot it under the lock, as the IPC path does
   (:2669). (c) The lock now freezes `MAX(round)` for a whole reference, so `now` is constant in `screen_work`,
   `reference_beacon` and the window check of `retain_trial_publication` (:526): say which `now` phase 3 uses.
   (d) A child store's guard only gets `advance` through `child.push_beacon` (:753-754).

Required tests (fake slow executor, no GPU), for both layouts of section 2 of the context: (a) while a
reference blocks, `latest_beacon`, `push_beacon` and a different trial's request complete; (b) two concurrent
calls for one trial launch once and return one commitment; (c) a trial that closes or expires during the run gets
no commitment and the admission is not credited twice; (d) a launch that raises releases the flight and a retry
works; (e) a cancelled guard stops the launch; (f) `export_trial_v2` during a reference never reads a partial
file. Part A adds two strict xfails of (a) for the root layout (`latest_beacon`, `push_beacon`); the child layout
needs a fixture with two runs, and (b) and (c) pass on today's code because the lock serialises them, so they are
written with the code.

Alternatives not taken: a lock per trial instead of per store (a wider change to every `with self._lock`
user); answering 202 and polling (an API change the miner tools do not expect); doing nothing and relying on
section 3 (the cadence only limits the damage).

Hash consequences of Part B: `trial_reference_v2` and `Admission.prepare_reference` are not in the `owned` set of
`ChallengeStore._service_implementation_v2` (:4283-4312) nor in its `paths`, so `implementation_hash` does not
move unless the work edits an owned function [V by reading]. `sources()` and the bundle hash move (they hash
every `src/hypertrain/**/*.py`).

### 3. Beacon cadence (Part A, implemented as settings) and the pusher proposal

Part A may not change `challenge/store.py` or the pusher's code, so it ships settings, a model and a report tool.

- `challenge_image.sh beacon` now writes `--interval 0.2 --catch-up 60 --max-failures 100`. It takes the stock
  compose (`3 / 20`) and the cadence the previous version of the script wrote (`0.2 / 3`), which is what the TEST
  stack runs [A]; it raises a failure budget below 100 and keeps a higher one; it refuses, with nothing
  changed, a compose file where the result is not exactly that. Sizing rules, from the model:
  - `--catch-up` must cover every round a lock can hide: about `lock / 3 + 10`. 60 covers 150 s; the old `3`
    skipped rounds after any lock over about 9 s, the stock `20` after one over about 50 s. A challenge
    restart of the same length is covered the same way.
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
- **Model** [H]: a virtual clock around the real, unmodified `Pusher.run`
  (`tests/ops/s100/beacon_model.py`): a lock held for T seconds, both layouts of the context, 10 s client
  timeout, 1.2 s pusher verify, 1.2 s service verify before the lock, a timed-out POST still applied (checked by
  a reviewer with uvicorn and a thread pool), the gate at `now >= emitted - 2`. Eight lock start phases per cell
  (offsets 0, 0.7, 1.4, 2.1, 2.8, 3.5, 5, 7 s into a drand period). Two checks against S13 [V]: a 60 s lock
  gives 5 pusher timeouts ("5 of its 10 allowed failures per 60 s of lock"); `--catch-up 3` after a 13 s lock
  closes the gate for 11-16 s (measured: first answer 20 s after the reference). Run as
  `tests/ops/s100/test_beacon_lock_tolerance.py` against the values the script writes.

| Lock T | `--catch-up 3`, budget 10 (previous): holes avg / max, gate closed after the lock (root) | `--catch-up 60` (shipped): holes, gate closed avg / max (root) | same, run in a child store | head-first proposal (root / child) |
| --- | --- | --- | --- | --- |
| 13 s (cache) | 1.1 / 2, 16 s | 0, 32 s / 39 s | 0, 16 s / 18 s | 0, 2 s / 1 s |
| 19 s (cache) | 3.0 / 4, 18 s | 0, 58 s / 66 s | 0, 37 s / 42 s | 0, 6 s / 5 s |
| 43 s (first run) | 11.0 / 12, 18 s | 0, 160 s / 167 s | 0, 114 s / 120 s | 0, 7 s / 5 s |
| 60 s (no cache) | 16.6 / 17, 19 s | 0, 231 s / 238 s | 0, 175 s / 185 s | 0, 2 s / 1 s |

  So the shipped profile trades speed for safety: never a hole, but the gate stays closed after a lock for up to
  about `4T + 10` seconds (run in the root store; about `3T` in a child). **That figure is very sensitive to the
  cost of one round.** With the shipped cadence and a 19 s lock the gate stays closed for 58 s at 2.4 s a round,
  103 s at 2.6 s, 266 s at 2.8 s, and at 2.9 s the pusher never catches up (a test pins the 2.6 s and 2.9 s
  cases). 2.4 s is one S7 measurement; the idle lag with a child run present has not been measured [?]. The head-first
  column does not depend on it. With the cache (T <= 19 s) the window is about a minute; **without it a 60 s
  reference closes the gate for about four minutes (three in a child), close to the 100-round (300 s) trial
  window.** The cache is therefore a precondition of the shipped profile, and the first (cold, 43-64 s)
  reference of a new image must run alone, before any second driver starts.
- **Proposal, not implemented (needs `beacon/push.py`, outside Part A, and approval because the production twin
  runs the same code):** push the head first, then at most one older pending round per tick, the pending list
  kept in memory (`HeadFirstPusher` in the model). Result in the last column: 0 holes and a gate that reopens
  1-7 s after the lock, whatever the round cost. A naive "newest first, then all the backlog" is worse than
  shipped (the gate closes again during the backfill). A pusher restarted during an outage loses its list;
  the follow-up would add the missing rounds to `GET /v1/beacon/latest` (a `store.py` change) or keep the list
  in a file.
- **Freshness bound:** the bound of 2 rounds is hard-coded in the store (`store.py:2185,2207,2238`). A TEST-only
  larger bound needs a `store.py` edit and is not proposed: it would only hide the lock. Part B removes the cause.

## Hash consequences

Measured with `network_qualification.sources()`/`bundle()` and `ChallengeStore._service_implementation_v2`
over a `git archive` of each commit [V], base `33b50656` and this branch's tip:

| Value | Base `33b50656` | This branch (`src` as of `a4872f92`; later commits are tests and docs) |
| --- | --- | --- |
| `implementation_hash` | `02e208a3…0189cc` | unchanged |
| `store.py` sha256 | `b5bc9904…aaec88` | unchanged |
| `network_qualification.py` sha256 | `dab7a6ab…7bda15` | unchanged |
| `cuda_executor_runner.py` sha256 (the `runner_sha256` of every record) | `a4eaa094…60ca5b` | unchanged |
| `cuda_executor.py` sha256 | `62c9d0df…69bad4` | `9154cdd6…4d8238` |
| `sources()` map: files, canonical sha256 | 151, `e0df98e6…507e8b` | 151, `d3347cce…3451d6` |
| `bundle()` | 2,396,160 bytes, `e78f33d1…eb70ba` | 2,406,400 bytes, `cfcb9d44…503c86` |

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

## Consequences

- A warm reference should take 12-19 s instead of 57-64 s [A, S13, through a wrapper]. S13's trials went from about
  125 s to 70-90 s with one driver and 40-50 s with two, **measured with `--catch-up 3`**: the shipped cadence
  adds the gate-closed time of the table to every reference until Part B or the head-first pusher lands, so
  expect more than 50 s per trial.
- A host gains one persistent directory per image digest; the executor creates it and nothing else; a host that
  cannot make it runs uncached and says so in the journal.
- Within the model, the TEST beacon skips no round in a lock of up to 150 s and the pusher survives 140 s of
  refused connections. The model has a one-round-wide edge at its limit and is not a measurement on the stack;
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

## Sources

`hypertrain/src/hypertrain/gpu_ops/cuda_executor.py`, `beacon/push.py`, `beacon/__main__.py`,
`beacon/drand.py:47`, `challenge/store.py:516-518,707-712,753-754,757-758,2179-2244,2584,2669,2701-2775,
2966-3036,4280-4312,6897-7336`, `challenge/admission.py:197,222-224,236-241,289-365,895,1047`,
`hypertrain/docker/Dockerfile.gpu:28-52`, `scripts/ops/s100_test/challenge_image.sh`,
`docs/s100-test-stack.md` (sections 1 and 6.8), `docs/adr/0008-generic-cuda-executor.md`, the S13 REPORT, all
read 2026-10-27.

## Not verified

No GPU run in this batch: the cache effect (57-64 s to 12-19 s) is S13's measurement on one B300 host through a
wrapper and the mechanism is unknown [?]. No docker daemon was available: the single mount, the refusal of two
mounts on one target and the behaviour under user-namespace remapping are from the fake host and from memory.
A challenge restart while a child store is locked can leave rounds in the root that the child never got; the
pusher reads the root's highest round and cannot refill them, and a seed in one of them is stranded. Reasoned
from the code, not reproduced; `beacon_gaps.py` with the run id on the child store would show it.
The beacon numbers are a model validated on two S13 observations, not a measurement on the s100 stack, and they
depend on a round costing 2.4 s. Part B is a design: its unlocked phase has the hazards listed and is untested.
