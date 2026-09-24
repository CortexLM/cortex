<!-- protocol_version: 1 -->

# Bounty miner guide

Bounty rewards reproducible Cortex product and backend bug reports tied to a
Bittensor hotkey. Its code lives in [CortexLM/bounty](https://github.com/CortexLM/bounty).
It is a challenge container that the master runs, updates automatically and
serves through the gateway under `/challenge/bounty/`. The full miner guide,
API reference and scoring rules are in that repository:

- [Miner guide](https://github.com/CortexLM/bounty/blob/main/docs/miner.md)
- [API reference](https://github.com/CortexLM/bounty/blob/main/docs/api.md)
- [Scoring](https://github.com/CortexLM/bounty/blob/main/docs/scoring.md)

This CLI still signs and sends the two miner requests:

```bash
uv run cortex miner --gateway "$GATEWAY" --wallet-name research --wallet-hotkey miner \
  bounty-pair --account-id "$CORTEX_ACCOUNT_ID" --accept-terms --session-file ./bounty-session
uv run cortex miner --gateway "$GATEWAY" --wallet-name research --wallet-hotkey miner \
  bounty-report --session-file ./bounty-session --title "..." \
  --body-file report.md --repro-file reproduction.md
```

`bounty-pair` posts to `/challenge/bounty/v1/pair` with `terms_accepted: true`.
It signs `cortex-bounty-v1|{account_id}|{nonce}|{exp}` with the hotkey's
Substrate signing context, not the Cortex one. `bounty-report` posts to
`/challenge/bounty/v1/reports`. Only a report that CortexLM/backend publishes as
`valid` earns weight: one point per valid report. Bounty pays
`share * min(N / 10, 1)`, split in proportion to each author's count, and the
rest burns to UID0. The master turns the container's weights into signed leaves
under the [challenge contract](../CHALLENGES.md). Validators never contact the
container.
