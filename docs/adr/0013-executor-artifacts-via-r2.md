# ADR 0013: executor results through R2 (presigned URLs), not through ssh

- Status: draft for branch `v3-artifact-r2`, 2026-10-27 (Principal SN100 D41). Forge decides
  promotion to `v3`. Opt-in, off by default; nothing here changes a production run, master, relay,
  profile, pin or emission, and no code of this batch ran on a GPU host or against the real R2
  service (see "Not verified").
- Order: brief `v3-artifact-r2`. Builds on ADR 0003 (R2 data plane: `hypertrain/src/hypertrain/storage/
  {r2,cas,offer}.py`) and ADR 0008 (generic CUDA executor).
- Labels: **[V]** verified in code or by a command on the date above, **[A]** reported by a cited
  document or by a design-review session of this batch (not re-run by the author; those notes are
  not retained), **[H]** hypothesis or arithmetic, **[?]** unknown.
- Code: `hypertrain/src/hypertrain/gpu_ops/artifact_transfer.py` (challenge side),
  `gpu_ops/cuda_executor_runner.py` (container side), a keyword on `Executor._request` and six lines
  of `cuda_executor.load`, and `R2ObjectStore.fetch_to(check=)`. Tests:
  `hypertrain/tests/gpu_ops/test_artifact_{runner,transfer,dispatch,example,r2_live}.py`. Operator
  page: `hypertrain/docs/cuda-executor.md`; example settings:
  `hypertrain/docs/examples/cuda-executor-artifacts.example.json`.

## Context

- **Today the result comes back on the ssh stdout** [V]. `Executor._attempt` runs `docker run`
  through `_remote(stdout=result_tar, max_bytes=max_result_bytes)`; `_read_result` then reads that
  tar with a whitelist (ADR 0008). `hypertrain/docs/cuda-executor.md` already warned that dense-1b
  would move about 60 GB per job.
- **What the result holds** [V by code, A for the measurement]. Per rank: `state.safetensors`
  (theta, m, v: 12P bytes with AdamW), `ef.safetensors` (4P), `delta.bin` (1.0156P), `leaves.json`,
  `summary.json`; `checkpoints/<t>.safetensors` (12P each, `n_leaves = H/J + 1`) under rank-0 only
  (`miner/island_launch.py:418-558`, `trainer/config.py`). A real 2-rank CPU island with P = 52.8 M
  gave 70.03 P of rank bytes, equal to the formula below [A]. Size:
  `S = (17.016 g + 12 (H/J + 1)) P` for `g` ranks.
- **Windows** [V]. A trial reference: 297 s from the seed round (`admission.py:256`,
  `store.py:2987-2990`), after the miner's proof, under the store lock (`store.py:2972`), and the
  in-tree miner client times that POST out at 120 s [A]. An audit lease: `audit_lease_rounds`
  (`protocol/messages_v2.py:666-673`) is 100 rounds = 300 s for `H <= 30`, else
  `ceil(2 H 4.0 / 3) + 100` rounds: 549 s at H = 31, 813 s at H = 64, 4,398 s at H = 512. A lease is
  not renewed.
- **ssh is RTT-bound** [A]. OpenSSH's channel window is 2 MiB (`rcvd adjust 2097152`); through an
  emulated delay one stream carried about 2 MiB / RTT in both directions: 105 MiB/s at 10 ms,
  30 MiB/s at 50 ms, 10 MiB/s at 150 ms. The job inputs (`8P` with `state_policy = reset`, `16P` with
  carry) travel the same way, on stdin.
- **Measured elsewhere** [V]. S13 (`v3-d34-s100-gpu`): a remote reference takes 57-64 s with a fresh
  `--rm` container and 12-19 s with a kept compute cache (tiny-cpu, 824 parameters). ADR 0003:
  18.8 MB/s up and 36.7 MB/s down to R2 from one host (64 MiB, 8 parts, 8 workers); nothing measured
  from a GPU host.
- **The challenge keeps a CPU path after the bytes arrive** [A, measured on one EPYC 9575F with
  SHA-NI]: `fetch_to` (block and whole sha256, 0.85 GiB/s), `_read_result` (1.4 GiB/s), `_file_sha`
  and `_publication_sha` (1.9 GiB/s each), and `validate_artifacts` (0.5 GiB/s, 125-133 ns per
  parameter) three times on a reference and four on an audit. That is **8.9 s per GiB** of result
  (reference) or **10.9 s per GiB** (audit, referee), on one thread. The s100 CPU is unmeasured [?].
