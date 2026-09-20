# Bounty operator reference

Bounty rewards useful vulnerability reports about the CortexLM backend. It is
up to 30% of subnet emission after
[algorithm 2 activation](how-to/trust-root.md#activate-proportional-bounty).
The legacy owner-signed profile retains its 20% allocation and algorithm 1.
Initial production pairing, intake and adjudication live
in CortexLM/backend. The Python subnet retains the compatibility intake below
and emits signed leaves, but does not export those local rows; the external
CortexLM/backend public feed is the sole scoring source.

## Pairing and intake

Before pairing, an operator verifies control of the named Cortex Chat account
out of band and creates a one-use authorization with authenticated
`POST /v1/admin/pair-grants`:

```json
{
  "account_id": "dedicated-mining-account",
  "hotkey": "5F...",
  "expires_at": 1800000300
}
```

The expiry must be in the future and no more than 300 seconds from the
operator's current time. The grant binds that exact account and hotkey. It is
stored in SQLite and consumed atomically only when pairing succeeds; an
expired or absent grant returns 403 without consuming the miner nonce.

The miner then signs the exact UTF-8 payload
`cortex-bounty-v1|<account_id>|<nonce>|<expiry>` with its Bittensor hotkey. The
nonce is 16 to 64 hexadecimal characters and is single-use across pairings.
`POST /v1/pair` also requires explicit terms acceptance. A successful response
returns an opaque session token; only its hash is stored.

The store consumes the nonce, grant and session change in one transaction.
Reusing an accepted nonce returns `409 nonce reused`, including an identical
signed request; the refusal returns no session data and does not consume a
new grant. This protection survives restarts and concurrent requests.
If a successful HTTP response or session token is lost, issue a fresh grant
and pair with a new nonce. Successful replacement revokes the previous session.

Pairing a CortexLM account again revokes the previous session for that account.
An operator grant may replace its hotkey; the old session cannot follow that
replacement. `POST /v1/reports` accepts the session, title, report body and
reproduction steps. The optional hotkey field, when present, must match the
currently paired hotkey.

Intake is transactional SQLite. Before storing a report it enforces:

- a readable and internally consistent external scoring feed;
- at most five pending reports per hotkey;
- at least 60 seconds since that hotkey's previous report;
- at most one in-flight feed validation per hotkey;
- a nonempty title distinct from the body after normalization;
- at least 80 body characters, 20 reproduction characters and four distinct
  evidence tokens;
- one canonical title/body fingerprint, so a duplicate never consumes a new
  triage slot.

Pairing and operator-adjudication JSON bodies are limited to 4096 bytes. Report
request bodies are limited to 262144 bytes. The service stops reading at the
limit and returns `413`, before signature, session or feed processing.

An unavailable or malformed feed returns 503 and creates no row. There is no
local or simulated production scorer.

## Adjudication

Operator-authenticated routes list reports and apply one of four verdicts:
`valid`, `already_fixed_not_prod`, `invalid_malicious`, or `duplicate`.
`valid` requires a severity (`trivial`, `minor`, `major`, or `critical`) before
it can earn credit. `duplicate` requires the original report ID. Operator
bearer values are compared through stored SHA-256 hashes and never returned.

These local rows support the triage workflow. They do not become weights until
the CortexLM/backend publishes the corresponding public reports and leaderboard.

## Public-feed consistency

For each scoring read, Cortex fetches:

- `/v1/bounty/public/status` to pin one immutable publication revision and its
  aggregate counters;
- `/v1/bounty/public/leaderboard?revision=<revision>`;
- every `/v1/bounty/public/reports` page for that same revision, following the
  opaque cursor until `has_more` is false.

Each response is bounded to 8 MiB and the complete report snapshot to 64 MiB.
Cortex requires API version 1, available adjudication, no unpriced valid report,
one revision across every response, complete pagination, unique report IDs and
leaderboard hotkeys, nonempty evidence, duplicate chains ending at a
non-duplicate report, and exact
agreement between status, leaderboard `valid_count` and the published reports.
A truncated leaderboard is accepted only when it is an exact ranked prefix;
Cortex reconstructs the complete ranking from the fully paginated reports.
Moving revisions, truncated report pagination or any inconsistency fail closed.
Transient transport, HTTP, JSON or revision errors receive at most three
read-only attempts inside one 30-second deadline; stable adjudication, pricing
and backlog gates fail immediately. A nonempty adjudication backlog with no
published report is treated as an unavailable scorer, not as a zero score.

## Score

Algorithm 2 signs each expected hotkey's exact count of `valid` reports as its
raw score. One valid report is one point regardless of severity. There is no
champion, precision gate, minimum author count or triage-noise gate. Invalid,
duplicate and already-fixed reports contribute zero points. Severity remains
required evidence for a valid publication, with no effect on its point value.

Let `n_i` be author i's valid count and `N = sum(n_i)` over the epoch's expected
participants, selected by the owner-signed policy and sealed metagraph:

```text
Bounty payout = 0.30 * min(N / 10, 1)
author i payout = 0.30 * n_i / max(10, N)
```

Five valid reports distribute 15% of subnet emission; ten or more distribute
30%, proportionally across authors. The remaining Bounty mass burns to UID0;
it never increases Proof's 70%. UID0 and unmapped author allocations also burn
without increasing other authors' allocations. Existing owner/permit submission
constraints are unchanged.

Counts use the complete cumulative report history in one pinned external
publication, with no epoch reset or new rolling window. Only expected hotkeys
enter `N`; historical authors outside the metagraph/policy are excluded.
Validated report IDs and rooted duplicate chains prevent duplicate credit.
An author with zero valid reports gets `NotAttempted`. Feed failure produces
`ChallengeInternal` for every expected participant and burns the Bounty share.

The legacy 2,000/8,000 owner profile retains algorithm 1, including its champion,
precision/severity scoring and signed encodings. A 3,000/7,000 owner profile
requires challenge-document version >=2 and algorithm 2. An algorithm 1 body
under that profile is rejected, even with a valid gateway signature.

## Operational checks

`GET /v1/status` reports `can_score` from a bounded feed probe. Successful
probes are shared for 15 seconds, failures for 5 seconds, and concurrent refresh
requests fail fast instead of multiplying full snapshot reads. Report intake
still performs an uncached feed read before storage. The response also includes
the bounded reason, operator-grant requirement, scoring constants, quotas and terms.
Before opening intake, verify that `can_score` is true, grant and pair a test
hotkey, submit a substantive report, adjudicate it through the operator route,
and confirm the public backend publishes a stable matching snapshot. The full
payment path still requires raw leaves, an immutable gateway seal, independent
validator recomputation and an on-chain submission.

Miner-facing request examples live in [the Bounty guide](external-miner/bounty.md).
