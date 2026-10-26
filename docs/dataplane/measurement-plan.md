# Multi-region R2 throughput: measurement plan

Plan only. Nothing here was run by batch `v3-dataplane-cloudflare` beyond the single-host
baseline below. It answers the open question of [ADR 0003](../adr/0003-cloudflare-data-plane.md):
can hosts in every region move a trial (about 2.1 GB up, 1.98 GB down) and a round inside the
297 s window, and where should buckets live? Cloudflare publishes no R2 bandwidth figure per
connection or region (vendor pages read 2026-10-26), so these numbers must be measured.

## Baseline (measured 2026-10-26)

One host at Cloudflare colo FRA (DE), bucket `WEUR`, 64 MiB object, 8 MiB parts, 8 workers,
`hypertrain/tests/storage/test_r2_live.py`, 8 passing runs:

- upload 12.0-26.6 MB/s;
- download 28.4-46.9 MB/s;
- single stream to the Cloudflare edge: 26.3 MB/s up, 112 MB/s down.

## Questions

1. Upload and download MB/s from each host region to each bucket location, with and without
   Local Uploads.
2. How throughput scales with parallel parts (workers) and part size, and where it saturates:
   the host link, R2, or 429/503 throttling.
3. How long until an object written in one region is readable at full speed from another
   (Local Uploads replicates asynchronously).
4. Whether a trial fits the 297 s window from every region with a 60 s margin. A trial is a
   2.1 GB upload plus a 1.98 GB download next to a ~130 s compute probe. Also whether a 126 MB
   delta fits the round's upload window.

## Matrix

| Factor | Values |
| --- | --- |
| Host region | the B300 region in use (`ap-southeast-3` today), one EU and one US region; a CPU instance is enough, as long as its network class matches the B300 host or is recorded |
| Bucket location hint | `apac`, `weur`, `enam` (one `cortex-v3-test-` bucket each) |
| Local Uploads | off, on (open beta; not on jurisdiction buckets) |
| Workers | 4, 8, 16, 32 |
| Part size | 8, 16, 32 MiB (whole 4 MiB blocks) |
| Object | 126 MB (a delta), 1.485 GB (a state) |
| Repetitions | 5 per cell, at two times of day |

The full product is 3 x 3 x 2 x 4 x 3 x 2 = 432 cells. Run the 1.485 GB object only on the
8 and 16 worker rows and the 8 and 32 MiB parts. Run the 126 MB object on every row.

## Method

- Extend the opt-in live test into a parametrised script that reuses `R2ObjectStore.put_file`
  and `fetch_to`, in the same `cortex-v3-test-` hygiene: random prefix, everything deleted
  at the end, credentials by path only.
- Record per run:
  - wall time and MB/s for the upload and the download;
  - the 429, 5xx and retry counts;
  - the colo from `/cdn-cgi/trace`;
  - the host's single-stream edge speed (`speed.cloudflare.com`), measured just before, so
    that the host link can be separated from R2.
- Cross-region visibility: write in region X, poll a ranged GET from region Y every 250 ms,
  and record the time to the first 206 and to a full-speed read.
- Report p50 and p95 per cell. Flag every cell where the 2.1 GB upload plus the 1.98 GB
  download takes more than 107 s: 297 s, minus the 130 s probe, minus a 60 s margin. That is
  at least 306 Mbit/s for the 32.7 Gbit of a trial.

## Cost and approvals

- R2: about 1.5 TB written and 1.5 TB read in total, about 200 k Class A and 400 k Class B
  operations, with storage held for hours only.
  - Inside the monthly free tier (1 M Class A, 10 M Class B) this is 0 USD.
  - At list price it is about 1.04 USD, and up to 4.86 USD with per-million rounding
    (r2/pricing, 2026-10-26).
- Hosts: provider egress of about 1.5 TB is the main cost, and AWS's per-GB rate is not known
  here [?]. Each region's instance hours also count. Both need a ledger line through Pilote,
  and Principal SN100 approval if they exceed 50 USD per decision.
- Local Uploads is enabled per bucket from the dashboard or Wrangler, on test buckets only.

## When

During P3, after ADR 0003 is accepted. Ideally run it on the B300 hosts of the next test
window: their network is the one that matters.
