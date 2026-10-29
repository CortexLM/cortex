# ADR 0003: Cloudflare R2 data plane

- Status: draft for branch `v3-dataplane-cloudflare`, 2026-10-26 (Principal SN100 D13). Forge
  decides promotion to `v3`. Nothing here changes a production run, master, relay, profile,
  pin, DNS record or emission. The challenge still stores objects on local disk by default.
  Amended 2026-10-27 by the branch `v3-core-eviction-fixes`: checkpoint retention (Decisions 3
  and 5, Consequences, cost model, founder decision 1), after finding F7 of the Lens review of
  `v3-core-eviction` (see ADR 0002, Sources).
- Order: brief `v3-dataplane-cloudflare` and checklist `C-cloudflare-data-plane`. The part size
  rule in Decision 2 is Forge's answer to the session question on 2026-10-26 (option A: parts
  are 4 MiB multiples, 8 MiB by default).
- Labels: **[V]** verified in code, by a probe on the real R2 service, or on a vendor page
  fetched on the date above; **[A]** reported by a cited document; **[H]** hypothesis or
  arithmetic; **[?]** unknown.
- Code: `hypertrain/src/hypertrain/storage/{r2,cas,offer}.py`. Tests:
  `hypertrain/tests/storage/{test_r2_client,test_cas,test_offer,test_r2_live}.py`. Test
  resources: [docs/dataplane/resources.md](../dataplane/resources.md). Multi-region plan:
  [docs/dataplane/measurement-plan.md](../dataplane/measurement-plan.md).

## Context

- **No production byte goes through R2 today** [V]. The challenge object store is a
  `LocalFSStore` (`challenge/app.py:328`, `challenge/store.py:540-551`), and service audits
  refuse any other store (`store.py:6931`). Only the relay can be configured onto S3 or R2
  (`relay/app.py:61-65`, `:240-256`), and `relay.joinbase.ai` runs on local disk on s100 [A,
  `livrables/forge/2026-10-20_deploy-hypertrain-joinbase-3.md:25-35`]. The repository has no
  Bunny code; the three Bunny-hosted relays exist only in the registry of a TEST run that cannot
  be mined [A, `livrables/forge/2026-10-20_hypertrain-single-live-run.md:53`].
- **Upload grants today** [V]. The coordinator key signs an `UploadGrant` in the `ht-relay/1`
  domain (`relay_envelope.py:22-23`, `store.py:9984`). It binds the run, round `w`, hotkey,
  relay, `delta_hash`, size and `chunk_manifest_hash`, and expires at `d_upload`
  (`relay_messages.py:114-133`). It has no admission field. The chunk list is 4 MiB blocks
  (`CHUNK_BYTES = 4 << 20`, `relay_messages.py:23`), at most 512, so a relay object is at most
  2 GiB (`:136-158`). The miner sends chunks one at a time (`relay/client.py:159-179`).
- **Volumes** [A, `2026-10-20_deploy-hypertrain-joinbase-5.md:90`; H from the 123,753,984-parameter
  od-base model]: a trial uploads state 1.485 GB, ef 0.495 GB and delta 126 MB (about 2.1 GB),
  and downloads about 1.98 GB of inputs. A round uploads only the delta through the relay,
  while state and ef are committed by reference (`miner/core.py:1282-1286`) [V]. Trusted
  reference publications take about 13 GB of server disk per trial [A, joinbase-6 report:33].
- **Window** [V]: the trial deadline is `seed + 100` drand rounds of 3 s (`admission.py:256`,
  `store.py:2737`), that is 297-300 s. Since ADR 0017 that is the default: a TEST policy may sign
  `challenge_rounds` up to 400. Measured 785-913 Mbit/s with 16 parallel 1 MiB PUTs, but
  231.6 Mbit/s in the relay's own pattern of one 4 MiB chunk at a time [A, joinbase-6 run 2].
