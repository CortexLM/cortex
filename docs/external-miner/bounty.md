<!-- protocol_version: 1 -->

# Bounty miner guide

Bounty (`bounty`, up to 3000 bps under algorithm 2) accepts reproducible Cortex
product and backend bug reports associated with your Bittensor hotkey. Install the [Python CLI](README.md)
and obtain the gateway URL from the subnet operator.

The initial production miner flow pairs and files reports in CortexLM/backend.
The Python gateway routes documented here remain compatibility/operator-test
surfaces; they do not publish a local report into the backend and cannot make it
creditable by themselves. Only a report published by the backend public feed can
earn weight.

## Check scoring availability

```bash
curl --fail-with-body "$GATEWAY/challenge/bounty/v1/status"
```

The response publishes research terms, quotas and `scoring_backend`.
`can_score` reflects a recent validated external-feed snapshot: successful
status probes are shared for up to 15 seconds and failures for up to 5 seconds.
Concurrent refreshes fail closed instead of multiplying full snapshot reads.
Every report POST performs its own uncached feed check, so an outage or
inconsistent publication after a successful status check can still return `503`.

## Pair a dedicated account

Use a dedicated Cortex Chat mining account. Read these terms before passing
`--accept-terms`:

> By pairing a Bittensor hotkey to a Cortex Chat account for Bounty Challenge,
> you accept that this dedicated mining account, its logs, and its conversations
> may be used for research, to fix product and backend bugs, and to remunerate
> (or penalize) the bound miner hotkey. Do not pair a private personal account.

Ask the subnet operator to verify that you control this account and authorize
the exact account/hotkey pair. That one-use authorization lasts at most five
minutes. Pairing without it, or after it expires, returns `403 pairing not
authorized by account operator`; request a fresh authorization and reuse your
unspent nonce.

```bash
uv run cortex miner --gateway "$GATEWAY" \
  --wallet-name research --wallet-hotkey miner \
  bounty-pair --account-id "$CORTEX_ACCOUNT_ID" \
  --accept-terms --session-file ./bounty-session
```

