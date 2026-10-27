# ADR 0013: executor results through R2 (presigned URLs), not through ssh

- Status: draft for branch `v3-artifact-r2`, 2026-10-27 (Principal SN100 D41). Forge decides
  promotion to `v3`. Opt-in, off by default; nothing here changes a production run, master, relay,
  profile, pin or emission, and no code of this batch ran on a GPU host or against the real R2
  service (see "Not verified").
- Order: brief `v3-artifact-r2`. Builds on ADR 0003 (R2 data plane: `storage/{r2,cas,offer}.py`) and
  ADR 0008 (generic CUDA executor).
- Labels: **[V]** verified in code or by a command on the date above, **[A]** reported by a cited
  document or by a design-review session of this batch (not re-run by the author), **[H]**
  hypothesis or arithmetic, **[?]** unknown.
- Code: `hypertrain/src/hypertrain/gpu_ops/artifact_transfer.py` (challenge side),
  `gpu_ops/cuda_executor_runner.py` (container side), a keyword on `Executor._request` and six lines
  of `cuda_executor.load`, and `R2ObjectStore.fetch_to(check=)`. Tests:
  `hypertrain/tests/gpu_ops/test_artifact_{runner,transfer,dispatch}.py`.

## Context

- **Today the result comes back on the ssh stdout** [V]. `Executor._attempt` runs `docker run`
  through `_remote(stdout=result_tar, max_bytes=max_result_bytes)`; `_read_result` then reads that
  tar with a whitelist (ADR 0008). `docs/cuda-executor.md` already warned that dense-1b would move
  about 60 GB per job.
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
  and `_publication_sha` (1.9 GiB/s each), and `validate_artifacts` three times (0.5 GiB/s each,
  125-133 ns per parameter). That is about **8.9 s per GB** of result, on one thread. The s100 CPU
  is unmeasured [?].
- **Protocol ceiling, independent of this ADR** [V]. A state object above
  `MAX_PARTS * PART_BYTES = 8192 * 1 MiB = 8 GiB` is refused by `put_large`
  (`data/store.py:338`, `protocol/messages_v2.py:56-58`), on the miner side (`miner/core.py:1285`)
  and the reference side (`gpu_ops/work_screen.py:122`). With AdamW `state.safetensors` is 12P, so
  **P <= 715.8 M**. The D20 dense-1b manifest (889.3 M, default `opt = adamw`) has a 10.67 GB state and
  cannot complete whatever the transport. With Muon on the attention and MLP matrices the state
  is about 8P and 1 B just fits [H]. Changing it is a wire change (maintainer approval); not touched.

## Decision

1. **Opt-in.** `HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS` names a settings file
   (`docs/examples/cuda-executor-artifacts.example.json`). `cuda_executor.load` then builds
   `ArtifactExecutor` (new module, subclass of `Executor`); without it nothing changes. The
   settings list the `operations` served (default `audit`, `referee`: the reference holds the store
   lock for the whole launch, and a 60 GB transfer under it would re-create the beacon 503 storms
   of S13 until executor-perf Part B lands). There is no `ExecutorConfig` field: `v3-executor-perf`
   and `v3-audit-integrity` edit that class, `_attempt` and the record literal now.
2. **The artifact is the tar the container streams today**: same members, same order, same
   headers; `_read_result`, `_check_environment` and `_publish` are untouched. Its identity is the
   sha256 of those bytes. The container serves it as an offset-addressed layout (`TarInfo.tobuf`
   headers, file bytes by `pread`), so no second copy is written; the layout is tested equal to
   `write_outputs` output and its hashes equal to `storage.cas.scan`.
