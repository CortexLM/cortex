# ADR 0017: the miner fast path of an admission trial (D94 lot B)

- Status: draft, 2026-10-29 (Principal SN100 D94, lot B). Forge decides promotion to `v3`. Written for branch
  `v3-miner-fast-path` on origin/v3 `708414d8`. It changes `hypertrain/src/hypertrain/miner/core.py` and
  `hypertrain/src/hypertrain/data/store.py`, so `sources()` and the qualification bundle move and the images
  must be rebuilt (see "Hash consequences"); `implementation_hash` does not move. No wire format, protocol
  constant, signed field or byte the miner sends changes. Nothing in this lot ran on a GPU host, on s100, on
  the nodes, against the relay or R2, or on-chain.
- Order: brief `v3-miner-fast-path`. Forge (CTO) settled four points through the relay tool on 2026-10-29:
  `miner/island_launch.py` stays untouched (its two `validate_artifacts` calls are not merged); "an object
  already uploaded is not re-sent" holds within one process only; the uploads of S17b are bound by the relay,
  not by this client; the executor's input push (805 MB over ssh in about 98 s of T_ref) is another lot and is
  only noted here.
- Labels: as ADR 0015. **[V]** verified in code or by a command at `708414d8` plus this lot, **[A]** reported
  by a cited document or session, **[H]** hypothesis or arithmetic, **[?]** unknown.
- Related: ADR 0016 (the S17b measurement and the 250 s rule), 0015 (the 101 M model), 0013 (executor results
  through R2), 0007 (the hash table this one copies), the D67 retry schedule (`docs/network-v2.md`).

## Context

S17b measured one admission trial at 100,676,352 parameters on the s100 TEST stack (ADR 0016, Context) [A]:
challenge to job 29 s, input GETs about 50 s (about 0.8 GB), GPU compute 70 s, post-compute 33 s, 1640 object
POSTs 90 s (about 1.7 GB), `probe-v2` 282.4 s of the 297 s window. The brief's target is a miner side of about
150 s or less at 101 M with nothing signed or sent changed.

At `708414d8` the probe published its four files (`state.safetensors`, `ef.safetensors`, `delta.bin`,
`leaves.json`) one after another, each with up to 16 part PUTs in flight (`UPLOAD_WORKERS`) and a drain at
each file's end before its index [V, reading]. `_fetch_trial` fetched the five trial inputs one after
another, each a single streamed and sha256-checked object [V, reading]. Every input was fetched, including
`start_state`, `ef_in` and `v0`, which the master builds from the manifest alone
(`ChallengeStore._stage_inputs_v2`, `challenge/store.py:3232`) [V, reading].

## Decision

### 1. One upload window for the four files

`data.store._publish_all(sources, put, workers)` publishes every source through one pool: at most `workers`
PUTs run, at most `2 * workers` chunks (32 MiB at 16) are read and not yet stored, and the next file's parts
go out while the last parts of the previous one are in flight. Each file's index is PUT only once every part
it lists is stored, so a reader never sees an index before its parts. The first PUT to fail, in time, stops
the publication: no more chunk is read, queued PUTs are dropped (`_Stopped`), running ones are waited for, and
that first failure is raised, never a later one. `put_files` and `put_file` wrap it; `put_file` with one worker
puts the same bytes in the same order as before [V, reading; `tests/data` passes].

`miner.core._upload_refs` publishes the four files through it with `_Once`: within one publication each
distinct object is POSTed once, and a caller that asks for a digest in flight waits for that POST and gets
its key or its error. Content addressing makes this safe: a digest the master acknowledged holds those bytes.
Nothing is remembered across processes (Forge). `NetworkMiner._round` takes a lock, so the 16 threads sign
their upload requests with one read of the master's round, not one each. Each POST is still `_put_object` with
its own D67 retries; a digest mismatch is never retried; the deadline and cancellation checks are those of
`_bounded_put` [V, reading].

