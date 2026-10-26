# v3-dataplane-cloudflare: R2 client, coordination decision, Tunnel (ADR 0003): REPORT

## Status

DONE: code, tests and ADR, 2026-10-26.
- Branch: `v3-dataplane-cloudflare`, based on `ac4c4dec` (`origin/v3-integration`).
- Not pushed: this session has no keyring (PUSH-ME below).
- Labels: [V] verified by a command, a probe or a run in this session; [A] reported by a
  cited document; [H] hypothesis; [?] unknown.

## Result

- **R2 client** [V], `hypertrain/src/hypertrain/storage/r2.py`:
  - `R2Client` gains CreateMultipartUpload, UploadPart, ListParts (paged), ListMultipartUploads,
    CompleteMultipartUpload with `If-None-Match: *`, and Abort.
  - It also gains `presign()` and an opt-in `Retry`: full-jitter backoff on 429, 5xx and
    transport errors, `Retry-After` honoured, at least 1 s on a 429.
  - `data.store.sigv4_presign` now signs and keeps the URL query and extra headers. The AWS
    published presign vector still matches.
- **Content-addressed store** [V], `storage/cas.py`, `R2ObjectStore`:
  - It implements the v3-core-eviction checkpoint-store contract (put/get/exists) and the
    `data.store.Store` presign.
  - Parts are whole 4 MiB blocks, 8 MiB by default, grown only past 10,000 parts (Forge,
    option A).
  - The block list is the relay `UploadChunkManifest` chunk list, and its digest equals
    `chunk_manifest_hash`.
  - Uploads resume from R2 itself (size + MD5 ETag of each held part), with no local journal.
  - Downloads are ranged, parallel and checked block by block.
  - A presigned PUT is bound to the key's own bytes, once.
- **Presigned offers (grants)** [V], `storage/offer.py`:
  - A host with no credential gets one UploadPart URL per part, each bound to its declared
    sha256, plus ListParts pages to resume.
  - The coordinator completes only when every planned part is held at its planned size.
- **Decision** [V, ADR]:
  - Bulk bytes go host to R2 directly.
  - Workers, Durable Objects and Queues are not retained for v3.0, so no prototype was built.
  - Tunnel and Access are for the control plane only.
  - One bucket per environment.
  - Fallback: any S3 endpoint, and the relay and local-disk paths are untouched.
- **R2 behaviour probed on the real service** [V, bucket `cortex-v3-test-dataplane-20261026`,
  2026-10-26]:
  - A presigned PUT or UploadPart signing `x-amz-content-sha256` answers
    `400 XAmzContentSHA256Mismatch` to other bytes and `403` without the header.
  - An unbound presigned PUT accepts any bytes.
  - `x-amz-checksum-sha256` gives `BadDigest` on PutObject and `501 NotImplemented` on
    UploadPart.
  - `If-None-Match: *` on CompleteMultipartUpload gives 200 for a new key and 412 for an
    existing one.
  - A non-final 4 MiB part passes UploadPart and fails CompleteMultipartUpload with
    `EntityTooSmall`.
  - A part's ETag is its MD5. ListMultipartUploads is supported.
  - Three back-to-back PUTs to one key all got 200.
- **Independent review** (a workflow of 4 reviewers plus 4 adversarial verifiers, run on the
  first version of this branch):
  - 43 findings: 32 confirmed and fixed, 11 refuted by the verifiers.
  - Fixes in code:
    - offers hide their URLs in `repr`;
    - part hashes are validated before any request;
    - an aborted upload raises `UploadGone` instead of `ObjectConflict`;
    - every completed put is checked with `exists()`;
    - `R2Config.region` makes the AWS fallback real.
  - Fixes in the ADR: the binding claim (R2 enforces the declared part hashes, not the grant),
    the blast radius, the Worker rationale, the checkpoint path for credential-free hosts, the
    cost formulas, the limits sourcing, the measurement threshold, and the brief items that
    were missing (outage, replacement plan, cache).
  - 13 tests were added, one for each confirmed coverage gap.