3. **The host declares, the challenge answers, the host uploads.** A presigned PUT can only be bound
   to a sha256 known before the URL is made, and the tar exists after training. So, in R2:
   - the challenge presigns `declare.json` (PUT, signed `if-none-match: *`) and `offer.json` (GET)
     and puts them in `request.json["artifact"]` (stdin of the container, never on its disk);
   - the container (preflight GET of the offer first, exit 11 if the store is unreachable) builds the
     layout, hashes it on threads (per-file sha256 for `COMPLETE`, then the whole-object chain beside
     the per-4 MiB-block and per-part digests) and PUTs `declare.json`
     `{version, request_sha256, size, sha256, part_bytes, blocks[], parts[]}`;
   - a thread of the challenge polls `declare.json` by HEAD, reads it **once**, by range, after a size
     gate (cap `4096 + 72 * (blocks + parts)` of the configured maximum), parses it strictly (exact
     keys, integers that are not booleans, lowercase hex, list lengths that follow from the size, the
     part size the **challenge** asked for, this attempt's `request_sha256`, whole hash different from
     the block-list key), calls `offer_upload` (block-list sidecar, `CreateMultipartUpload`, one
     UploadPart URL per part whose signed `x-amz-content-sha256` is the declared part hash, ListParts
     pages) and writes `offer.json` echoing `sha256(declare bytes)`; a declaration it will not answer
     gets `{"refused": <fixed word>}` and creates nothing;
   - the container uploads the parts R2 does not hold yet (threads, bounded memory, backoff on
     429/5xx/transport, 403/400/404 final, URL lifetime tracked on its own clock), re-hashing each part
     before it leaves, and prints one status line;
   - after ssh returns, the challenge completes the upload (`finish_upload`: every planned part held at
     its size, `If-None-Match`), checks the object's `HEAD` against the declaration, checks free disk
     (2.2 S + 1 GiB), and fetches with `R2ObjectStore.fetch_to` (ranged, parallel, every 4 MiB block
     against the sidecar, then the whole sha256, atomic replace; `check=` stops it on cancel or
     deadline). The result is only accepted if that sha256 equals the declared one **and** the
     existing reference/audit comparison passes.
   Why R2 as the control channel and not the ssh pipes: the ssh transport and the shared fake host
   stay as they are, nothing is added to the docker verbs, and the container's only side channel
   besides ssh is the pair of URLs it was given.
4. **Keys are per attempt**: `runs/<run>/exec/<job digest>/<nonce 32 hex>/{declare.json, offer.json,
   r/<sha256>, r/<block-list key>}`. The nonce is random, journaled (`exec_artifact_begin`) before
   any URL exists, and never reused. **Cross-attempt resume is not delivered, and cannot be with one
   tar**: `summary.json` holds `elapsed_ns`, `environment.json` the host's UUIDs, and `COMPLETE`
   binds the request, which carries a clock-based budget; two attempts of one job on one host gave
   three different tar hashes [A]. Adopting an earlier attempt's open upload would also let a lying
   host's metadata and still-valid URLs reach an honest host's upload. Per-file content-addressed
   objects would give real resume (see "Next").
5. **The host holds URLs, never a credential.** The R2 token is a regular file, owner = the
   challenge's uid, mode 0600, holding only the five R2 names (never `CLOUDFLARE_API_TOKEN`), read
   by `load_credentials` with the process environment ignored; the bucket must start `cortex-v3-`
   and the endpoint be https. URLs live `min(time left to the job deadline + margin, cap)` seconds.
   They appear in no log, journal, exception, record or `repr` (tests build a harvest of every
   signature and search for it; the harvest finds them in `request.json`, the one place they travel).
   The container accepts https, or http to a literal loopback address, on **one origin**, without
   user info or fragment, through `http.client` (no redirect, no proxy variable), and prints a class
   name or a status, never a URL.
6. **Time.** The island's budget is `budget_seconds - reserve_seconds` so the transfer has time left
   (default reserve 60 s; the window table says what a profile needs). Every wait in the container is
   bounded (offer wait, deadline, URL lifetime); a failure of the challenge's thread sets an event that
   stops the ssh call at once and is reported as the attempt's failure, not as a deadline.
7. **Provenance.** `role-dispatch.json` gains `artifact: {transport, sha256, size, part_bytes, parts,
   upload_ms, download_ms}` (strings and integers: `envelope_v2.load_json` refuses floats);
   `result_tar_sha256` is the declared and verified hash. Journal: one record per attempt event, not
   per part.

## Failure model