- **R2 limits** that shape the design [V, read 2026-10-26]:
  - From `r2/objects/upload-objects` (updated 2026-07-29): a part is at least 5 MiB, except the
    last; all parts except the last must be the same size; at most 10,000 parts; the maximum
    part is "5 GiB".
  - From `r2/platform/limits` (updated 2026-06-08): its footnotes cap a single PUT and a part
    at 5 MiB under 5 GiB (4.995 GiB), and an object at 5 GiB under 5 TiB (4.995 TiB). The code
    uses these stricter values.
  - Also from `r2/platform/limits`: one write per second per key; faster writes get 429.
  - From `r2/api/s3/presigned-urls` (updated 2026-08-22): presigned URLs allow GET, HEAD, PUT
    and DELETE only, live 1 s to 7 days, and do not work on custom domains.
  - From `r2/buckets/object-lifecycles` (updated 2026-04-21): incomplete uploads are aborted
    after 7 days by default.
  - From `r2/pricing` (updated 2026-10-26): egress is free.
  - From `workers/platform/limits` (updated 2026-10-12) and
    `cache/concepts/default-cache-behavior` (updated 2026-09-14): one request through a proxied
    hostname carries at most 100 MB of body on Free/Pro, 200 MB on Business, and up to 5 GB on
    Enterprise. Tunnel public hostnames are proxied (`tunnel/concepts/routing`, updated
    2026-10-16), so the same cap applies to them.
- **Probed on the real service on 2026-10-26** [V, bucket `cortex-v3-test-dataplane-20261026`.
  The probe output is in the batch report, and `test_r2_live.py` re-checks the binding, the
  `EntityTooSmall` rule and the conditional complete on every run]:
  - A presigned PUT or UploadPart that signs `x-amz-content-sha256` answers
    `400 XAmzContentSHA256Mismatch` to other bytes and `403` when the header is missing.
  - An unbound presigned PUT accepts any bytes. `S3Store.presign(sha, "PUT")` builds such a URL,
    and the v1 `upload_url` route (`challenge/store.py:1253`) would return one on an S3-backed
    store. Today's store is local, so that route returns a `file://` URI.
  - `x-amz-checksum-sha256` works on PutObject (`400 BadDigest`) but returns `501
    NotImplemented` on UploadPart.
  - `If-None-Match: *` on CompleteMultipartUpload returns 200 for a new key and 412 for an
    existing one.
  - A 4 MiB non-final part is accepted at UploadPart and refused with `EntityTooSmall` at
    CompleteMultipartUpload.
  - A part's ETag is its MD5.
  - Three back-to-back PUTs to one key all returned 200, so the 1 write/s limit was not
    reproduced at that rate.
- **Existing client** [V]: `storage/r2.py` already signs SigV4 on stdlib + httpx with no boto.
  It did single PUTs only, never retried, and could not presign an UploadPart URL:
  `sigv4_presign` dropped the URL query and signed the host header only. No S3 SDK is in either
  lockfile.
- **Account inventory, read-only** [V, 2026-10-26, account `...d807`]:
  - 8 R2 buckets before this batch; 2 Workers scripts, 0 Queues, 0 Durable Object namespaces,
    5 tunnels and 17 zones.
  - The API token on this host has 384 permission groups, including `Account API Tokens Write`,
    `Billing Write` and `DNS Write`. A leak of it would expose the whole account.
  - The R2 S3 key lists every R2 bucket of the account (9, the test bucket included). It
    authenticates the S3 API only, so a leak exposes R2, not DNS, billing or tokens.

## Decision

1. **Bulk bytes go host to R2 directly**, over the S3 endpoint
   `<account>.r2.cloudflarestorage.com`. No Worker, Tunnel or proxy carries them:
   - a whole 1.5 GB object in one request is refused above 100/200 MB below Enterprise;
   - for 8 MiB parts, a Worker would add a hop, a 128 MB isolate, per-request billing and a
     second component to secure, while R2 already enforces each part's sha256 on a presigned
     URL, and a presign is a local HMAC.

   Every R2 credential stays on the coordinator (the challenge master). Hosts, auditors and
   validators get presigned URLs only.
2. **Parts are whole 4 MiB blocks** (Forge, option A). The default part is 8 MiB. The part grows
   one block at a time only when an object would otherwise need more than 10,000 parts
   (`cas.py:74`). The client refuses a part under 5 MiB, a size that is not a block multiple,
   and an object needing more than 10,000 parts at the size asked for. The 4 MiB block stays
   the unit of hashing, verification and resume. The block list of an object is the relay's
   `UploadChunkManifest` chunk list, and its canonical bytes hash to `chunk_manifest_hash`
   (`cas.py:93-133`; test `test_block_list_is_the_relay_chunk_manifest`). A grant signed today
   therefore binds the same digest.
