# Site API (`GET /v1/site/*`)

Marketing aggregator on the **master gateway**. CamelCase JSON matching the
frontend `BaseApi` contract (`types.ts` / `contract.ts`).

## Sources

| Site path | Upstream |
|-----------|----------|
| `/v1/site/arenas`, `/arenas/bounty`, `/arenas/proof` | Static frames + registry pick `challenge_id=bounty\|proof` → `/v1/status` |
| `/v1/site/network`, `/validators` | Chain tip / metagraph when available; numeric unknowns are `0` or omitted — never invented TAO price/emission |
| `/v1/site/activity` | Empty until bounty/proof publish an ops event stream (never invent copy) |
| Coding arena | `status: "paused"`, empty submissions / matrix / leaderboard |

Live arenas listed by `/v1/site/arenas` are **`coding` (paused)**, **`bounty`**,
and **`proof`**. Retired slugs (`design`, `prism`, `relearn*`) answer **404**.

Bounty and Proof leaderboards / submissions stay empty pages until those
challenges publish a marketing list. Missing backends leave the static live
frame in place so the emission column still accounts for every paid challenge.

`GET /v1/site/arenas/{slug}/submissions` and `/leaderboard` accept optional
`?q=` — ignored on the empty boards.

Backends must be registered (same as challenge proxy), e.g.
`deploy/scripts/register-challenge-backends.sh`.