| Event | Class | Action |
| --- | --- | --- |
| R2 unreachable at the start of an attempt (probe) | fall back | `exec_artifact_unavailable`; the attempt runs the ssh way (`fallback = classic`, default) or fails (`fail`) |
| Operation not in `operations` | classic | no R2 call |
| Host cannot reach the store (runner preflight, exit 11) | host busy | not counted, host skipped for this sweep |
| Declaration refused (size, shape, wrong request), exit 10 | attempt failed | counted; nothing created in R2 beyond the mailbox |
| Store fails while the island runs or during the upload (exit 10), URL expired (403), status line absent or wrong | attempt failed | counted; open upload aborted, object and block list deleted |
| Challenge thread fails (store down) | attempt failed | cancels ssh at once; container removed |
| Host lost (ssh 255) after every planned part is stored | salvaged | complete, fetch, verify: the hashes vouch for the bytes, not the exit status |
| Host lost mid-upload | host lost, not counted | upload aborted, nothing published; the next attempt starts over with new keys |
| Container exits 0 with a part missing or short | attempt failed | `finish_upload` refuses |
| Uploaded bytes differ from the declared whole hash or block list | host refused | quarantined as in ADR 0008; object deleted |
| Parts differ from the hashes the host declared | none reaches R2 | R2 answers 400 (signed `x-amz-content-sha256`) |
| Storage error on the challenge side after ssh succeeded | attempt failed | one translation boundary; text is a class name |
| Stdout longer than 1 MiB in R2 mode | attempt failed | the cap is `SMALL`, not `max_result_bytes` |

## Windows: which sizes fit which windows

Serial model, per job: `T = start + compute + hash + input + upload + download + cpu + 13 s`, where
hash = `S / 2.3 GB/s` in the container [V, this host], input = `I / BW_ssh` (BW_ssh = 42 MB/s at 50 ms
RTT [A]), cpu = 8.9 s per GB of result [A], and the 13 s is the challenge-side fixed part [A].
`start + compute` is [V] for dense-1b H = 8 (124-204 s on one GPU, S3), [A] for od-base (63.4-63.9 s,
joinbase-6), [H] elsewhere. R2 throughput is a scenario axis, up/down in MB/s: **A** 18.8/36.7 [V,
ADR 0003, one non-GPU host]; **B** 100/88 [A, host to s100, not R2]; **C** 500/500 [?]; **D**
1250/1250 [?]. Totals in seconds (low-high where compute is a range) [H, arithmetic]:

| Profile (P, g, H/J) | S | Window | A | B | C | D |
| --- | --- | --- | --- | --- | --- | --- |
| tiny-cpu (824 parameters) | 13 kB | 300 | fits | fits | fits | fits |
| kimi-tiny (77.0 M, 1, 8/4) | 4.08 GB | 300 | 464-484 | **223-243** | **152-172** | **142-162** |
| od-base TEST (123.8 M, 2, 64/16) | 11.64 GB | 813 | 1169 | **481** | **279** | **251** |
| dense 0.35 B (350 M, 1, 8/4) | 18.56 GB | 300 | 1854-1885 | 758-789 | 436-467 | 391-422 |
| od-large (353.9 M, 8, 512/64) | 86.4 GB | 4398 | 8475-10011 | 3371-4907 | 1871-3407 | 1664-3200 |
| dense-1b D20 (889.3 M, 2, 8/4) | 62.3 GB | 300 | 5897-5977 | 2218-2298 | 1137-1217 | 987-1067 |

Bold: fits its window. Reading:

- Only a **kimi-tiny-sized** job (<= about 4 GB) fits a 300 s lease, and only at about 100 MB/s each
  way or better [?]. An **od-base-sized** job (<= about 12 GB) fits an H = 64 lease (813 s) at 100 MB/s
  and the 297 s trial only at 500 MB/s with the proof POST decoupled. Over the 300 s windows, 18.6 GB
  (0.35 B) fits nowhere in the table. These are the first non-toy steps: **kimi-tiny for 300 s leases,
  od-base for H = 64 leases**; the 100-300 M band the brief names is, today, 77-125 M.
- **dense-1b D20 does not fit at any rate in the table, and cannot complete at all** (protocol
  ceiling above). The 60 GB figure of `docs/cuda-executor.md` is the same order (62.3 GB at H = 8,
  g = 2; 105 GB at the plan shape H = 30, J = 5).
- Past about 150 MB/s the **challenge CPU path (8.9 s/GB) dominates**: at C and D it is 36-104 s of the
  total for the first two profiles. R2 alone does not widen the windows beyond that point; fewer
  validations of the same bytes (three on a reference, four on an audit) and a threaded `fetch_to`
  are the next lever.
- The input leg stays on ssh and is 15 s (kimi-tiny) to 169 s (dense-1b) at 42 MB/s; at 150 ms RTT
  (14 MB/s) od-base's 1.98 GB alone takes about 140 s. That is why inputs by R2 GET are the next lot.