3. **Content-addressed, write-once keys**:
   - One bucket per environment, `cortex-v3-<env>`.
   - `datasets/<dataset_id>/<shard_sha256>` with `If-None-Match`, plus `manifest.json` and
     `build_record.json` (today's publisher layout). The operator rewrites those two on a
     re-publish, which happens rarely and far below 1 write/s.
   - `runs/<run_id>/objects/<sha256>`: written by the coordinator.
   - `runs/<run_id>/up/<hotkey>/<sha256>`: host uploads.
   - `runs/<run_id>/ckpt/<sha256>`: eviction checkpoints, chained by ADR 0002: state objects and
     their manifests of 344 to 720 bytes. A chain is read from its head back to its base and a
     link replays from its parent state, so which of them may be deleted, and when, is ADR 0002's
     rule ("Retention of checkpoint objects"), not a per-prefix choice.
   - A multipart object carries `x-amz-meta-sha256` and `x-amz-meta-blocks`. The second names a
     sidecar object under the same prefix holding the block list.
   - No key under `runs/` is ever rewritten. Chain heads, leases and grant state live in the
     coordinator database, never in a mutable R2 key, which also avoids the per-key write limit.
   - Lifecycle:
     - abort incomplete uploads after 1 day under `runs/`;
     - the coordinator deletes settled deltas as it goes (DeleteObject is free). It deletes
       checkpoint objects only by the rule of ADR 0002 ("Retention of checkpoint objects"): a
       state once the link it ends and the link built on it are both released (audited, out of
       dispute, grace elapsed; the last state of a shard: once the round is settled and the next
       base is stored), a manifest never before its round's retention ends. "Superseded by a
       newer head" is not enough, because a resumer reads every manifest back to the base and a
       link replays from its parent state;
     - expire `runs/<run_id>/` after settlement and the dispute window (founder decision). R2
       counts object age, not run state [A, r2/buckets/object-lifecycles], so on `ckpt/` this
       rule is created only once the run is settled and its disputes are closed (or the
       coordinator lists and deletes the prefix then): a rule installed at run start would
       expire the head state of a stalled round;
     - never expire `datasets/`.

     The cost model's retention figures assume these deletes and keep K = 2 states per host
     (see the cost model); ADR 0002's rule can need more. None is built here: neither the
     checkpoint store of ADR 0002 nor `R2ObjectStore` has a delete, and the coordinator keeps no
     record of past rounds' heads or of each link's audit and dispute status.
4. **Signed grants become presigned part offers** (`offer.py`). The coordinator first checks
   the grant (subnet member, admission owner, commit, deadline), as `upload-grant` does
   (`store.py:9820-9989`). Then it:
   - opens the multipart upload on `up/<hotkey>/<sha256>`;
   - stores the sidecar block list;
   - presigns one UploadPart URL per part, signing `x-amz-content-sha256` = the part sha256
     the host declared;
   - presigns ListParts pages, so the host can resume without any credential.

   The host checks each part against its declared hash before sending it. The coordinator
   completes with `If-None-Match: *` only when every planned part is held at its planned size.
   R2 itself would complete over a missing part number, so `finish_upload` refuses that case.
   URLs expire at `d_upload`.

   What R2 enforces is the part hashes the host declared. Nothing ties them to the grant: the
   sha256 of an 8 MiB part cannot be derived from its two 4 MiB block hashes. So:
   - the versioned grant must also sign the declared part-hash list, which makes a wrong
     declaration attributable to its signer;
   - the coordinator never reads the bulk bytes, and on an offer-written key `exists()` means
     completed, not verified;
   - a wrong declaration damages only the uploader's own key, and every reader verifies the
     whole sha256 (`test_host_declaring_other_bytes_gets_an_object_no_reader_accepts`).
5. **Store contract** (`cas.py:175`, `R2ObjectStore`): the v3-core-eviction checkpoint store,
   plus the `data.store.Store` presign. It holds the S3 credential, so it runs on the
   coordinator and on trusted operator hosts.
   - `put` returns the sha256. A multipart object is invisible until CompleteMultipartUpload.
   - `get` verifies the sha256; a 404 is `ObjectNotFound`.
   - `exists` is true only for a completed object carrying its sha256 metadata.
   - `fetch_to` is a ranged, parallel download that checks every block and replaces the target
     atomically.
   - `presign(sha, "PUT")` returns a single-request upload bound to the key's own bytes, once.
     It signs `x-amz-content-sha256`, `x-amz-meta-sha256` and `If-None-Match: *`. A caller
     that sends no headers, such as today's v1 miner `_put` (`miner/core.py:574`), needs a
     change.
   - Resume needs no local journal, because an evicted Spot host loses its disk with it. The key
     fixes the content, so an open upload on the key keeps every part whose size and MD5 ETag
     match the local bytes. A lost reply to a successful complete counts as success once the
     object exists. An upload aborted mid-way raises `UploadGone`, and a re-run starts over.
   - **Checkpoints on a Spot host** (ADR 0002 flushes on notice into a `CheckpointStore`). The
     host holds no credential, so its store is an offer client:
     - `put` declares the hashes, asks the coordinator for an offer, runs `send_offer`, and the
       coordinator runs `finish_upload`;
     - `get` reads through a presigned GET.

     That adapter needs coordinator routes that no document defines yet (ADR 0002 has none)
     and is not built here, so the flush that ADR 0002's drain runs on a Spot host has no
     implementation in either branch (ADR 0002, Consequences). The
     alternative is R2 temporary credentials scoped to the host's checkpoint prefix and lease
     (TTL up to 7 days; per-action scope needs local JWT signing) [V, r2/api/s3/temporary-
     credentials, updated 2026-10-16]. They would let the host run `R2ObjectStore` itself, but
     they bind no content hash.

     In both models a host must not be able to delete under `ckpt/`: `R2ObjectStore.presign`
     signs GET and PUT only, and a temporary-credential scope must exclude DeleteObject. A host
     that could delete a parent state would make the next link unreplayable, and a manifest a
     broken resume (ADR 0002, "Retention of checkpoint objects") [H]. Only the coordinator's
     credential deletes.
6. **Retry** (`r2.py:50`): full-jitter exponential backoff on 429, 5xx and transport errors.
   A numeric `Retry-After` is a floor up to the cap (16 s by default), and a 429 waits at least
   1 s. Five attempts by default.
   It is opt-in per client, so existing callers keep their behaviour.
7. **Workers, Durable Objects and Queues are not retained for v3.0**, so no coordination
   prototype is built (brief item 3).
   - A presign is a local HMAC with no call to R2 [V, presigned-urls page].
   - The grant decision needs the coordinator's admission and round state; a Worker or DO
     would be a second source of truth.
   - Bulk bytes do not need a Worker (Decision 1).
   - R2 event notifications are at-least-once and carry no checksum. The consumer would still
     re-verify, and `finish_upload` already is the commit point.
   - Reopen this when the coordinator must run active-active in several regions, or when
     beacon fan-out at 500 hosts needs an edge cache; a DO is single-threaded at about 1,000
     req/s.
8. **Tunnel and Access are for the control plane only.** Hosts without a public IP need no
   Tunnel: they make outbound HTTPS calls to R2 and the master. Tunnel may later publish the
   master API without open ports, with Access service tokens (no seat) on admin routes. That
   needs a DNS record, so it is a founder decision and was not created here.
9. **Regions.** Start with one bucket per environment, with a location hint near the
   coordinator and auditors and no jurisdiction. Local Uploads (open beta, PutObject and
   UploadPart only, not on jurisdiction buckets) is enabled only after the measurement plan
   shows a gain. If cross-region reads of round inputs miss the window, the coordinator copies
   the content-addressed objects into regional read buckets: a key is valid in any bucket, and
   readers verify [H, not built].
10. **Fallback.** The client works against any S3 endpoint given its endpoint and SigV4
    region (`R2Config.endpoint` and `R2Config.region`, `auto` for R2). A MinIO or AWS S3
    secondary is therefore a configuration change, and content addressing lets a reader take a
    mirror's bytes.
    - **When R2 fails or throttles**: requests are retried within the budget, then the upload or
      download fails. A trial or round can then miss its window, and today a late trial suspends
      the miner (`admission.py:222-228`). The coordinator must therefore treat a recorded
      data-plane outage as an excused miss. That is for ADR 0002 to settle [H].
    - Keys are unique, so a per-key 429 is not expected. A third-party report gives `503
      SlowDown` above 128 concurrent reads of one byte range [A]. If 500 hosts read the same
      round input, the mitigation is the regional read copies of Decision 9 [H].
    - During migration the relay and local-disk paths stay as they are; `relay.joinbase.ai` and
      the Bunny relays are not touched.
    - Porting the relay onto R2 as it is would fail. Its journal is one key rewritten by
      compare-and-swap about 3 times per chunk (`relay/core.py:321-346`, `:394-402`), above
      1 write/s, and a 429 there becomes a relay 503 [H from code].
11. **Datasets and Cache.**
    - Public datasets stay behind a custom domain with Cloudflare Cache. Shards are at most
      64 MiB, under the 512 MB cacheable limit below Enterprise. Their extensions are not all
      default-cached, so a Cache Rule ("Cache Everything") on the dataset path is needed [V,
      `cache/concepts/default-cache-behavior`].
    - Checkpoints, states and uploads are not cacheable: 1.485 GB is above 512 MB, and
      presigned URLs do not work on custom domains. They are read with presigned GETs on the
      S3 endpoint.
12. **Replacing Bunny and `relay.joinbase.ai`.**
    1. A TEST run uses `R2ObjectStore` as its challenge store, behind a configuration flag
       (not built).
    2. New runs upload through offers, with a versioned grant and coordinator routes, and their
       relay registry lists no Bunny relay.
    3. Runs that already exist keep their relays until settlement and `CustodyRelease`.
    4. Rollback: start the next run with a relay registry again. Nothing is migrated in place.
    5. The Bunny relays are retired once no unsettled run references them. Their account owner
       is unknown, so this is a founder decision.

## Consequences

- **Measured throughput** [V, opt-in test, 2026-10-26, this host at Cloudflare colo FRA (DE),
  bucket `WEUR`, 64 MiB in 8 parts of 8 MiB with 8 workers, 8 passing runs]:
  - upload 12.0-26.6 MB/s, mean 18.8 MB/s (96-213 Mbit/s);
  - download 28.4-46.9 MB/s, mean 36.7 MB/s.
  - A single stream from the same host to the Cloudflare edge measured 26.3 MB/s up and
    112 MB/s down, so the host's uplink is the likely ceiling [H].
  - At the 18.8 MB/s mean a 2.1 GB trial upload takes about 112 s. Next to a 130 s probe,
    that fits the 297 s window only for the upload leg.
  - With the 1.98 GB download included, a remote host needs about 196 Mbit/s with no margin,
    or 306 Mbit/s with 60 s of margin [H: 32.7 Gbit per trial in the 167 s the probe leaves,
    107 s with the margin]. One sample host is not a fleet: see the measurement plan.
- **What the auditor reads is what the miner committed** [V]. The commit names the sha256, the
  key is that sha256, and every read verifies it, block by block when the sidecar exists.
  R2 refuses part bytes other than the declared part hashes. Whether those parts form the
  committed object is checked by readers only (Decision 4). Storage plays no part in bitwise
  training determinism.
- **Blast radius.** Today the host's API token opens the whole account, and its R2 key opens
  every R2 bucket [V]. Production needs a bucket-scoped `Object Read & Write` R2 token for the
  coordinator only. A leaked presigned URL writes one part of one upload, or reads one object,
  until it expires. `repr()` of an offer hides its URLs.
- **Hash side effect** [V]: the new and changed files under `hypertrain/src/hypertrain/` move
  `network_qualification.sources()`, as any source change does. `implementation_hash`,
  run ids, golden roots and protocol messages are unchanged.
- **Not built here**:
  - wiring the store into the challenge (`Config` reads no R2 setting);
  - a versioned `UploadGrant` that carries offer URLs and signs the part-hash list;
  - the host-side checkpoint adapter;
  - the coordinator's deletes of settled deltas and round inputs;
  - the coordinator's deletes of checkpoint objects: they need a delete on the checkpoint store,
    a persisted record of every round's shard heads and of each link's audit and dispute status,
    and the audit and dispute readers of P1 (ADR 0002, "Retention of checkpoint objects");
  - Local Uploads;
  - regional read buckets;
  - a lifecycle rule, a production bucket or a production token.

### Cost model (monthly, R2 only, tariffs read 2026-10-26)

Tariffs [V, r2/pricing, updated 2026-10-26]:
- Standard storage $0.015/GB-month; Class A $4.50 per million; Class B $0.36 per million.
- Egress is free.
- Free tier per month: 10 GB-month, 1 M Class A and 10 M Class B.
- A 412 or `BadDigest` PutObject is billed as Class A; authentication failures are not billed.

Formulas, per host and day, with an 8 MiB part:
- Class A per object a host uploads through an offer = parts + 5 (sidecar PUT, Create, the
  host's ListParts, the coordinator's ListParts, Complete).
- Class A per object the coordinator stores itself = parts + 4 (sidecar PUT,
  ListMultipartUploads, Create, Complete).
- Class B per read = parts + 2 (HEAD and sidecar GET).

Assumptions [H, every one a placeholder until ADR 0002 and the founder fix them]:
- 144 rounds/day (10 min);
- a 126 MB delta per host-round, which the coordinator reads back once;
- 1.98 GB of round inputs published once per round and read by every host;
- 24 checkpoints of 1.485 GB per host per day, K states held per shard at any time (one shard
  per host at the default capacity of 1), with K = 2 (the last two kept) in the rows below.
  ADR 0002's rule keeps a state until its link is released, so K = 2 holds only if T_hold, the
  time from a link's acceptance to its release, is at most one checkpoint interval [?]. The
  rows are a lower bound for checkpoint storage: the extra cost is hosts x (K - 2) x 1.485 GB x
  $0.015 per GB-month [H, arithmetic]. The 24 per day also disagree with ADR 0002's example
  interval of 10 minutes and with a link at the end of each unit, which write and read more
  checkpoints than the rows count [?];
- one 2.1 GB admission trial per host per day, kept 1 day;
- deltas kept 10.1 h (the TEST `retain_until`) and round inputs 1 day, which needs the
  coordinator deletes of Decision 3;
- auditors re-read 10 % of host-rounds.

| hosts | written TB | read TB | stored GB | Class A M | Class B M | storage $ | Class A $ | Class B $ | **R2 $/month** |
|---|---|---|---|---|---|---|---|---|---|
| 16 (pilot) | 35.4 | 161.1 | 489 | 4.8 | 19.9 | 7.18 | 18.00 | 3.60 | **28.78** |
| 100 (mid) | 176.2 | 1,006.7 | 1,556 | 24.1 | 124.3 | 23.19 | 108.00 | 41.40 | **172.59** |
| 500 (full) | 846.9 | 5,033.6 | 6,641 | 116.3 | 621.4 | 99.46 | 522.00 | 220.32 | **841.78** |

Billable operations are rounded up to the next million after the free tier.

- Operations dominate. With 32 MiB parts (still 4 MiB blocks) the totals fall to 11.68, 67.83
  and 327.34 USD.
- Retention is the other lever: 7 days of hourly checkpoints instead of 2 per host adds 59, 370
  and 1,849 USD/month.
- Workers, Durable Objects and Queues cost 0 because they are not retained. Tunnel is free, and
  Zero Trust is free up to 50 users, with service tokens using no seat.
- Load Balancing (from $5/month) and Argo (from $5/month plus $0.10/GB) are not needed.
- Outside Cloudflare, host egress is probably the largest cost and is not priced here [?]. For
  example, AWS charges for data out of EC2 beyond a 100 GB/month allowance, but the per-GB rate
  did not render on the page read today. That is 35-847 TB/month at the three scales.
- Today's Bunny and relay cost is unknown [?]: no invoice or account owner is on record.
- The test in this batch made about 35 Class A operations per run (9 runs, plus about 50 probe
  requests), well inside the free tier. Its cost is 0.00 USD; it would be under 0.01 USD at list
  price.

## Rejected alternatives

- **4 MiB multipart parts**: R2 answers `EntityTooSmall` at completion (probed).
- **One R2 object per 4 MiB block plus an index** (ht-parts-v1 style): this doubles the Class A
  operations of an 8 MiB plan, a reader needs a URL per block, and a single object cannot be
  served with one presigned GET.
- **boto3 or aioboto3**: absent from both lockfiles. It would bring a dependency, a lock and
  image change and a mypy override, while the repository already signs SigV4.
- **Temporary R2 credentials per host** (one bucket, prefix-scoped, TTL up to 7 days): they
  bind no content hash, need the Cloudflare API token on the coordinator to mint them, and
  per-action scoping is local-signing only. Presigned parts bind each part's bytes.
- **Uploads through a Worker or a Tunnel hostname**: 413 above 100/200 MB below Enterprise.
- **A Durable Object as grant and lease coordinator**: a second source of truth beside the
  coordinator database, with no throughput need at 500 hosts (Decision 7).
- **The relay on an R2 backing as it is**: its journal key exceeds 1 write/s (Decision 10).
- **Templar's pattern** (one bucket per peer, read credentials committed on chain; MIT
  `one-covenant/templar`) [A, Scout]: the published read key covers the whole account. Only the
  idea of unique keys per writer and step is reused; no code was copied.