For an encrypted hotkey add `--wallet-password-file /private/hotkey-password`
before `bounty-pair`. The wallet coldkey secret and a Proof public key are not
required. Wallet options and the development-only `--dev-seed-file` path are
described in the [mining index](README.md#usage).

The CLI signs locally and posts to `/challenge/bounty/v1/pair`. It writes the
returned session to the specified new file with mode `0600`, refuses to
overwrite an existing file, and prints only `paired` and the public hotkey.
There is no automatic session cache or CLI-driven Chat activation flow in
this Python implementation. A session is a secret bearer credential.

The pair API accepts `account_id`, `hotkey` (SS58 or public-key hex), `nonce`,
`exp`, `signature` and `terms_accepted: true`. The exact UTF-8 signing payload
is:

```text
cortex-bounty-v1|{account_id}|{nonce}|{exp}
```

Use sr25519's **Substrate** signing context. This is different from the Cortex
context used by Proof and bundle signatures. The CLI supplies a random
32-character hexadecimal nonce and an expiry five minutes in the future.
Pairing requires explicit terms acceptance (`403` otherwise), a valid signature
and an unexpired signing window, plus the operator authorization above.
Successful pairing consumes the authorization and nonce and returns `201` with
session metadata. A nonce is single-use: retrying an accepted signed request
returns `409 nonce reused` with no session data, including after a restart.
This refusal does not consume a fresh operator authorization. If the response
or session file is lost, request a new authorization and pair with a new nonce.
Pairing the same account again revokes its previous session, which then returns
`401 invalid_session`, including an operator-authorized hotkey replacement.
Sessions do not currently expire automatically by age.
The complete pairing JSON body is limited to 4096 bytes; larger requests return
`413 pair request too large` before signature processing.

## Submit a report

```bash
uv run cortex miner --gateway "$GATEWAY" \
  --wallet-name research --wallet-hotkey miner \
  bounty-report --session-file ./bounty-session \
  --title "Reproducible failure in the research upload path" \
  --body-file report.md --repro-file reproduction.md
```

The request goes to `/challenge/bounty/v1/reports`. Include enough distinct
evidence to reproduce the problem and explain its impact. The API accepts
`session`, optional `hotkey`, `title`, `body` and `repro_steps`; the CLI includes
its selected hotkey. A supplied hotkey must match the session (`403` on
mismatch). The API validates substance before inserting a report.

| Limit | Value |
|-------|-------|
| Reports awaiting adjudication per hotkey | 5 |
| Minimum interval between reports | 60 seconds |
| Concurrent feed validations per hotkey | 1 |
| Minimum body length | 80 characters |
| Minimum reproduction length | 20 characters |
| Maximum complete report request body | 262144 bytes |

An empty or repeated title/body and low-substance repeated-token content
return `400`. A quota violation or concurrent validation returns `429`. Framework schema failures may
return `422`; an oversized request returns `413`. Successful intake returns
`201` with `id`, `miner_hotkey`,
`state` and `fingerprint`; acceptance into triage is not a reward.

Report reads (`GET /v1/reports` and `/v1/reports/{id}`) and
`POST /v1/admin/adjudicate` require operator bearer authentication. The public
gateway denies report reads. There is no public `bounty show` command.

## Scoring and adjudication

Cortex **reads** the CortexLM/backend public API configured by
`BOUNTY_BACKEND_PUBLIC_URL`. The required backend routes are
`/v1/bounty/public/status`, `/v1/bounty/public/leaderboard` and
`/v1/bounty/public/reports`. Status pins an immutable revision; Cortex requests
the leaderboard and every cursor-paginated report page for exactly that
revision. This subnet does not serve `/v1/public/*` or a substitute public
leaderboard.

The external publication is the only scoring source. A local operator
adjudication alone does not place a report in that publication; the backend
must publish consistent, justified records. The Python subnet does not export
local reports or adjudications into the external backend automatically.
Leaderboard counts alone are not creditable evidence. Mixed revisions,
truncated report pagination, unavailable adjudication, unpriced valid reports,
any duplicate chain without a non-duplicate root, or disagreement between
status counters, reports and leaderboard fails closed. A leaderboard capped by the backend is
accepted only as an exact ranked prefix; Cortex rebuilds the complete ranking
from the report pages. Transient transport, HTTP, JSON or revision errors receive
at most three read-only attempts, all within the same 30-second snapshot deadline.
Stable adjudication, pricing and backlog gates are not retried. A waiting
adjudication backlog with no published report also fails closed instead of
scoring every miner as `NotAttempted`.

| Adjudication | Algorithm 2 points |
|--------------|--------------------|
| `valid` with severity | 1, regardless of severity |
| `valid` without severity | Invalid publication; scoring fails closed |
| `already_fixed_not_prod` | 0 |
| `invalid_malicious` | 0 |
| `duplicate` | 0; the original valid report is counted once |

Every author with valid evidence participates proportionally. There is no
champion, precision gate, minimum of three reports, severity weighting or
triage-noise gate. Severity (`trivial`, `minor`, `major`, `critical`) remains
required publication evidence and does not affect point value.

For `N` valid reports across the epoch's expected participants, the total Bounty
payout is `0.30 * min(N / 10, 1)` of subnet emission. An author with `n` valid
reports gets `0.30 * n / max(10, N)`. Thus five valid reports distribute 15%;
ten or more distribute 30%. Unused mass burns to UID0, never to Proof or other
authors. Proof retains its separate 70% share. An allocation to UID0 or an
unmapped author burns without increasing another author's allocation.

Counts include cumulative published history at one immutable revision, without
an epoch reset or rolling window. The population is the sealed metagraph's
hotkeys selected by the owner-signed participant policy. Historical authors
outside that population do not enter the total. Duplicate and rejected reports
never add points. Chain weights retain the protocol's independent u16 rounding.

`GET /v1/status` exposes the active `scoring_version`, `points_per_valid_report`,
`full_share_reports`, population and window. Algorithm 2 requires the owner-signed
3000/7000 profile and document version >=2. Until the gateway and validators
complete [activation](../how-to/trust-root.md#activate-proportional-bounty),
legacy 2000/8000 deployments retain algorithm 1 and its champion/precision rules.

If the backend is unreadable, unconfigured or inconsistent, report intake
returns `503` without storing a report. Emission pays nobody from Bounty and
covers the expected participant set with `NoScore(ChallengeInternal)` leaves.
The configured Bounty share then burns to uid 0 through normal sealing. There
is no offline scorer or forced simulation path.

Validators independently verify the [sealed bundle](validators.md); they do
not rerun reports or fetch the Bounty feed. See [troubleshooting](troubleshoot.md)
for refusal and retry guidance.
