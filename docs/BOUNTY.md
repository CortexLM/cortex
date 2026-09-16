# Bounty Challenge (live challenge)

Operator reference. Miners start at [`external-miner/bounty.md`](./external-miner/bounty.md).
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
repro / account / hotkey. The public gateway returns **403** on GET/HEAD (and any non-POST) report
reads (POST submit stays on the miner path). Public consumers still hit
CortexLM/backend; this is defense in depth on the ingest list, not a public
board.

| Field | Value |
|-------|--------|
| `challenge_id` | `bounty` |
| `challenge_scoring_version` | `1` |
| Port | `8096` (local host `28096`) |
| Emission | `2000` bps (20%; Proof has the other 80%) |
| Trust-root row | `bounty` @ 2000 bps, `all_metagraph_hotkeys` |

The row is committed in [`../config/challenges.toml`](../config/challenges.toml)
and mirrored by [`../config/challenges.staging.toml`](../config/challenges.staging.toml)
for staging. Both are owner-signed: changing a share without re-signing fails
at load, and `crates/bounty-challenge/tests/trust_root_linkage.rs` asserts both
files stay live, payable, and in step. Do not edit them by hand — see
[`../config/CEREMONY.md`](../config/CEREMONY.md) and
[`runbooks/trust-root-rotation.md`](runbooks/trust-root-rotation.md).

## Operator environment

Set on the **master** host (never baked into git; see
[`../deploy/env/bounty-challenge.env.example`](../deploy/env/bounty-challenge.env.example)):

| Variable | Role |
|----------|------|
| `BOUNTY_BACKEND_PUBLIC_URL` | Base URL of the CortexLM/backend public API. **The only scorer.** Unset → ingest 503s and every leaf is a cover. |
| `BOUNTY_EMIT_POLL_SECS` | Seconds between emitter ticks (default 120). |
| `BASE_CHALLENGE_SK_FILE` | Bounty leaf mini-secret. Its public key **must** match the trust-root `bounty` row, or every leaf is rejected. |
| `BASE_CHALLENGE_GATEWAY_ENDPOINT` | Master gateway for `POST /v1/weights/raw` (default `http://gateway:8080`). |
| `BASE_NETUID` | Subnet `E` is derived from. |
| `BASE_CHAIN_ENDPOINT(S)` | Chain for `E`; `BASE_CHAIN_ENDPOINTS` is the ordered failover list and wins over the singular. |
| `BOUNTY_ADMIN_TOKENS_FILE` | Operator bearer for `POST /v1/admin/adjudicate` and the report reads. Empty → admin **503**. |
| `BOUNTY_SESSION_SECRET_FILE` | Pairing session HMAC. Empty → ephemeral (pairings do not survive restart). |
| `BOUNTY_CHAT_COMMAND` | Chat inject command. Env-only; docs use the placeholder. |

`BOUNTY_FORCE_SIM` is retired: it is ignored, warned about at boot, and
`deploy/scripts/assert-compose-matrix.sh` fails if any compose file sets it.

### What validators do and do not verify

Validators **never** read the bounty feed and **never** re-run a report. They
verify the sealed bundle: the gateway signature, the merkle root, D24
completeness against the local owner-signed trust root, and the recomputed
weight vector. So the feed → leaf → seal path is entirely on the challenge
host, and a bug there is invisible to consensus until someone reads
`/v1/status` or the sealed vector.

That is why the emitter publishes its outcome: the reward linkage has two
halves that fail independently — the backend publishing adjudications, and this
host turning them into signed leaves. `can_score` covers only the first.

## Why bug reports need different evaluation

Proof's research evaluation protects private holdouts against memorization.
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
   → `NotAttempted`)
4. `POST /v1/weights/raw` on the gateway, which seals what validators fetch

**A reachable feed is not a paying one**, and the difference is a separate
outcome. When the feed answers and no row maps to payable weight — nothing
adjudicated, everything still `pending`, or a `valid` row the operator never
priced — the tick is `unpaid`: `E` is covered with
`NoScore(ChallengeInternal)`, the share burns to uid 0, and `/v1/status`
reports `last_outcome: "unpaid"` with `last_feed_read: true`. It is not signed
as a scored epoch, because `NotAttempted` claims the challenge *chose* not to
invoke the miner, which is false here and would report a healthy tick while
paying nobody.