- Peak disk on the challenge is about `2S` (the file `fetch_to` writes and the copy `_read_result`
  stages); `result.tar` is unlinked before `_publish`, which then adds the inputs. RAM for the input
  tar is the inputs. `max_artifact_bytes` defaults to 16 GiB and must be raised per profile (at most
  10,000 parts: 168 GB with 16 MiB parts). `declare.json` is about 2 MB at 103 GB and `offer.json`
  4-8 MB at 6-10 k parts; the container reads at most 32 MiB.

## Consequences

- A result of several GB reaches the challenge over many HTTPS streams instead of one ssh stream,
  verified block by block, and the host never holds the R2 credential.
- Cost [H from ADR 0003 tariffs]: a 62 GB job at 16 MiB parts is about 3.7 k UploadPart + about 3.7 k
  range GETs + about 12 other operations: well under 0.1 USD per job; about 270 such jobs stay inside
  the monthly free Class A tier. Orphan uploads expire by the bucket rule below.
- **Prerequisites for the TEST bucket (Forge)**: a bucket `cortex-v3-test-*`; a bucket-scoped
  `Object Read & Write` token stored 0600 outside the repository and never displayed; lifecycle rules
  "abort incomplete uploads after 1 day" and "expire `runs/*/exec/` after 1 day" (the two mailbox
  objects of every attempt are left for the rule); a ledger line; at most 50 USD. The code cleans its
  own uploads, object and block list; it does not install a rule.
- **Hash consequences** (measured with `network_qualification.sources()` / `bundle()` and
  `ChallengeStore._service_implementation_v2`, before = `33b50656`, after = this branch's tip):

| Value | Before | After |
| --- | --- | --- |
| HASHES_PLACEHOLDER | | |

  Older qualification configs stop validating against a tree at this commit, as after ADR 0007 and
  0008; `runner_sha256` of the provenance changes. The batches are not additive: ADR 0012 assumes the
  runner is unchanged. No wire message, manifest field, run id or golden changes.
- Residual risk: with `network = default` (required) a compromised island could reach the declare,
  offer and part URLs of this one job until they expire. The island is image-pinned and parses
  safetensors without pickle; the URLs are kept out of its environment, argv, work directory and log.

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
  lock released); fewer validations; the 8 GiB object ceiling (wire change).

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
- **Hashing with MD5 for resume** (ETag): dropped with adoption; part sizes are checked like
  `send_offer` does and the whole sha256 is the authority.

## Not verified

- No GPU host, no real ssh/docker and no real R2: the tests use the fake host and the in-memory R2 of
  `tests/storage/fake_r2.py`; they are not evidence of live behaviour. The opt-in live test
  (`tests/gpu_ops/test_artifact_r2_live.py`, marker `cloudflare`) was written and **not run**: this
  session has no bucket-scoped token. It is where these R2 behaviours get probed, none of which ADR
  0003 probed: a signed `if-none-match` on a presigned single PUT answering 412 the second time; a
  presigned GET of an absent key answering 404 (the runner's preflight and polling rely on it);
  UploadPart on an aborted upload answering 404; the status of an expired URL; overwriting a part
  number; thousands of parts to one key (the 1 write/s rule); completing a 6,000-part upload within
  the client timeout; a signed `content-length`. Until it runs they are [?].
- R2 throughput from a GPU host (scenarios A-D above), the s100 CPU rate, the container image's CA
  bundle for `ssl` (the GPU image installs no `ca-certificates` itself [?]), SHA-NI on the hosts.

## Sources (read 2026-10-27)

- `docs/adr/0003-cloudflare-data-plane.md`, `docs/adr/0008-generic-cuda-executor.md`,
  `docs/cuda-executor.md`; S13 report `v3-d34-s100-gpu--82f4b9a8/REPORT.md`.
- Code: `gpu_ops/{cuda_executor,cuda_executor_runner,work_screen}.py`, `storage/{r2,cas,offer}.py`,
  `data/store.py:150-192,338`, `miner/island_launch.py`, `protocol/messages_v2.py:56-58,666-673`,
  `challenge/{store.py:2972,2987,4249-4312,7338; admission.py:256}`.
- Design review of this batch (six read-only lenses, 2026-10-27): the ssh window, the size formula,
  the CPU rates and the tar-hash observation are [A] from it.
