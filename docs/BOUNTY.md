# Bounty Challenge (live challenge)

Control-plane notes. Miners start at [`external-miner/bounty.md`](./external-miner/bounty.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

**Public transparency lives in CortexLM/backend.** This subnet **reads**
`GET {BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/leaderboard` and
`GET {BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/reports`. It does not
serve `/v1/public/*` (or any unauthenticated public leaderboard). Public
consumers hit the backend. Never bake a host into git.

Internal ingest (pair / reports / adjudicate) stays on this service so
Chat can bind a hotkey. Scoring maps hotkey → lattice from published
backend rows that carry `problem_found`, `justification`, and — for a
creditable `valid` — a `severity`.

| Field | Value |
|-------|--------|
| `challenge_id` | `bounty` |
| `challenge_scoring_version` | `1` |
| Port | `8096` (local host `28096`) |
| Emission | `3000` bps |

## Why this challenge is not gated like the Relearn ones

The LLM challenges defend a private holdout against a model that memorised it.
Bounty has no model and no holdout to memorise; its scarce resource is
**adjudication**, and its failure modes are volume plays against a human or
agent triage queue. Copying the LLM gate stack here would gate the wrong thing.
The three real attacks and their answers:

| Attack | Answer |
|--------|--------|
| Flood the queue with junk and keep whatever sticks | Precision is `valid / (valid + malicious)`, so junk subtracts. A net-negative miner burns toward uid 0, and ingest quotas cap the queue one hotkey can occupy |
| File many real but worthless bugs | Pay is precision **×** impact, where impact is the operator severity. Forty cosmetic findings are worth a fraction of four critical ones |
| Split one finding across reports, or re-file known ones | Duplicates and already-fixed re-files earn nothing, and their share of a miner's adjudications is a **canary kept off the paid number** |

## Adjudication and scoring

Miners pair a Bittensor hotkey to a dedicated Cortex Chat account, then file
bug reports. Every report is tagged with that hotkey. Operators adjudicate:

| Verdict | Weight |
|---------|--------|
| `valid` + `severity` | reward, scaled by severity (`trivial` 625 → `critical` 10000 bps) |
| `valid` without `severity` | **not creditable** and blocks the crown — an unpriced bug cannot be paid for |
| `already_fixed_not_prod` | ack only — no reward, no penalty, counts as triage noise |
| `invalid_malicious` | penalty (burn toward uid 0) |
| `duplicate` | no reward, no penalty, counts as triage noise |

Evidence is required to be **paid**, never to be **penalized**. A malicious row
always counts against the miner who filed it, with or without a severity;
that asymmetry is deliberate, or forgetting a field would be an escape hatch.

Champion is displacement vs the previous bounty champion on precision, subject
to a `MIN_PRECISION_BPS` floor so beating a sloppy incumbent is not enough.
Validators do not evaluate reports; they verify sealed bundles. Unmatched
emission burns to uid 0.

**The published leaderboard is informational.** It only breaks ties in the walk
order. A hotkey that tops `valid_count` with no adjudicated, justified reports
has no tallies, is never judged, and is paid nothing.

## Off the visible score

`triage_noise_bps` — the share of a miner's adjudications that were duplicates
or already-fixed re-files — is reported on the verdict and is **not** in the
lattice. Precision cannot see duplicates at all, so without this a miner could
re-file the same finding indefinitely at zero visible cost while consuming the
triage capacity the whole challenge runs on. Above `MAX_TRIAGE_NOISE_BPS` it is
a hard zero, and because it is absent from the paid number a miner tuning
precision cannot tune it away.

## Fail-closed ingest

Scoring needs the backend feed. `GET /v1/status` publishes `scoring_backend`
(`backend_public` | `sim` | `unconfigured`), `force_sim`, and `can_score`.

With neither `BOUNTY_BACKEND_PUBLIC_URL` nor `BOUNTY_FORCE_SIM=1`, the host
cannot turn a report into weight, and `POST /v1/reports` answers **503**
without storing anything. Accepting reports there would take real work —
finding a real bug — and pay nothing for it. `BOUNTY_FORCE_SIM=1` selects
local adjudications only; it is CI/local, reported on `/v1/status`, and
`deploy/scripts/assert-compose-matrix.sh` fails if a staging or prod overlay
enables it.

Ingest quotas, all per hotkey and all published on `/v1/status`: at most 5
reports awaiting adjudication, one report per 60s, an 80-character body, and a
20-character reproduction. Over the cap is `429`; too thin is `400`. Neither
records anything against the miner — the report is fine, the queue is not.

Chat inject is env-only (`BOUNTY_CHAT_COMMAND`). Docs and examples use the
placeholder `<BOUNTY_CHAT_COMMAND>` only. Never commit the live token.
Optional `X-Lium-Api-Key` is accepted and never logged.
