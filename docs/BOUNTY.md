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

`GET /v1/reports` and `GET /v1/reports/{id}` are operator-local: same bearer
as `POST /v1/admin/adjudicate`. Empty admin hashes → **503**
`auth_unconfigured`. Missing/wrong bearer → **401** and no report body /
repro / account / hotkey. The public gateway returns **403** on those GET
reads (POST submit stays on the miner path). Public consumers still hit
CortexLM/backend; this is defense in depth on the ingest list, not a public
board.

| Field | Value |
|-------|--------|
| `challenge_id` | `bounty` |
| `challenge_scoring_version` | `1` |
| Port | `8096` (local host `28096`) |
| Emission | `7000` bps |

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

## From a published row to a validator's weight

The backend feed is not a dashboard this subnet reads for colour — it is the
scorer. Each tick the challenge service:

1. `GET {BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/leaderboard` + `/reports`,
   re-read until two consecutive reads agree — the two routes are separate
   GETs, and a publish landing between them would mix revisions
2. derives `E` from the metagraph at `last_epoch_block` (`AllMetagraphHotkeys`)
3. maps published rows onto one leaf per hotkey in `E` for the current subnet
   epoch (champion → `Score`, net-malicious → `InvalidResponse`, everyone else
   → `NotAttempted`); an unreadable feed pays nobody but still covers `E` with
   `ChallengeInternal`
4. `POST /v1/weights/raw` on the gateway, which seals what validators fetch

Operator knobs: `BASE_CHALLENGE_GATEWAY_ENDPOINT`, `BASE_NETUID`,
`BASE_CHAIN_ENDPOINT(S)`, `BOUNTY_EMIT_POLL_SECS` (default 120s). Re-emitting
the current epoch is normal: the gateway supersedes on a changed digest and
409s an identical one.

## Fail-closed ingest and emission

`GET /v1/status` publishes `scoring_backend` (`backend_public` |
`unconfigured`), `backend_public_configured`, and `can_score`.

Without `BOUNTY_BACKEND_PUBLIC_URL` — or when the feed is unreachable, 5xx,
unparseable, or moving under the read — the host cannot turn a report into
weight. Two things follow, and neither is a degraded mode:

- `POST /v1/reports` answers **503** without storing anything. Accepting
  reports there would take real work (finding a real bug) and pay nothing.
- the emitter pays **nobody**: it covers `E` with
  `NoScore(ChallengeInternal)` (`BUNDLE_SPEC` §3.3.1 — "challenge-side fault;
  still must cover the participant"), so the 7000 bps burns to uid 0.

Covering `E` is not a hedge, it is the difference between bounty failing and
the subnet failing. Bounty holds a **paid** trust-root row, and D24 requires a
leaf per participant for every paid challenge: leave `E` uncovered and
`POST /v1/admin/seal` answers **409 incomplete_participant_set** for the whole
bundle, so proof's weights go unsealed too. The emitter therefore runs even
on a host with no feed at all — it simply never pays.

"Moving under the read" is in that list for the same reason. The feed is two
routes, and a mixed pair is worse than no pair: every tally comes from
`/reports`, so a stale half can under-count a miner's valid rows or drop it to
`NotAttempted`, and `/leaderboard` decides the champion walk order — a verdict
the backend never published either way. Each tick therefore re-reads the pair
until two consecutive reads agree (bounded, so a settling feed is a retry
rather than a lost epoch) and compares the *parsed* snapshot, so a field the
public DTO does not model cannot make a still feed look like a moving one. Two
equal composites are still refused when `/leaderboard` `valid_count` does not
match the `valid` rows on `/reports`: a feed that always serves revision A on
one route and revision B on the other is stable under re-read and must not be
signed. A feed that never holds still, or whose halves disagree, is an error,
and the paragraphs above apply. No backend change is needed for this; a
published revision or ETag on both routes would let the moving-feed check
collapse to a single round.

A failed tick also tries not to take back a score. Once the process has scored
an epoch, a feed outage inside that same epoch **holds** instead of superseding
a champion's leaf with a burn; a backend hiccup does not get to decide the
epoch. The watermark is in-process (the gateway has no read side for raw
leaves), so a restart during an outage can still burn an epoch that had
scores — the next successful tick supersedes it back. The bias is deliberate:
burning pays nobody who was not already paid, while staying silent would 409
the seal for every challenge.

Only a missing `BASE_CHALLENGE_SK_FILE` stops emission entirely — a leaf the
trust root rejects is not weight — and that case is logged as the 409 it will
cause.

**There is no offline scorer.** `BOUNTY_FORCE_SIM` is retired: it is ignored,
warned about at boot, and `deploy/scripts/assert-compose-matrix.sh` fails if
any compose file sets it. A local stand-in here would pay miners on
adjudications no validator could reproduce, which is exactly what the sealed
bundle exists to prevent. To exercise scoring locally, point
`BOUNTY_BACKEND_PUBLIC_URL` at a stand-in backend that serves the two public
routes.

Ingest quotas, all per hotkey and all published on `/v1/status`: at most 5
reports awaiting adjudication, one report per 60s, an 80-character body, a
20-character reproduction, and at least four distinct body tokens. Title and
body must differ after whitespace collapse. Over the cap is `429`; too thin
is `400`. Neither records anything against the miner — the report is fine,
the queue is not.

The same title+body fingerprint, after case and whitespace collapse, is
always a `duplicate` — including when the original was already closed as
invalid or already-fixed. Re-filing the same text after a reject does not
open a new triage slot.

Chat inject is env-only (`BOUNTY_CHAT_COMMAND`). Docs and examples use the
placeholder `<BOUNTY_CHAT_COMMAND>` only. Never commit the live token.
Optional `X-Lium-Api-Key` is accepted and never logged.