- **Protocol ceiling, independent of this ADR** [V]. A state object above
  `MAX_PARTS * PART_BYTES = 8192 * 1 MiB = 8 GiB` is refused by the part writer `_publish`
  (`data/store.py:338`, reached through `put_file` and `artifact_ref`; constants at
  `protocol/messages_v2.py:57-59`), on the miner side (`miner/core.py:1285`) and the reference side
  (`gpu_ops/work_screen.py:122`). With AdamW `state.safetensors` is 12P, so **P <= 715.8 M**. The D20
  dense-1b manifest (889.3 M, default `opt = adamw`) has a 10.67 GB state and cannot complete
  whatever the transport. With Muon on the attention and MLP matrices the state is about 8P and 1 B
  just fits [H]. Changing it is a wire change (maintainer approval); not touched.

## Decision

1. **Opt-in.** `HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS` names a settings file. `cuda_executor.load` then
   builds `ArtifactExecutor` (new module, subclass of `Executor`); without it nothing changes. The
   settings list the `operations` served (default `audit`, `referee`: the reference holds the store
   lock for the whole launch, and a 60 GB transfer under it would re-create the beacon 503 storms
   of S13 until executor-perf Part B lands) and must keep `max_artifact_bytes` at or below the
   executor's `max_result_bytes`, because `_read_result` still bounds the downloaded tar by the
   latter (raise both or neither; the executor refuses to load otherwise). There is no
   `ExecutorConfig` field: `v3-executor-perf` and `v3-audit-integrity` edit that class, `_attempt`
   and the record literal now.
2. **The artifact is the tar the container streams today**: same members, same order, same
   headers; `_read_result`, `_check_environment` and `_publish` are untouched. Its identity is the
   sha256 of those bytes. The container serves it as an offset-addressed layout (`TarInfo.tobuf`
   headers, file bytes by `pread`), so no second copy is written; the layout is tested equal to
   `write_outputs` output and its hashes equal to `storage.cas.scan`.
