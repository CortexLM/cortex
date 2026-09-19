# Bounty operator reference

Bounty rewards useful vulnerability reports about the CortexLM backend. It is
20% of subnet emission. The subnet accepts reports and emits signed leaves, but
the external CortexLM/backend public feed is the sole scoring source.

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
The session cannot be moved to another hotkey. `POST /v1/reports` accepts the
session, title, report body and reproduction steps. The optional hotkey field,
when present, must match the paired hotkey.

Intake is transactional SQLite. Before storing a report it enforces:

- a readable and internally consistent external scoring feed;
- at most five pending reports per hotkey;
- at least 60 seconds since that hotkey's previous report;
- a nonempty title distinct from the body after normalization;
- at least 80 body characters, 20 reproduction characters and four distinct
  evidence tokens;
- one canonical title/body fingerprint, so a duplicate never consumes a new
  triage slot.

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

- `/v1/bounty/public/leaderboard`;
- `/v1/bounty/public/reports`.

Both responses are bounded to 8 MiB. Cortex requires two consecutive identical
parsed snapshots, matching publication tokens when both routes provide them,
unique IDs and hotkeys, and exact agreement between leaderboard `valid_count`
and the valid reports. Moving, mixed or invalid snapshots fail closed.

## Score

Scoring uses integer arithmetic only. A contender needs at least three decided
valid/malicious reports, nonnegative net credit, no unpriced valid report, at
least 60% precision, no more than 50% duplicate/already-fixed triage noise, and
strictly better precision than the current champion. Severity weights are 6.25%,
25%, 50% and 100% for trivial through critical.

An eligible champion receives:

```text
1_000_000 * precision_bps * average_severity_bps / 100_000_000
```

Only one hotkey is champion for the snapshot. A miner with negative net credit
gets `InvalidResponse`; other expected hotkeys get `NotAttempted`. Feed failure
produces `ChallengeInternal` for every expected participant, so the Bounty mass
burns to UID 0 while the bundle remains complete.

## Operational checks

`GET /v1/status` actively probes the feed and reports `can_score`, a bounded
reason, the operator-grant requirement, scoring constants, quotas and terms.
Before opening intake, verify that `can_score` is true, grant and pair a test
hotkey, submit a substantive report, adjudicate it through the operator route,
and confirm the public backend publishes a stable matching snapshot. The full
payment path still requires raw leaves, an immutable gateway seal, independent
validator recomputation and an on-chain submission.

Miner-facing request examples live in [the Bounty guide](external-miner/bounty.md).