### 2. Trial inputs side by side

`NetworkMiner._get_all(wanted)` runs `_get_whole` for every input in one pool of at most `FETCH_WORKERS` (16)
streams. Each stream is the existing one: identity encoding only, 64 KiB chunks written to disk while
hashed, the object size bound, the sha256 check, retries on transport errors, 5xx and 404 as before. The
first failure in time is raised once every stream has ended; the others stop at their next chunk and are not
retried, so nothing writes into the trial folder once it returns and the folder is deleted as before [V,
tests]. The journal keeps one line per retry; the lines of different inputs may now interleave.

### 3. Genesis inputs built before the challenge

`_fetch_trial` starts `_genesis_inputs` on one thread before it reads the round and asks for the challenge.
The thread builds `start_state`, `ef_in` and `v0` as the master stages them: `init_params` of the manifest's
model, then `reset` packs theta and a zero error feedback, `carry` takes `AnchorCache().genesis(manifest,
hotkey, theta)`. After the job is read and checked, each built input is written only if its sha256 equals
the digest the job pins for it (`objects[name]`, already checked against `job.<name>_sha256`). Any other
outcome falls back to the GET: a build that raises is logged and every input is fetched; a built input whose
digest differs is logged and that one is fetched. Only `samples` and `sample_proofs`, which depend on the
challenge, always cross the wire. At 101 M this removes about 805 MB of GETs (`start_state` and `ef_in` are 4
bytes per parameter each) [V, sizes]. The build took 2.41 s for 805,424,912 bytes at 101 M on the dev host
and raised the peak RSS by 1,937 MiB (theta and two packed copies) [V, one run].

### What moved before the challenge, and what did not

| Work | Moved | Why |
| --- | --- | --- |
| `start_state`, `ef_in`, `v0` | yes, built on a thread while the challenge is asked | a function of the manifest (and hotkey under `carry`); used only when its sha256 is the job's pin, so the bytes trained from are the master's |
| `samples`, `sample_proofs` | no | they depend on the challenge nonce and beacon |
| uploading while computing | no | `published/` appears only by an atomic rename after `validate_artifacts` (`island_launch.py:678`, `:687`), and the refs and commitments the screen signs are computed before any upload; an earlier upload needs `island_launch.py`, which Forge kept untouched |
| the two `validate_artifacts` (about 33 s of post-compute) | no | Forge's decision: `island_launch.py` untouched |
| CUDA context warm-up | no | the context lives in the torchrun worker that `island_launch` starts after the job |
| imports | no | `hypertrain.miner.core` takes 1.4 s and `work_screen`, `island_launch` and `replay` add 0.00 s after it [V, measured]; the CLI already pays it before the challenge |
| the executor's input push (about 98 s of T_ref at 101 M) | no | another lot (Forge); it is the reference side, not the miner side |

## Measurements

`hypertrain/scripts/bench_miner_objects.py` serves the probe's objects and the trial inputs from a stdlib
HTTP/1.1 server on 127.0.0.1 with a fixed latency per object request, and drives the NetworkMiner code that
is imported: once on this branch, once with `PYTHONPATH` on a `git archive` of `708414d8` [V]. Both trees put
the same objects on the wire (equal `posted_sha256` per pair, the multiset of digest and length received).
Dev host, 2026-10-29, one run each:

| Case | `708414d8` | this branch |
| --- | --- | --- |
| 1640 x 1 KiB objects, 20 ms per POST | 101.28 s (one at a time) | 6.42 s (15.8 x) |
| 101 M probe files, 1639 objects, 1,711,611,974 B, 20 ms per POST | 6.78 s | 6.74 s |
| the same, 100 ms per POST | 15.48 s | 14.93 s |
| 5 trial inputs, 806,524,992 B, 20 ms per GET | 0.80 s, peak 1 in flight | 0.48 s, peak 5 |
| the same, 100 ms per GET | 1.22 s | 0.67 s |

