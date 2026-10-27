# ADR 0012: executor performance: CUDA compute cache, beacon cadence, lock-free remote reference

- Status: draft for branch `v3-executor-perf`, 2026-10-27 (Principal SN100 D40, split in two by D42).
  **Part A** (sections 1 and 3) is implemented on this branch. **Part B** (section 2) is designed here
  and its code waits for `v3-audit-integrity` to be merged. Forge decides promotion to `v3`. No
  protocol constant, manifest field, wire format, production host or pin changes.
- Order: brief `v3-executor-perf`. Evidence: the S13 REPORT (branch `v3-d34-s100-gpu`, 2026-10-27).
- Labels: **[V]** verified in code or by a command on the date above, **[A]** reported by a cited
  document or session, **[H]** hypothesis or model output, **[?]** unknown.

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
| two drivers with the cache | 40-50 per trial |

Three separate costs follow, each with its own cause:

1. **Cold CUDA start.** A bare `torch` start in the image is 6-15 s. The first use of the CUDA kernels in
   every fresh process costs 25-45 s of CPU (fp32 matmul +6.4 s, bf16 matmul +9.6 s, AdamW +4.7 s on
   node A). The second process of the same container needs 0.7-2 s. `--rm` throws away `/root/.nv`
   [A, S13, 4 runs]. Why a driver compute cache helps while the image pins `CUDA_DISABLE_PTX_JIT=1`
   (`docker/Dockerfile.gpu:46`, "only precompiled SASS runs", `docs/protocol.md:136`) is **not established**
   [?]: S13 measured the effect, not the mechanism.
2. **The remote reference holds the store lock.** `ChallengeStore.trial_reference_v2` takes `self._lock`
   at `challenge/store.py:2972` and keeps it through the gate, the launch (`_role_launch`, :3000),
   `prepare_reference` and the commit (to :3035) [V]. `latest_beacon` (:757-758) and `push_beacon`
   (:712 `_tx`, after a BLS verify that runs before the lock) need the same lock [V]. So while a
   reference runs, no beacon round can land, and no other trial request is served.
3. **The pusher cannot catch up fast, and can leave holes.** The gate refuses with 503 "beacon
   infrastructure paused" when `now < emitted - 2` (`store.py:2185`, and :2207 on the test ledger) [V].
   `beacon.push.Pusher` pushes rounds oldest first, at most `--catch-up` of them after the highest
   round the service holds (`push.py:53-55,94-108`), one HTTP call at a time with a 10 s timeout
   (`beacon/__main__.py:86`) [V]. A round it skips is never filled: the service reports only its
   highest round, so `plan()` cannot see a gap [V]. Each round costs about 2.4 s on s100 (pusher
   BLS verify 1.2 s, service verify 1.2 s) against a 3 s drand period [A, `docs/s100-test-stack.md`].

Exact rounds are read, so a hole is a defect, not a cosmetic gap. `Admission.challenge` reads
`_beacon(seed_beacon)` (`admission.py:241`) and the seed is `now + 1` after a join (:197), after a
trial is finalized (:895) and after a resume (:1047) [V]. A seed that falls in a hole answers 500
`BeaconUnavailable: round N not pushed` until the identity's window passes and it is suspended
`CHALLENGE_EXPIRED`. This is the S13 incident (two identities lost). It needs no second pusher:
`challenge_image.sh beacon` set `--catch-up 3`, which skips rounds after any lock longer than about 9 s.

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
  per image digest keeps a cache made by another image from ever serving this one, so a result depends on the
  pinned image only. The cost is one cold start per new image digest, which a new image pays anyway.
- The executor creates the directory itself, over ssh, before the container starts: `mkdir -p` then `chmod 0700`
  (the call fails if the directory belongs to another user). It reads back the path it was given and refuses a
  report that is not the path it asked for. The directory is remembered per host and forgotten after any failed
  attempt, so a directory removed by hand costs one attempt, not the job. Nothing outside that directory (and
  the parents `mkdir -p` creates) is written on the host.
- The provenance record gets `compute_cache: true|false` and the journal's `exec_attempt` gets the same
  field, so a later question of the form "did a persistent cache serve this result" has an answer.
- **Decided against a durable container per host.** `docker exec` into a long-lived container would also keep
  the cache, but the container fixes `--gpus device=...` at creation (the executor picks the idle GPUs per job),
  it is a resident process on a machine the validator cycles, it needs a lifecycle (stale, replaced image,
  orphaned after a challenge restart) and it breaks ADR 0008's "nothing persists on the host" far more than a
  directory does. The mount keeps `--rm`, per-job devices and the existing cleanup.
- Determinism: the cache holds binaries the driver built, keyed by the driver for the device [A, NVIDIA
  documentation of `CUDA_CACHE_PATH`, not re-read here]. S13's 28 `MATCH` span references run without the cache
  (the first 7) and with it (21), against a miner that also ran cached [A]. That is evidence, not proof [?]: the
  replay plan below compares `publication_sha256` of a cold and a warm run of the same job.
