<!-- protocol_version: 1 -->

# Bounty miner guide

Bounty (`bounty`, 2000 bps) accepts reproducible Cortex product and backend bug
reports associated with your Bittensor hotkey. Install the [Python CLI](README.md)
and obtain the gateway URL from the subnet operator.

## Check scoring availability

```bash
curl --fail-with-body "$GATEWAY/challenge/bounty/v1/status"
```

The response publishes research terms, quotas and `scoring_backend`.
`can_score` reflects a live read and validation of the external backend feed;
on failure it is `false` and `reason` explains the refusal. Every report POST
checks the feed again, so an outage or inconsistent publication after a
successful status check can still return `503`.

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
`401 invalid_session`.
Sessions do not currently expire automatically by age.

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
| Minimum body length | 80 characters |
| Minimum reproduction length | 20 characters |

An empty or repeated title/body and low-substance repeated-token content
return `400`. A quota violation returns `429`. Framework schema failures may
return `422`. Successful intake returns `201` with `id`, `miner_hotkey`,
`state` and `fingerprint`; acceptance into triage is not a reward.

Report reads (`GET /v1/reports` and `/v1/reports/{id}`) and
`POST /v1/admin/adjudicate` require operator bearer authentication. The public
gateway denies report reads. There is no public `bounty show` command.

## Scoring and adjudication

Cortex **reads** the CortexLM/backend public API configured by
`BOUNTY_BACKEND_PUBLIC_URL`. The required backend routes are
`/v1/bounty/public/leaderboard` and `/v1/bounty/public/reports`. This subnet
does not serve `/v1/public/*` or a substitute public leaderboard.

The external publication is the only scoring source. A local operator
adjudication alone does not place a report in that publication; the backend
must publish consistent, justified records. The Python subnet does not export
local reports or adjudications into the external backend automatically.
Leaderboard counts alone are not creditable evidence.

| Adjudication | Effect |
|--------------|--------|
| `valid` with severity | Eligible evidence, subject to the scoring gates |
| `valid` without severity | Not creditable; missing severity prevents eligibility |
| `already_fixed_not_prod` | No reward or direct penalty; counts as triage noise |
| `invalid_malicious` | Negative credit; may lead to a burn outcome |
| `duplicate` | No extra reward or direct penalty; counts as triage noise |

Paid score is precision times mean severity impact, subject to champion
displacement and eligibility gates. Precision is priced valid reports divided
by priced valid plus malicious reports. The minimum precision is 6000 bps,
and at least three decided reports are required. Severity levels are
`trivial`, `minor`, `major` and `critical`. Unpriced valid rows cannot be used
to manufacture credit.

The duplicate/already-fixed triage-noise ratio is an **off-score gate**. It is
not multiplied into the visible precision-times-severity score; exceeding
5000 bps rejects eligibility. A high report count is not a substitute for
precision and severity.

If the backend is unreadable, unconfigured or inconsistent, report intake
returns `503` without storing a report. Emission pays nobody from Bounty and
covers the expected participant set with `NoScore(ChallengeInternal)` leaves.
The 2000 bps share then burns to uid 0 through normal sealing. There is no
offline scorer or forced simulation path.

Validators independently verify the [sealed bundle](validators.md); they do
not rerun reports or fetch the Bounty feed. See [troubleshooting](troubleshoot.md)
for refusal and retry guidance.