Read: the base already ran 16 part PUTs within one file, so a large file gains only at the three file
boundaries and at the small `leaves.json`; many small objects gain by the concurrency. On loopback the client
publishes 1.7 GB in under 7 s; S17b took 64-90 s for the same bytes, so the real path is bound by the relay
(about 18 MB/s at 16 POSTs, Forge) [A], and this lot does not shorten it. The trial input saving at 101 M is
the 805 MB of section 3, not the loopback seconds: S17b fetched the five inputs in about 50 s [A], so most of
that time goes if the GETs ran near the relay's rate [H].

Not established: the miner side at 101 M after this lot. With the relay-bound uploads unchanged and about 40 s
of input GETs gone, it would stay well above 150 s [H]; reaching it needs the upload path or the post-compute
work that Forge kept out of this lot.

## Hash consequences

Measured with `network_qualification.sources()`, `bundle()` and `ChallengeStore._service_implementation_v2`
on a `git archive` of `708414d8` and of this branch's tip, each tree's own code in a child `python -I -B`
(2026-10-29) [V]:

| Value | `708414d8` | this branch |
| --- | --- | --- |
| `implementation_hash` | `7af135ca…70be62` (`avx2`/none); `c509dfd1…83a920` (`avx512`/none); `24fbe36d…c185ce` (`avx2`/`sm_103`/math); `9bcf9bdd…2c2dd5` (`avx2`/`sm_103`/`sdpa-flash`); `c8af2ae7…68891f` (`avx2`/`sm_120`/`sdpa-flash`) | unchanged: neither file is among its twelve, nor among the `challenge/store.py` functions it reads |
| `sources()` (158 files), JCS map | `97e3c592…5f1be3` | `7526f901…3bb769` |
| `sources()`, `json.dumps(sort_keys)` map | `7a721848…b6b4bb` | `1051ce21…2b58e2` |
| `bundle()` (2,775,040 bytes) | `2c064435…0b8196` | `b63d72d2…35045a` |
| `miner/core.py` | `0a5a6f30…3d1618` | `86b534f5…d297aa` |
| `data/store.py` | `e8bcf356…6fb906` | `52f1d2b7…22a2f1` |
| `miner/island_launch.py`, `challenge/store.py` | `8eeae8f4…5ffc32`, `22fb5dcd…c56676` | unchanged |
| `network_qualification.py` | `2206486f…319688` | unchanged |
| example `RunManifestV2` run id | `aa39197d…ec8abd` | unchanged (golden tests pass) |

Consequence: a GPU image built from `708414d8` fails `network_gpu_qualification.py run` against this tree
(`qualification_tree_is_not_the_image`), and every CUDA qualification must be re-run on an image built from
the tree that ships this lot. The server image must be rebuilt for the miner client only; the challenge
service code it runs is unchanged.

## Tests and mutants

All on CPU in default CI, against fake object routes and the in-process service of
`tests/challenge/test_service_network_v2.py`; no torch on the publication and fetch tests, torch on the
genesis and `probe-v2` ones. New file `hypertrain/tests/miner/test_probe_fast_path.py` (18 tests);
`test_probe_trial_v2.py` reads the retries of a missing input per input. Red before each step: 9 of the 11
upload tests at `708414d8`, 4 fetch tests (of 33 run) and the 5 genesis tests before their own step [V].