- Disk: the driver bounds the cache (`CUDA_CACHE_MAXSIZE`, not set by the executor) [A]; a directory per image
  digest is removed by the operator with `sudo rm -rf` of the root (the files belong to root in the container).

### 2. The remote reference must not hold the store lock (Part B, designed, not implemented)

Template: `execute_audit_v2` (`store.py:7110-7330`) already does it for audits: claim under the lock, run
unlocked with a `LeaseGuard` registered in `_lease_guards_v2`, then settle in a second `_tx()` that re-validates.
`trial_reference_v2` follows the same three phases.

1. **Claim (locked).** Today's gate and reads: `_fresh_v2`, the admission row and its challenge, return the stored
   `reference_commitment` when there is one. Otherwise register a single-flight entry keyed
   `(admission_id, challenge.nonce)`. A second concurrent call for the same key **waits for the first and returns
   the same result** (it does not start a second launch and does not answer a new 409: `cpu_miners.py` treats an
   unknown 409 as a lost window and stops).
2. **Run (unlocked).** `stage_reference`, the launch, `screen_work`, the object puts. Nothing in this phase may
   use `self._db` or `admission.store.db` unlocked. Which of these touch the shared SQLite connection (the object
   store, `stage_reference`) must be confirmed when the code is written, and any that does takes the lock for
   that call only [?]. The `LeaseGuard` (`deadline_beacon`, `cutoff`) stays: `push_beacon` calls `guard.advance`,
   so a reference that outlives its window is cancelled, now while the lock is free.
3. **Commit (locked, `_tx`).** Re-validate against the state read in phase 1: challenge unchanged (the existing
   `REFERENCE_STATE_CHANGED` test at `admission.py:343`), `proof_digest is None`, the trial still `OPEN`, no
   commitment yet, and `_fresh_v2` again. If the identity was suspended or the trial closed or expired meanwhile,
   nothing is written and the call fails; the flight entry is always released in `finally`.

Required tests (fake slow executor, no GPU): (a) while a reference blocks, `latest_beacon`/`push_beacon` and a
different trial's request complete; (b) two concurrent calls for one trial launch once and return one
commitment; (c) a trial that closes or expires during the run gets no commitment; (d) a launch that raises
releases the flight and a retry works; (e) a cancelled guard stops the launch. (a) is added in Part A as a strict
`xfail` that fails on today's code.

Hash consequences of Part B: `trial_reference_v2` and `Admission.prepare_reference` are not in the `owned` set of
`ChallengeStore._service_implementation_v2` (:4283-4312) nor in its `paths`, so `implementation_hash` does not
move unless the work edits an owned function [V by reading]. `sources()` and the bundle hash move (they hash
every `src/hypertrain/**/*.py`).

### 3. Beacon cadence (Part A, implemented as settings) and the pusher proposal

Part A may not change `challenge/store.py` or the pusher's code, so it ships settings and evidence.