3. **The host declares, the challenge answers, the host uploads.** A presigned PUT can only be bound
   to a sha256 known before the URL is made, and the tar exists after training. So, in R2:
   - the challenge presigns `declare.json` (PUT, signed `if-none-match: *`) and `offer.json` (GET)
     and puts them in `request.json["artifact"]` (stdin of the container, never on its disk);
   - the container (preflight GET of the offer first, 10 s per request, exit 11 if the store is
     unreachable) builds the layout, hashes it on threads (per-file sha256 for `COMPLETE`, then the
     whole-object chain beside the per-4 MiB-block and per-part digests) and PUTs `declare.json`
     `{version, request_sha256, size, sha256, part_bytes, blocks[], parts[]}`;
   - a thread of the challenge polls `declare.json` by HEAD (a store that fails is waited out for
     `store_blip_seconds`, 30 s: the island runs for minutes and nothing can be declared before it
     ends), reads it **once**, by range, after a size gate (cap `4096 + 72 * (blocks + parts)` of the
     configured maximum), parses it strictly (exact keys, integers that are not booleans, lowercase
     hex, list lengths that follow from the size, the part size the **challenge** asked for, this
     attempt's `request_sha256`, whole hash different from the block-list key), calls `offer_upload`
     (block-list sidecar, `CreateMultipartUpload`, one UploadPart URL per part whose signed
     `x-amz-content-sha256` is the declared part hash, ListParts pages) and writes `offer.json`
     echoing `sha256(declare bytes)`; a declaration it will not answer gets `{"refused": <fixed
     word>}` and creates nothing;
   - the container uploads the parts R2 does not hold yet (threads, bounded memory, backoff on
     429/5xx/transport, 403/400/404 final, URL lifetime tracked on its own clock), re-hashing each part
     before it leaves, and prints one status line;
   - after ssh returns, the challenge completes the upload (`finish_upload`: every planned part held at
     its size, `If-None-Match`), checks the object's `HEAD` against the declaration, sets
     `2.2 S + 1 GiB` of disk aside (against the other downloads in flight) and fetches with
     `R2ObjectStore.fetch_to` (ranged, parallel, every 4 MiB block against the sidecar, then the whole
     sha256, atomic replace; `check=` stops it on cancel or deadline). The result is only accepted if
     that sha256 equals the declared one **and** the existing reference/audit comparison passes.
   Why R2 as the control channel and not the ssh pipes: the production ssh transport and the verbs
   docker is asked for stay as they are, and the container's only side channel besides ssh is the
   pair of URLs it was given.
4. **Keys are per attempt**: `exec/<run>/<job digest>/<nonce 32 hex>/{declare.json, offer.json,
   r/<sha256>, r/<block-list key>}`, a top-level prefix so that the bucket rules in
   "Consequences" are literal prefixes and cannot reach the data plane of ADR 0003 (`runs/`,
   `datasets/`). The nonce is random, journaled (`exec_artifact_begin`) before any URL exists, and
   never reused. **Cross-attempt resume is not delivered, and cannot be with one tar**:
   `summary.json` holds `elapsed_ns`, `environment.json` the host's UUIDs, and `COMPLETE` binds the
   request, which carries a clock-based budget; the tar of one job differs from run to run, even on
   one host [A]. Adopting an earlier attempt's open upload would also let a lying host's metadata and
   still-valid URLs reach an honest host's upload. Per-file content-addressed objects would give real
   resume (see "Not built").
5. **The host holds URLs, never a credential.** The R2 token is a regular file, owner = the
   challenge's uid, mode 0600, holding only the five R2 names (never `CLOUDFLARE_API_TOKEN`), read
   by `load_credentials` with the process environment ignored (also by its HTTP clients:
   `trust_env=False`); the bucket must start `cortex-v3-` and the endpoint be https (plain http only
   to a literal loopback address, for the tests). The mailbox URLs live `time left to the job deadline
   + margin` seconds (at most 7 days, they are used after the island ran); part and list URLs are made
   after it and live the same but at most `url_ttl_cap_seconds`. The offer announces the container a
   lifetime shorter by what has passed, a poll interval and 5 s. URLs appear in no log, journal,
   exception, record or `repr` (tests build a harvest of every signature and search for it; the
   harvest finds them in `request.json`, the one place they travel). The container accepts https, or
   http to a literal loopback address, on **one origin**, without user info or fragment, through
   `http.client` (no redirect, no proxy variable), and prints a class name or a status, never a URL.
6. **Time.** The island's budget is `budget_seconds - reserve_seconds` so the transfer has time left
   (default reserve 60 s); a lease with less than `reserve + 30 s` left does not use R2 at all
   (`exec_artifact_skipped`, the attempt runs the ssh way). Every wait in the container is bounded
   (offer wait, deadline, URL lifetime); a failure of the challenge's thread sets an event that stops
   the ssh call at once and is reported as the attempt's failure, not as a deadline.
7. **Provenance.** `role-dispatch.json` gains `artifact: {transport, sha256, size, part_bytes, parts,
   upload_ms, download_ms}` (strings and integers: `envelope_v2.load_json` refuses floats);
   `result_tar_sha256` is the declared and verified hash. Journal: one record per attempt event, not
   per part.

## Failure model

| Event | Class | Action |
| --- | --- | --- |
| R2 unreachable at the start of an attempt (one 5 s probe) | fall back | `exec_artifact_unavailable`; the attempt runs the ssh way (`fallback = classic`, default) or fails (`fail`) |
| Operation not in `operations` | classic | no R2 call |
| Lease shorter than `reserve + 30 s` | classic | `exec_artifact_skipped` |
| Host cannot reach the store (runner preflight, exit 11) | host busy | not counted, host skipped for this sweep |
| Declaration refused (size, shape, wrong request, lease over), exit 10 | attempt failed | counted; nothing created in R2 beyond the mailbox |
| Store fails during the upload (exit 10), URL expired (403), status line absent or wrong | attempt failed | counted; open upload aborted, object, block list and mailbox deleted |
| Store down for longer than `store_blip_seconds` while waiting | attempt failed | cancels ssh at once; container removed |
| Host lost (ssh 255) after every planned part is stored | salvaged | complete, fetch, verify: the hashes vouch for the bytes, not the exit status |
| Host lost mid-upload, or the store cannot say what it holds | host lost, not counted | upload aborted, nothing published; the next attempt starts over with new keys |
| Container exits 0 with a part missing or short | attempt failed | `finish_upload` refuses |
| Uploaded bytes differ from the declared whole hash or block list | host refused | quarantined as in ADR 0008; object deleted |
| Parts differ from the hashes the host declared | none reaches R2 | R2 answers 400 (signed `x-amz-content-sha256`) |
| Storage error on the challenge side after ssh succeeded | attempt failed | one translation boundary; text is a class name |
| Not enough free disk for `2.2 S + 1 GiB` | attempt failed | before the first range is requested |
| Stdout longer than 1 MiB in R2 mode | attempt failed | the cap is `SMALL`, not `max_result_bytes` |

## Windows: which sizes fit which windows

Serial model, per job, for an audit lease: `T = start + compute + hash + input + upload + download
+ cpu + 13 s`. hash = `S / 2.3 GB/s` in the container [V, this host]; input = `I / 31 MB/s` over ssh
at 50 ms RTT [A, the measured 30 MiB/s]; cpu = 10.9 s per GiB of `S` [A]; 13 s is the challenge-side
fixed part [A]. R2 throughput is a scenario axis, up/down in MB/s: **A** 18.8/36.7 [V, ADR 0003, one
non-GPU host]; **B** 100/88 [A, host to s100 in joinbase-6, not R2]; **C** 500/500 [?]; **D**
1250/1250 [?]. Per profile (P, g, H/J, inputs, start + compute, lease):

| Profile | S | Inputs | Start + compute | Lease | A | B | C | D |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| kimi-tiny (77.0 M, 1, 8/4) | 4.08 GB | reset, 0.62 GB | 70-90 s [H] | 300 s | 475-495 | **233-253** | **162-182** | **153-173** |
| od-base TEST (123.8 M, 2, 64/16) | 11.64 GB | carry, 1.98 GB | 64 s [A, joinbase-6 reference] | 813 s | 1200 | **513** | **311** | **283** |
| dense 0.35 B (350.3 M, 1, 8/4) | 18.57 GB | reset, 2.80 GB | 109-140 s [H] | 300 s | 1903-1934 | 806-837 | 484-515 | 439-470 |
| od-large (353.9 M, 8, 512/64) | 86.4 GB | carry, 5.66 GB | 572-2108 s [A/H] | 4398 s | 8633-10169 | 3529-5065 | **2029-3565** | **1822-3358** |
| dense-1b D20 (889.3 M, 2, 8/4) | 62.3 GB | reset, 7.11 GB | 124-204 s, one-GPU round of a qualification tool, executor start not included [V S3, ?] | 300 s | 6037-6117 | 2357-2437 | 1276-1356 | 1127-1207 |

Totals in seconds [H, arithmetic]; bold: fits its lease. tiny-cpu (824 parameters) moves tens of kB.
Reading:

- Only a **kimi-tiny-sized** job (<= about 4 GB) fits a 300 s lease, and only at about 100 MB/s each
  way or better [?]. An **od-base-sized** job (<= about 12 GB) fits an H = 64 lease (813 s) at 100 MB/s;
  the 297 s trial reference would need 500 MB/s and the proof POST decoupled from the 120 s client
  timeout. 18.6 GB (0.35 B) fits no 300 s lease in the table. These are the first non-toy steps:
  **kimi-tiny for 300 s leases, od-base for H = 64 leases**; the 100-300 M band the brief names is,
  today, 77-125 M.
- **dense-1b D20 does not fit at any rate in the table, and cannot complete at all** (protocol
  ceiling above). The 60 GB figure of `hypertrain/docs/cuda-executor.md` is the same order (62.3 GB at
  H = 8, g = 2; 105 GB at the plan shape H = 30, J = 5).
- Past about 200 MB/s each way the **challenge CPU path (10.9 s/GiB) is larger than upload plus
  download**: at C and D it is 42-118 s of the total for the first two profiles. R2 alone does not
  widen the windows beyond that point; fewer validations of the same bytes and a threaded `fetch_to`
  are the next lever.
- The input leg stays on ssh and is 20 s (kimi-tiny) to 229 s (dense-1b) at 31 MB/s; at 150 ms RTT
  (about 11 MB/s) od-base's 1.98 GB alone takes about 180 s. That is why inputs by R2 GET are the next
  lot.
- Peak disk on the challenge is about `2S` (the file `fetch_to` writes and the copy `_read_result`
  stages); `result.tar` is unlinked before `_publish`, which then adds the inputs. RAM for the input
  tar is the inputs. `max_artifact_bytes` defaults to 16 GiB and must be raised per profile together
  with `max_result_bytes` (at most 10,000 parts: 168 GB with 16 MiB parts). `declare.json` is about
  2 MB at 103 GB and `offer.json` 4-8 MB at 6-10 k parts; the container reads at most 32 MiB.

## Consequences

- A result of several GB is designed to reach the challenge over many HTTPS streams instead of one
  ssh stream, verified block by block, with the R2 credential never leaving the challenge. This has
  run against an in-memory R2 only.
- Cost [H from ADR 0003 tariffs]: a 62 GB job at 16 MiB parts is about 3.7 k UploadPart, about 3.7 k
  range GETs and about 12 other Class A operations, plus one Class B HEAD per `poll_ms` while the
  island runs (about 600 per 300 s lease): well under 0.1 USD per job; about 270 such jobs stay
  inside the monthly free Class A tier.
- **Prerequisites for the TEST bucket (Forge)**: a bucket `cortex-v3-test-*`; a bucket-scoped
  `Object Read & Write` token stored 0600 outside the repository and never displayed; lifecycle rules
  "abort incomplete uploads after 1 day" and "expire after 1 day" on the literal prefix `exec/` (the
  code deletes its own uploads, object, block list and mailbox, but a host that still holds a URL can
  write a mailbox object again, up to 5 GiB, until the URL expires); a ledger line; at most 50 USD.
  The code does not install a rule.
- **Hash consequences** (measured with `network_qualification.sources()` / `bundle()` and
  `ChallengeStore._service_implementation_v2`, before = `c7ab9b1d` (origin/v3 with ADR 0012), after = this branch's tip, 2026-10-27):

| Value | Before | After |
| --- | --- | --- |
| `implementation_hash` | 02e208a3…0189cc | unchanged |
| `store.py` sha256 | 71005ecd…2eab3c | unchanged |
| `network_qualification.py` sha256 | dab7a6ab…7bda15 | unchanged |
| `sources()` map, canonical sha256 | ca84b7cb…837ad9 (151 files) | f7cd6dae…c69f18 (152 files) |
| `bundle-hash` | 2,406,400 bytes, e809c519…1d6b1a | 2,457,600 bytes, 4b7ef59e…73014a |
| `cuda_executor.py` sha256 | 8c4cb625…2a9f4e | fdefab16…50bed5 |
| `cuda_executor_runner.py` sha256 (`runner_sha256`) | a4eaa094…60ca5b | 5930be17…4dff90 |

  The new module and the edits to `cuda_executor.py`, the runner and `storage/cas.py` move the
  `sources()` map and the reviewed bundle, as any source change does; `implementation_hash`
  (`challenge/store.py` owned names and ten listed paths, none under `gpu_ops` or `storage`),
  `store.py`, `network_qualification.py`, run ids, goldens and protocol messages are unchanged. Older
  qualification configs stop validating against a tree at this commit (`qualification_source_or_profile_changed`),
  as after ADRs 0007 and 0008, and the dispatch source pins must be recomputed once after the last
  `src` commit of the integration. `runner_sha256` of the provenance changes. ADR 0012
  (`docs/adr/0012-executor-performance.md`) states that the runner is not edited and `runner_sha256`
  is unchanged: that holds for 0012 alone, and not once both land.
- Residual risk: with `network = default` (required) a compromised island could reach the declare,
  offer and part URLs of this one job until they expire. The island is image-pinned and parses
  safetensors without pickle; the URLs are kept out of its environment, argv, work directory and log.
- Size of the change: about 1,200 lines outside tests, over the 400-line guideline, because the
  container side must ship as one `-c` string (the runner) and the challenge side is one module. If a
  split is wanted: PR 1 = `fetch_to(check=)`, the runner section and its tests; PR 2 =
  `artifact_transfer`, the `load()` hook, the dispatch tests, ADR and docs.

## Not built

- **Inputs through R2** (a presigned GET per object, the container verifying each against the
  request's hashes): the input leg above. The challenge would upload each content-addressed input once.
- **Per-file content-addressed objects** instead of one tar: the large files (state, ef, delta,
  leaves, checkpoints) are identical across ranks and across attempts (checkpoint H equals the
  state), so one object per distinct file removes 31-54 % of the bytes [A: od-base 11.64 to 8.04 GB,
  od-large 86.4 to 40.0 GB], lets the challenge fetch while the host still uploads, and makes a lost
  host resumable across attempts. It needs a manifest reader next to `_read_result` (which another
  lot is editing), so it is a follow-up.
- A production bucket, token and lifecycle rule; a live run; the reference operation (needs the store
  lock released); fewer validations; falling back to the ssh way when every host reports exit 11; the
  8 GiB object ceiling (wire change).

## Rejected alternatives

- **Adopting an open upload across attempts**: impossible (the tar differs), and unsafe (metadata and
  URLs of another host's declaration).
- **An `ExecutorConfig` field and a `launch` wrapper**: textual conflicts with live edits of
  `v3-executor-perf` and `v3-audit-integrity`; reproduced with `git merge-file`.
- **A duplex exchange over the ssh pipes** (offer on stdin after the tar): framing after a tar stream,
  a new transport method and a rewrite of the shared fake host.
- **A coordinator HTTP route for hosts**: a new authenticated surface on the challenge.
- **A second copy of the tar on the host's disk**: twice the disk at 60 GB.
- **`urllib` in the container**: proxy variables, redirects and URL text in its errors.
- **MD5 part ETags for resume**: dropped with adoption; part sizes are checked like `send_offer` does
  and the whole sha256 is the authority.

## Not verified

- No GPU host, no real ssh/docker and no real R2: the tests use the fake host and the in-memory R2 of
  `hypertrain/tests/storage/fake_r2.py`; they are not evidence of live behaviour. **The largest real
  transfer in the tests is about 128 MiB** (memory-bound test of the runner); the end-to-end tests
  move 17-25 MiB; the 2.1, 11.6 and 103 GB sizes were exercised only as synthetic declarations
  (part counts, document sizes, pagination). The multi-GB round trip the brief asks for is deferred
  to the live test.
- The opt-in live test (`hypertrain/tests/gpu_ops/test_artifact_r2_live.py`, marker `cloudflare`;
  run with `HT_R2_EXEC_TOKEN_FILE=<0600 env file> HT_R2_TEST_BUCKET=cortex-v3-test-<name>
  [HT_R2_ARTIFACT_MIB=1024 HT_R2_ARTIFACT_EVIDENCE=<file outside the repository>] uv run --frozen
  pytest -s -m cloudflare tests/gpu_ops/test_artifact_r2_live.py`) was written and **not run**: this
  session has no bucket-scoped token. It probes, none of which ADR 0003 probed: a signed
  `if-none-match` on a presigned single PUT answering 412 the second time; a presigned GET of an
  absent key answering 404 (the runner's preflight and polling rely on it); UploadPart on an aborted
  upload answering 404; the status of an expired URL; overwriting a part number; and then the
  throughput of a real artifact. Still [?] after it: thousands of parts to one key at 16-32 workers
  (the 1 write/s rule), completing a 6,000-part upload within the client timeout, a signed
  `content-length`, and how R2 answers a request whose URL expired before it read the body.
- R2 throughput from a GPU host (scenarios A-D above), the s100 CPU rate, the container image's CA
  bundle for `ssl` (the GPU image installs no `ca-certificates` itself [?]), SHA-NI on the hosts.

## Sources (read 2026-10-27)

- `docs/adr/0003-cloudflare-data-plane.md`, `docs/adr/0008-generic-cuda-executor.md`,
  `hypertrain/docs/cuda-executor.md`; S13 report `v3-d34-s100-gpu--82f4b9a8/REPORT.md`; the S3
  qualification evidence (`livrables/forge/2026-10-26_qualif-b300/`, dense-1b one-GPU rounds
  124.1-203.9 s) and the joinbase-6 report (`livrables/forge/2026-10-20_deploy-hypertrain-joinbase-6.md`:
  reference 63.4-63.9 s; host to s100 785-913 Mbit/s).
- Code: `hypertrain/src/hypertrain/` `gpu_ops/{cuda_executor,cuda_executor_runner,work_screen}.py`,
  `storage/{r2,cas,offer}.py`, `data/store.py:150-192,338`, `miner/island_launch.py`,
  `protocol/messages_v2.py:57-59,666-673`, `challenge/{store.py:2972,2987,4249-4312,7338;
  admission.py:256}`.
- Design review of this batch (six read-only lenses before the code, five after, 2026-10-27): the
  ssh window, the size formula, the CPU rates, the tar-hash observation and the 31-54 % figure are
  [A] from it; the notes are not retained.
