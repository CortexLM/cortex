# S5 / D20 plan (written 15:59Z, revised at the blocker)

| # | Step | Owner | Writes | Rollback |
|---|---|---|---|---|
| 1 | Read the precedents and inspect the s100 TEST stack and host B, read-only | S5 | none | n/a |
| 2 | Build the B300 TEST manifest and disposable keys offline; dry-run validate with the repo tools | S5 | `/tmp/s5-d20/` only | delete the folder |
| 3 | Publish the run on `hypertrain-test-challenge`; snapshot `/data/hypertrain-test/state` before and after | S5, outside a seal window | s100 TEST stack | **NOT DONE: BLOCKER** (REPORT.md): the image cannot ingest a 148-SM qualification and s100 has no GPU for references |
| 4 | Miner on host B: `--rm` GPU container, join-v2 → probation → run-v2 → upload | S5 | `--rm` container | `docker rm -f`, then check that only the image is left |
| 5 | Audits MATCH and `get_weights` 200 | TEST auditors | none on prod | n/a: no TEST auditor exists |
| 6 | Clean host B: only the image may remain | S5 | none | n/a |
| 7 | Commit the tools and docs, then REPORT.md | S5 | git | revert |

Stop conditions from the brief: any impact on production, any need for a production key, a
TEST-stack change that cannot be rolled back, a seal window in progress. Item 3 hit "the
image needs a swap", so it stops there.

What remains doable: steps 1, 2 and 6, plus a read-only network probe from host B to the s100
TEST relay and challenge, timed.

## Revision 15:58Z: founder/Forge chose B

Order: (1) the offline TEST manifest and disposable keys, validated; (2) on host B, a `--rm`
container running an in-process TEST challenge and relay with a 148-SM B300 CUDA manifest, the
`8a90d9bc` source mounted read-only and GPU training inside run-v2, through audits and
`get_weights`; (3) clean B and write the REPORT. Hard stop at 16:50Z. Nothing is written to s100.
Rollback for (2): the container is `--rm`; `~/s5d20` on B is deleted at the end.