- `challenge_image.sh beacon` now sets `--interval 0.2 --catch-up 40 --max-failures 30` (was `0.2`, `3`, and
  the compose file's own `--max-failures`). Rules behind the numbers, from the model below:
  - `--catch-up` must cover every round a lock can hide: `ceil(T_max / 3) + 4`. 40 covers a lock of 120 s,
    and a challenge restart of the same length. Below that the pusher skips rounds: the old `3` leaves holes
    after a 13 s lock.
  - `--max-failures` must outlast the lock at one failure per 10 s timeout plus the relay fetch (about 11.4 s):
    `ceil(T_max / 11.4) + 5`. 30 survives a 300 s lock (the S13 backfill lock was about 5 minutes). At the
    compose file's 5 the pusher exits during a lock over about 50 s, at the default 10 over about 110 s.
  - `--interval 0.2` is kept (the production twin's value; idle lag 1-2 rounds, measured 2026-10-26 [A]).
- `scripts/ops/s100_test/beacon_gaps.py` (read-only, run inside the TEST challenge) lists rounds missing from the
  `beacon` table, the lag, and any open identity whose `seed_beacon` is one of them. It is what the replay uses
  to prove "no hole" instead of inferring it.
- **Model** [H], a virtual clock around the real, unmodified `Pusher.run`: a service whose lock is held for T
  seconds and blocks both `GET /v1/beacon/latest` and `POST /v1/admin/beacon`, 10 s client timeout, 1.2 s
  pusher verify, 1.2 s service verify before the lock, the gate at `now >= emitted - 2`. Eight lock start
  phases per cell. Two checks against S13 [V]: a 60 s lock gives 5 pusher timeouts ("5 of its 10 allowed failures
  per 60 s of lock"); `--catch-up 3` after a 13 s lock closes the gate for about 18 s (measured: first answer 20 s
  after the reference). The same model runs as a test (`tests/ops/s100/test_beacon_lock_tolerance.py`).

| Lock T | `--catch-up 3` (old): holes avg / max, gate closed after the lock | `--catch-up 40` (shipped): holes, gate closed after the lock avg / max | head-first proposal: holes, gate closed |
| --- | --- | --- | --- |
| 13 s (cache) | 0.9 / 2, 18 s | 0, 30 s / 40 s | 0, 2 s |
| 19 s (cache) | 2.9 / 4, 19 s | 0, 57 s / 67 s | 0, 6 s |
| 43 s (first run) | 10.9 / 12, 19 s | 0, 159 s / 168 s | 0, 7 s |
| 60 s (no cache) | 16.5 / 17, 19 s | 0, 230 s / 239 s | 0, 3 s |

  So the shipped profile trades speed for safety: never a hole, but the gate stays closed for up to about `4T + 10`
  seconds after a lock. With the cache (T <= 19 s) that is about a minute; **without it a 60 s reference
  closes the gate for four minutes, close to the 100-round (300 s) trial window.** The cache is therefore a
  precondition of the shipped profile, and the first (cold, 43-64 s) reference must run alone, before any
  second driver starts.
- **Proposal, not implemented (needs `beacon/push.py`, outside Part A, and approval because the production
  twin runs the same code):** push the head first, then at most one older pending round per tick, the
  pending list kept in memory. In the model above that gives 0 holes and a gate that reopens 2-7 s after the
  lock. A naive "newest first, then all the backlog" is worse than shipped (the gate closes again during the
  backfill, 11-166 s). A pusher restarted during an outage loses its list; the follow-up would add the missing
  rounds to `GET /v1/beacon/latest` (a `store.py` change) or keep the list in a file.
- **Freshness bound:** the bound of 2 rounds is hard-coded in the store (`store.py:2185,2207,2238`). A TEST-only
  larger bound needs a `store.py` edit and is not proposed: it would only hide the lock. Part B removes the cause.

## Hash consequences

To be measured on the final tree (`network_qualification.sources()`/`bundle()` and
`ChallengeStore._service_implementation_v2`, same tool as ADR 0008), see the end of this file. Expected, from the
files touched: `gpu_ops/cuda_executor.py` changes, so the `sources()` map and the reviewed bundle hash change;
`implementation_hash` does not (it hashes the `owned` functions of `store.py` and ten named files, none touched);
`cuda_executor_runner.py` is not edited, so `runner_sha256` is unchanged; no run id, golden root, manifest field or
constant moves. Older qualification configs no longer validate against a tree at this commit
(`qualification_source_or_profile_changed`) and the dispatch source pins must be recomputed, as for ADR 0008.
`v3-audit-integrity` edits the same module (EXE-01) and will move the same hashes: the two batches' values are
not additive, only the merged tree's are.

## Consequences

- A warm reference should take 12-19 s instead of 57-64 s, and trials 40-50 s instead of about 125 s [A].
- A host gains one persistent directory per image digest; the executor creates it and nothing else.
- The TEST beacon never skips a round in a lock of up to 120 s, never exits in one of up to 300 s, and the
  operator has a command that proves it. The price is a longer 503 window after each lock until Part B or the
  head-first pusher lands.
- ADR 0008 is amended on two lines: the container "No volume, no bind mount" row and the "Data in and out" row now
  read "except the compute cache directory".

## Rejected alternatives

1. A durable container per host (see section 1).
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

## Replay plan (nodes A and B, then s100 TEST)

Measure, do not assume: (1) `cuda_executor_check.py` on A; (2) first reference alone, record `exec_attempt` to
`exec_published` in the journal (expect 43-64 s cold), then two more (expect 12-19 s) with `compute_cache` true;
(3) determinism: the same job staged in two directories, once with `compute_cache: false` and once warm, compare
`publication_sha256` in `role-dispatch.json`; (4) `challenge_image.sh beacon`, `beacon_gaps.py` after every ten
trials (must print no missing round and no stranded seed) and the `lag` line; (5) trials with two drivers,
recording the gate-closed time per reference against the table above. A hole, a stranded seed or an unexplained
`MISMATCH` stops the run and is reported, not worked around.

## Sources

`hypertrain/src/hypertrain/gpu_ops/cuda_executor.py`, `beacon/push.py`, `beacon/__main__.py`,
`challenge/store.py:707-712,757-758,2179-2244,2966-3036,4280-4312,7110-7330`, `challenge/admission.py:197,236-241,
288-362,895,1047`, `docker/Dockerfile.gpu:44-52`, `scripts/ops/s100_test/challenge_image.sh`,
`docs/s100-test-stack.md` (sections 1 and 6.8), `docs/adr/0008-generic-cuda-executor.md`, the S13 REPORT, all
read 2026-10-27.

## Not verified

No GPU run in this batch: the cache effect (57-64 s to 12-19 s) is S13's measurement on one B300 host and the
mechanism is unknown [?]. The beacon numbers are a model validated on two S13 observations, not a measurement on
the s100 stack. Part B is a design: the unlocked phase's use of the shared SQLite connection is unconfirmed.