Adjudication is therefore a hard dependency of the reward path, not a
reporting one. A published row only becomes weight when it is `valid`, carries
a `severity`, and is justified; anything short of that pays nothing that epoch.

Operator knobs: `BASE_CHALLENGE_GATEWAY_ENDPOINT`, `BASE_NETUID`,
`BASE_CHAIN_ENDPOINT(S)`, `BOUNTY_EMIT_POLL_SECS` (default 120s). Re-emitting
the current epoch is normal: the gateway supersedes on a changed digest and
409s an identical one.

### Reading `/v1/status`

| Field | Meaning |
|-------|---------|
| `scoring_backend` | `backend_public` or `unconfigured`. |
| `can_score` | Whether this host *may* turn a report into weight. |
| `emitter_wired` | Whether a leaf emitter was wired. `false` is the only condition that also 409s the seal (missing `BASE_CHALLENGE_SK_FILE`). |
| `emitter.last_outcome` | `never` / `scored` / `unpaid` / `burned` / `held` / `error`. |
| `emitter.last_feed_read` | Whether the feed answered on the last tick. |
| `emitter.last_paid` | Positive leaves on the last tick. |
| `emitter.last_reason` | Why nobody was paid, or why the epoch was held. |
| `emitter.scored_epoch` | Highest epoch this process scored (in-process). |

`can_score` alone does not mean anyone is being paid. `unpaid` with
`last_feed_read: true` is the combination to watch: the backend is up and the
epoch still pays nobody, which means adjudication is behind.

## Fail-closed ingest and emission

`GET /v1/status` publishes `scoring_backend` (`backend_public` |
`unconfigured`), `backend_public_configured`, `can_score`, `emitter_wired`,
and the `emitter` block.

Without `BOUNTY_BACKEND_PUBLIC_URL` — or when the feed is unreachable, 5xx,
unparseable, or moving under the read — the host cannot turn a report into
weight. Two things follow, and neither is a degraded mode:

- `POST /v1/reports` answers **503** without storing anything. Accepting
  reports there would take real work (finding a real bug) and pay nothing.
- the emitter pays **nobody**: it covers `E` with
  `NoScore(ChallengeInternal)` (`BUNDLE_SPEC` §3.3.1 — "challenge-side fault;
  still must cover the participant"), so the 2000 bps burns to uid 0.

A feed that answers and crowns nobody is the third case, and it is treated as
a cover rather than a score. `NotAttempted` on every leaf would claim the
challenge chose not to invoke the miners, which is false, and it would seal as
a legitimate-looking unpaid epoch while `/v1/status` reported success. The
tick is `unpaid` instead: same `ChallengeInternal` cover, same burn, and the
status says which half is missing. The three outcomes are deliberately
distinct — `scored`, `unpaid` (feed read, nothing payable), `burned` (feed
unreadable) — because an operator needs to know whether to wait on the backend
or go look at adjudication.

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

A tick that produces no weight also tries not to take back a score. Once the
process has scored an epoch, a later unproductive tick inside that same epoch
**holds** instead of superseding a champion's leaf with a cover — whether the
feed went down or stayed up and simply stopped publishing the crowned hotkey.
A backend hiccup, or a publish that reverts, does not get to decide the epoch.
The watermark is in-process (the gateway has no read side for raw leaves), so a
restart during an outage can still burn an epoch that had scores — the next
successful tick supersedes it back. The bias is deliberate: burning pays nobody
who was not already paid, while staying silent would 409 the seal for every
challenge. Independently, the gateway refuses a `ChallengeInternal` cover from
replacing a positive leaf for the same key (409, original kept), so a lost
watermark cannot reseal a paid allocation into a uid-0 burn.

Only a missing `BASE_CHALLENGE_SK_FILE` stops emission entirely — a leaf the
trust root rejects is not weight — and that case is logged as the 409 it will
cause. It is also the only case that publishes `emitter_wired: false`.

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