## Founder decisions

1. Retention per prefix (uploads, checkpoints, round inputs): it drives most of the storage cost.
   For checkpoints the founder fixes T_hold within ADR 0002's rule ("Retention of checkpoint
   objects"): a parent state outlives its link's audit and dispute window, and manifests outlive
   their round's retention.
2. Production bucket creation, location hint and token scopes. Recommended: one bucket per
   environment, a bucket-scoped `Object Read & Write` token for the coordinator only, and
   rotation of today's account-wide token and key.
3. DNS and Tunnel publication of the master API, and Access policies.
4. Paid add-ons (Load Balancing, Argo, Workers Paid if a DO is reopened) and Local Uploads
   (beta).

## Sources (all read 2026-10-26)

- https://developers.cloudflare.com/r2/pricing/ (updated 2026-10-26)
- https://developers.cloudflare.com/r2/platform/limits/ (updated 2026-06-08)
- https://developers.cloudflare.com/r2/objects/upload-objects/ (updated 2026-07-29)
- https://developers.cloudflare.com/r2/api/s3/presigned-urls/ (updated 2026-08-22)
- https://developers.cloudflare.com/r2/api/s3/api/ and `/r2/reference/consistency/` (updated 2026-10-14)
- https://developers.cloudflare.com/r2/api/error-codes/ (updated 2026-07-31)
- https://developers.cloudflare.com/r2/platform/release-notes/ (updated 2026-04-21)
- https://developers.cloudflare.com/r2/buckets/object-lifecycles/ and `/r2/buckets/event-notifications/` (updated 2026-04-21)
- https://developers.cloudflare.com/r2/buckets/local-uploads/ (updated 2026-09-18); changelog 2026-02-03 (open beta)
- https://developers.cloudflare.com/r2/reference/data-location/ (updated 2026-08-19)
- https://developers.cloudflare.com/r2/api/s3/temporary-credentials/ (updated 2026-10-16)
- https://developers.cloudflare.com/r2/api/tokens/ (updated 2026-10-01)
- https://developers.cloudflare.com/workers/platform/limits/ (updated 2026-10-12) and `/workers/platform/pricing/` (updated 2026-10-02)
- https://developers.cloudflare.com/durable-objects/platform/pricing/ (updated 2026-10-18) and `/durable-objects/platform/limits/` (updated 2026-06-01)
- https://developers.cloudflare.com/queues/platform/pricing/ and `/queues/platform/limits/` (updated 2026-04-21)
- https://developers.cloudflare.com/cache/concepts/default-cache-behavior/ (updated 2026-09-14)
- https://developers.cloudflare.com/tunnel/concepts/routing/ (updated 2026-10-16)
- https://www.cloudflare.com/plans/zero-trust-services/ and https://www.cloudflare.com/plans/ (no date)
- https://aws.amazon.com/ec2/pricing/on-demand/ (data-transfer table did not render; 100 GB/month free allowance only)
- Scout benchmark `livrables/scout/2026-10-29_benchmark-entrainement-decentralise.md` and
  `livrables/scout/_work/d_cloudflare.md` [A]; Templar `https://github.com/one-covenant/templar`
  at `0fefc2ad`, MIT [A].