Commits:

| Commit | Subject |
| --- | --- |
| `999b2f91` | feat(storage): add multipart, retry and bound presign to the r2 client |
| `fce3d56a` | feat(storage): add a content-addressed r2 store in 4 mib block multiples |
| `b7207a76` | feat(storage): add presigned multipart upload offers for hosts |
| `26879763` | test(storage): add the opt-in cloudflare round trip on a test bucket |
| `602214b5` | docs(adr): record the cloudflare r2 data plane decision in adr 0003 |
| (this commit) | docs(dataplane): report the r2 data plane batch and its evidence |

Files:

- `hypertrain/src/hypertrain/storage/r2.py`: multipart primitives, `presign`, `Retry`,
  `R2Config.region`, `NoSuchKey`, `UploadGone`, `PartInfo`, `ObjectInfo.meta`.
- `hypertrain/src/hypertrain/data/store.py`: `sigv4_presign` keeps and signs the query, signs
  `headers`.
- `hypertrain/src/hypertrain/storage/cas.py` (new): `plan_parts`, `BlockList`, `scan`,
  `R2ObjectStore`, `put_headers`.
- `hypertrain/src/hypertrain/storage/offer.py` (new): `offer_upload`, `send_offer`,
  `finish_upload`.
- `hypertrain/tests/storage/fake_r2.py` (new): fake R2. It checks header and presigned SigV4
  against the request as sent, checks the payload sha256, and applies the probed multipart
  rules and fault injection.
- `hypertrain/tests/storage/test_r2_client.py` (9 functions, 14 cases), `test_cas.py` (19, 21),
  `test_offer.py` (10, 10), `test_r2_live.py` (1, opt-in): 45 new offline cases, 2.8 s in total.
- `hypertrain/pyproject.toml`: marker `cloudflare`.
- `docs/adr/0003-cloudflare-data-plane.md`, `docs/dataplane/measurement-plan.md`,
  `docs/dataplane/resources.md`, `docs/index.md`, `hypertrain/CHANGELOG.md`,
  `hypertrain/docs/r2.md`.

## Tests

Before the change [V], the new tests run against `git archive ac4c4dec hypertrain/src` with
`PYTHONPATH`:

```
E   ImportError: cannot import name 'Retry' from 'hypertrain.storage.r2'
E   ModuleNotFoundError: No module named 'hypertrain.storage.cas'
E   ImportError: cannot import name 'offer' from 'hypertrain.storage'
3 errors in 0.23s
```

On the base, `sigv4_presign` drops the UploadPart query (`partNumber kept: False | uploadId
kept: False`); on this branch both are kept (`True | True`).

After the change [V]:

```
cd hypertrain && uv run --no-sync pytest -q -p no:cacheprovider tests/storage tests/data/test_store.py
```

```
SKIPPED [1] tests/storage/test_r2_live.py:49: opt-in live R2 test: set HT_CLOUDFLARE_ENV_FILE and HT_R2_TEST_BUCKET
70 passed, 1 skipped in 2.69s
```

That is 45 new cases (`test_r2_client` 14, `test_cas` 21, `test_offer` 10) and the 25
existing storage and data-store tests.

Local mirror of `.github/workflows/hypertrain-ci.yml` on the final tree. It used the same
commands, the 8 round-robin shards and the same deselects, with basetemps under
`/tmp/dp3/ci` (e2e under `.scratch`), 2026-10-26 20:16-20:30Z:

| Job | Result |
| --- | --- |
| lint: `ruff format --check`, `ruff check`, `mypy --package hypertrain ...` | 402 files formatted, all checks passed, mypy: no issues in 134 source files |
| shard 0 | 400 passed |
| shard 1 | 151 passed, 2 skipped |
| shard 2 | 355 passed |
| shard 3 | 654 passed, 3 skipped, **1 failed**: `tests/trainer/test_engine_flags.py::test_compile_flag_repeatable` (`InvalidCxxCompiler`: this host has no C++ compiler for Inductor; a known local-only failure that also fails on the base) |
| shard 4 | 289 passed |
| shard 5 | 232 passed, 3 skipped |
| shard 6 | 203 passed, 3 skipped |
| shard 7 | 610 passed, 7 skipped |
| sim | 5 passed |
| e2e | 94 passed |