| Behaviour | Test |
| --- | --- |
| one window for four files, at most `UPLOAD_WORKERS` POSTs, each index after its parts, later files not waiting | `test_the_four_files_share_one_window_and_each_index_follows_its_own_parts` |
| the sequential bytes stored, a shared part POSTed once | `test_the_bytes_stored_are_the_sequential_ones_and_a_shared_part_is_posted_once` |
| queued PUTs dropped after a failure | `test_queued_puts_are_dropped_once_one_put_has_failed` |
| the first failure in time raised, by the reader or at the end | `test_the_failure_raised_is_the_first_in_time_not_the_first_in_file_order` (2 cases) |
| a digest in flight is POSTed once and both callers get its error | `test_a_caller_of_a_digest_in_flight_waits_for_the_one_post_and_gets_its_error` |
| one round read per publication | `test_the_threads_of_a_publication_read_the_master_round_once` |
| bounded memory: reads stay within the window while PUTs are stuck | `test_reads_stay_within_the_window_while_the_puts_are_stuck` |
| at least 5 x faster for many small objects, each POSTed once | `test_many_small_objects_publish_at_least_5x_faster_than_one_by_one` |
| `probe-v2` GETs and POSTs concurrently, each object once, proof accepted | `test_probe_v2_fetches_and_posts_its_objects_concurrently_and_the_proof_is_accepted` |
| inputs side by side within `FETCH_WORKERS`, each read once and verified | `test_trial_inputs_are_fetched_side_by_side_within_fetch_workers` |
| sha256 checked on the parallel path | `test_a_corrupt_trial_input_fails_the_fetch_on_the_parallel_path` |
| the first input failure stops the others and is the one raised, unretried | `test_the_first_trial_input_failure_stops_the_others_and_is_the_one_raised` |
| genesis built locally, only the samples fetched, proof accepted (`reset`, `carry`) | `test_the_genesis_inputs_are_built_locally_and_only_the_samples_are_fetched` |
| a failed or foreign build falls back to the GETs | `test_a_failed_or_foreign_genesis_build_falls_back_to_the_gets` (2 cases) |
| the build runs while the challenge is asked | `test_the_genesis_build_runs_while_the_challenge_is_asked` |

The retry (5xx, transport, bounded 404, 4xx never), signing, refusal, cancellation and cleanup behaviours
keep their existing tests in `test_client_network_v2.py` and `test_probe_trial_v2.py`, which pass unchanged
but for the per-input retry count above.

**21 hand mutants killed** [V], each in its own copy of `hypertrain/` under the scratch directory, run
against `tests/miner` and `tests/data`, a kill counted only on a failed test; the unmutated copy passes the
same set: an unbounded pool, an unbounded read window, a later error raised instead of the first (reader and
end), the index before its parts, no stop after a failure, the end not waiting, the sha256 check skipped on
the upload and on the parallel GET, no dedupe, one publication per file, an unbounded or serial GET, a lost
first GET error, no stop of the other GETs, an unlocked round read, and for the genesis an unchecked pin,
`carry` built as `reset`, no fallback, the build after the challenge and the build unused.

## Rejected

- **One pool per file with a larger window**: still drains at each file's end, and the four files of the
  probe are small in number; the shared window removes the three drains for the same memory bound.
- **A persistent record of uploaded digests across processes**: Forge kept "not re-sent" in-process; a
  master that lost an object would then never get it back from this miner.
- **More than 16 workers**: the S17b uploads are relay-bound; more concurrency moves no byte faster and
  widens the memory window.
- **Building `samples` early or prefetching anything that the challenge decides**: it would need the nonce
  and beacon of a challenge not yet issued.
- **Starting uploads before `validate_artifacts`**: needs `island_launch.py` (Forge: untouched) and would
  publish bytes the screen has not yet bound.

## Not verified

- The miner side of a real trial after this lot: no run on s100, on a GPU host or through the relay. The
  150 s target of the brief is not shown to be met and is likely not met with the uploads unchanged [H].
- That the relay sustains 16 concurrent POSTs as it did within one file: S17b used the same 16 within a file
  [A]; the shared window only removes the drains between files.
- The genesis build on a GPU host or under the image's CPU pin: the bytes match the master's in CI because
  both run the same code on the same host; a miner whose build differs falls back to the GETs and logs it.
- The peak RSS on a miner host at 101 M or at the D94 sizes; measured once on the dev host only.