Total: 2,993 passed, 18 skipped, 1 failed (environment).

The first mirror run, before the review fixes, used deep basetemps under `.scratch/`. It also
failed `tests/gpu_ops/test_network_role_dispatch.py::test_actual_probe_inflight_cancel_reaps_process_retains_custody`
with `OSError: AF_UNIX path too long`. The test passes with a short basetemp, and it passed in
this run.

Other gates [V]:
- `tests/test_publish_exclusions.py`: 57 passed. The publish secret scan covers the new files.
- `uv run --no-sync python -I ../scripts/check_repo.py --final`: "Repository contracts passed".

Each commit was also checked in isolation, from a `git archive` of `hypertrain/` at that commit
with the same venv and `PYTHONPATH` on the archive. `tests/storage`, `tests/data/test_store.py`
and `tests/datasets/test_publish_cli.py` pass at every commit:

| Commit | Result |
| --- | --- |
| `999b2f91` | 42 passed |
| `fce3d56a` | 63 passed |
| `b7207a76` | 73 passed |
| `26879763`, `602214b5` and this commit | 73 passed, 1 skipped (the opt-in live test) |

Outside the mirror, 7 `tests/relay/test_transport_v2.py` `pinned_kind` tests fail when run
alone, on the base too. They need a `kind` binary on `PATH` ("pinned kind executable required
in PATH"). Inside the full shard they pass. That ordering dependency is not caused by this
branch.

### Required by Forge

- `test_plan_refuses_non_final_parts_under_5_mib`: a 4 MiB part size is refused, by
  `plan_parts` and by the store constructor; an `8 MiB + 1` object gets a 1-byte last part.
- `test_plan_refuses_an_object_needing_more_than_10000_parts_at_8_mib`: 10,000 x 8 MiB is
  accepted and one byte more at 8 MiB is refused; with no size given the plan grows to 12 MiB
  (6,667 parts); the 4.995 TiB object cap.
- `test_block_list_is_the_relay_chunk_manifest`: block bodies equal
  `relay.client.chunk_manifest(path).chunks`, and the digest equals `chunk_manifest_hash()`.

### Mutations (51 mutants, 51 killed)

Each mutant was applied to a copy of `hypertrain/src`, never to the worktree, and the named test
was run with `PYTHONPATH` on the copy. The import path was asserted, and every kill is a test
failure, not a collection error.
- M1-M29 were written with the code.
- N1-N21 cover the gaps the independent review confirmed. Each of those mutants survived the
  earlier suite.

| Mutant | Killed by |
| --- | --- |
| min part size check dropped | `test_plan_refuses_non_final_parts_under_5_mib` |
| 10,000-part bound off by one | `test_plan_refuses_an_object_needing_more_than_10000_parts_at_8_mib` |
| auto plan does not grow, grows at exactly 10,000, object cap dropped | same test |
| whole-block rule dropped | `test_plan_parts_are_whole_4_mib_blocks` |
| last block length wrong, block count off at exact multiples | `test_block_list_is_the_relay_chunk_manifest` |
| resume trusts size without MD5 | `test_resume_resends_a_held_part_whose_bytes_differ` |
| resume ignores open uploads | `test_crashed_upload_is_invisible_then_resumes_from_r2` |
| lost complete reply not handled | `test_complete_whose_reply_was_lost_is_a_success` |
| UploadPart 404 not `UploadGone` | `test_put_succeeds_when_another_writer_completes_first` |
| aborted upload reported as a conflict | `test_aborted_upload_is_reported_and_a_rerun_starts_over` |
| single PUT or multipart complete not write-once | `test_put_refuses_a_key_held_by_other_bytes` |
| get does not verify, fetch_to skips block checks | `test_reads_refuse_a_corrupt_object` |
| fetch_to whole-sha check dropped | `test_fetch_without_a_block_list_still_checks_the_whole_sha256`, `test_host_declaring_other_bytes_gets_an_object_no_reader_accepts` |
| missing sidecar not corrupt, sidecar size unchecked | `test_fetch_refuses_a_missing_or_wrong_block_list` |
| download window never refills, block list written after complete | `test_put_file_multipart_round_trip` |
| exists ignores sha256 metadata | `test_exists_is_false_for_an_object_without_its_sha256_metadata` |
| presigned PUT not bound to sha, not write-once | `test_presigned_put_accepts_only_the_keyed_bytes_once` |
| part not re-checked before send | `test_file_changed_after_hashing_is_refused` |
| presign drops the URL query, does not sign headers | `test_presigned_part_url_is_bound_to_its_query_and_headers` |
| region ignored | `test_region_is_the_sigv4_scope_of_requests_and_presigned_urls` |
| 429 not retried, no 1 s floor | `test_retry_backs_off_on_throttling_and_server_errors` |
| transport errors or 502/504 not retried | `test_retry_covers_throttling_server_errors_and_transport_errors` |
| backoff does not grow | `test_retry_backoff_doubles_up_to_its_cap` |
| Retry-After ignored | `test_retry_honours_retry_after` |
| one attempt too many | `test_retry_gives_up_after_its_attempts_and_without_policy_never_retries` |
| ListParts reads one page | `test_list_parts_follows_pages` |
| complete not conditional | `test_complete_is_write_once_and_reports_a_gone_upload` |
| finish completes over a missing part | `test_finish_refuses_an_upload_with_a_missing_part` |
| finish ignores part sizes | `test_finish_refuses_a_part_shorter_than_planned` |
| finish lost reply not handled | `test_finish_survives_a_lost_reply_and_a_second_call` |
| send_offer resends held parts, reads one list page | `test_send_offer_resumes_from_every_presigned_part_list_page` |
| send_offer ignores retry | `test_send_offer_retries_a_throttled_part` |
| host sends a part off its declaration, part URL not bound | `test_part_url_accepts_only_the_declared_bytes` |
| offer accepts a short hash list, part hashes checked after requests | `test_offer_refuses_part_hashes_that_do_not_fit_the_plan` |
| presign ignores expires, offer repr shows URLs | `test_offer_urls_expire_as_asked_and_stay_out_of_repr` |

## R2 probe output (2026-10-26, bucket `cortex-v3-test-dataplane-20261026`)

Two scratch probe scripts: SigV4 from `hypertrain.data.store`, statuses and R2 error codes only.
- In `2a`/`A` the presigned URL signs `x-amz-content-sha256` as a header, with an
  `UNSIGNED-PAYLOAD` canonical request, which is the scheme the code uses.
- `2b`/`6b` put the hash in the canonical request instead: R2 refuses that signature.
- `6`/`B` send `x-amz-checksum-sha256` on UploadPart: 501.

```
1 header-signed x-amz-content-sha256 mismatch: HTTP 400 XAmzContentSHA256Mismatch
2a presign hdr sha(A) unsigned-canon, body B: HTTP 400 XAmzContentSHA256Mismatch
2a presign hdr sha(A) unsigned-canon, body A: HTTP 200 
2b presign canon sha(A), body B: HTTP 403 SignatureDoesNotMatch
2b presign canon sha(A), body A: HTTP 403 SignatureDoesNotMatch
3 presign checksum-sha256(A), body B: HTTP 400 BadDigest
3 presign checksum-sha256(A), body A: HTTP 200 
3 HEAD checksum header present: True matches: True
3b header-signed checksum-sha256(A), body B: HTTP 400 BadDigest
3c presign unbound, body B: HTTP 200 
3d presign checksum bound but header omitted, body A: HTTP 403 SignatureDoesNotMatch
4 create: HTTP 200 
4 part 1: HTTP 200 
  etag==md5(part): True
4 part 2: HTTP 200 
  etag==md5(part): True
4 ListParts: HTTP 200 
  listed sizes: [b'8388608', b'1048576'] etags match: True
4 complete If-None-Match new key: HTTP 200 
4 HEAD: HTTP 200 size=9437184 meta_ok=True etag_suffix=b-2"
4 complete again (upload gone): HTTP 404 NoSuchUpload
4b complete If-None-Match existing key: HTTP 412 PreconditionFailed
4b abort: HTTP 204 
4b ListParts after abort: HTTP 404 NoSuchUpload
4c PUT If-None-Match existing key: HTTP 412 PreconditionFailed
5 part 1 (4 MiB): HTTP 200 
5 part 2 (4 MiB): HTTP 200 
5 complete with 4 MiB non-final part: HTTP 400 EntityTooSmall
5 abort: HTTP 204 
6 presigned part checksum bound, wrong body: HTTP 501 NotImplemented
6 presigned part checksum bound, right body: HTTP 501 NotImplemented
6b presigned part content-sha canon, wrong body: HTTP 403 SignatureDoesNotMatch
6 abort: HTTP 204 
7 range GET: HTTP 206 len=16
7 presigned range GET: HTTP 206 ok=True
A presigned part content-sha bound, wrong body: HTTP 400 XAmzContentSHA256Mismatch
A presigned part content-sha bound, header omitted: HTTP 403 SignatureDoesNotMatch
A presigned part content-sha bound, right body: HTTP 200 
  etag==md5: True
A2 presigned part sha+length bound, right body: HTTP 200 
abort: HTTP 204 
B create with checksum-algorithm SHA256: HTTP 200 
B header part checksum, wrong body: HTTP 501 NotImplemented
B header part checksum, right body: HTTP 501 NotImplemented
abort: HTTP 204 
C three back-to-back PUTs on one key: [200, 200, 200]
```

## Live R2 test (opt-in, marker `cloudflare`)

Command (credentials by path only, never printed):

```
cd hypertrain && HT_CLOUDFLARE_ENV_FILE=/var/lib/openbot/.cloudflare-cortex.env \
  HT_R2_TEST_BUCKET=cortex-v3-test-dataplane-20261026 \
  uv run --no-sync pytest -q -s -p no:cacheprovider -m cloudflare tests/storage/test_r2_live.py
```

- **Region**: this host is at Cloudflare colo FRA (DE). The bucket is `WEUR` (location hint
  `weur`, Standard, default jurisdiction).
- **Object**: 64 MiB in 8 parts of 8 MiB, 8 workers.
- **Runs**: 9, of which 8 passed:

| Run (UTC) | Upload | Download |
| --- | --- | --- |
| 19:29:17 | 3.83 s = 17.5 MB/s | 1.61 s = 41.8 MB/s |
| 19:29:29 | failed: R2 answered `502` to the unretried 403 probe; the probes now go through `Retry` | |
| 19:29:41 | 3.94 s = 17.0 MB/s | 1.43 s = 46.9 MB/s |
| 19:30:23 | 3.23 s = 20.8 MB/s | 1.54 s = 43.6 MB/s |
| 19:30:35 | 2.53 s = 26.6 MB/s | 1.88 s = 35.7 MB/s |
| 19:30:46 | 3.90 s = 17.2 MB/s | 2.36 s = 28.4 MB/s |
| 19:38 (a) | 3.96 s = 16.9 MB/s | 1.94 s = 34.5 MB/s |
| 19:38 (b) | 3.04 s = 22.0 MB/s | 2.09 s = 32.1 MB/s |
| 20:14 (after the review fixes) | 5.57 s = 12.0 MB/s | 2.20 s = 30.5 MB/s |

- **Measured**: upload 12.0-26.6 MB/s (mean 18.8, 96-213 Mbit/s); download 28.4-46.9 MB/s
  (mean 36.7). For comparison, this host's single stream to the Cloudflare edge
  (`speed.cloudflare.com`) is 26.3 MB/s up and 112 MB/s down.
- **Also checked on R2 in every run**:
  - a bound part URL refuses other bytes (400) and a missing header (403);
  - a credential-free host completes an offer;
  - a 4 MiB non-final part fails completion;
  - a conditional complete on an existing key returns 412.
- **Cost**:
  - about 35 Class A and 15 Class B operations per run, plus about 50 probe requests, and at
    most 73 MiB stored for seconds;
  - inside the R2 free tier (1 M Class A, 10 M Class B, 10 GB-month per month; r2/pricing read
    2026-10-26), so **0.00 USD**;
  - under 0.01 USD at list price.
  - Total batch cloud spend: 0.00 USD, against a cap of 5 USD.

## Cloudflare resources

- **Read-only inventory at session start** [V, account `...d807`]:
  - token active, 8 R2 buckets, 2 Workers scripts, 0 Queues, 0 DO namespaces, 5 tunnels,
    17 zones;
  - no `cortex-v3-test-*` resource left by the two earlier attempts of this batch.
- **Created**: `cortex-v3-test-dataplane-20261026` (R2 bucket, `WEUR`), 2026-10-26T19:14:35Z,
  for the probes and the live test. Recorded in `docs/dataplane/resources.md`.
- **Deleted**: emptied, then deleted at 2026-10-26T20:16:38Z with the commands in
  `resources.md` (`left: 0 0`, `True []`).
- **Inventory after deletion** [V]: 8 R2 buckets, 2 Workers, 0 Queues, 0 DO namespaces,
  5 tunnels, 17 zones, no `cortex-v3-test-*` resource. This is the starting state.
- **Not touched**: `relay.joinbase.ai`, Bunny, DNS of existing zones. No DNS change, no Tunnel,
  no Worker, no Queue, no DO namespace, no token.
- **Finding** [V]: the API token on this host has 384 permission groups, including
  `Account API Tokens Write`, `Billing Write` and `DNS Write`, and the R2 S3 key lists all
  account buckets. ADR 0003 asks the founder for bucket-scoped production tokens and for
  rotation.

### Ledger request (for Pilote)

Line to add to `livrables/subnet-100/ledger.md`:

- 2026-10-26, Cloudflare R2, batch v3-dataplane-cloudflare.
- One test bucket `cortex-v3-test-dataplane-20261026`, created 19:14:35Z, deleted 20:16:38Z.
- 9 live runs and 2 probe scripts: about 370 Class A and 170 Class B operations, under 0.1 GB
  stored for minutes.
- **0.00 USD**: R2 free tier (r2/pricing, read 2026-10-26).

This session did not write the ledger.

## Not verified

- Throughput from any host other than this one (FRA). B300 hosts, other regions and Local
  Uploads were not measured: see `docs/dataplane/measurement-plan.md`.
- Wiring into the challenge (`Config` reads no R2 setting) and a versioned `UploadGrant`
  carrying offer URLs. Neither is in this batch.
- Compatibility with the v3-core-eviction checkpoint manager: tested against the contract
  copied from its uncommitted draft (`elastic/store.py:25-31`, put/get/exists, `ObjectNotFound`
  and `CorruptObjectError`). The real class is not on any branch yet.
- The R2 per-key 1 write/s limit (429) was not reproduced; the code avoids rewriting keys
  anyway.
- AWS egress price for host uploads: the per-GB table did not render on the AWS page [?].
- Bunny and relay cost: unknown [?].
- CI on GitHub: not run, because the branch is not pushed. The local mirror of
  `hypertrain-ci.yml` is above.

## Size

1,971 changed lines in total.
- Code outside tests: about 790 lines (`r2.py` +190, `cas.py` 371, `offer.py` 177,
  `store.py` +30), docstrings included. The rest is tests (811) and docs.
- That is over the ~400-line guideline. The five commits are layered (client, store, offers,
  live test, ADR) and can be reviewed one by one. A split into two PRs is possible:
  client + store, then offers + ADR.

PUSH-ME v3-dataplane-cloudflare
STEP-DONE
