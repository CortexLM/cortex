<!-- protocol_version: 1 -->

# Validators

Bounty scoring reads CortexLM/backend on master. Proof scoring is centralized
(master + digest-pinned RLM judge). You still run a validator.

Live ids and emission: `bounty` 7000 bps, `proof` 3000 bps (sum 10000).
`relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and
`prism` have no row and earn 0 — a leaf claiming those ids fails the
trust-root check, which is the point.

## Job

1. Pull `GET /v1/weights/latest` from the master gateway.
   - Prod: `https://chain.joinbase.ai/v1/weights/latest`
   - Staging / VPC: `$BASE_GATEWAY_ENDPOINT/v1/weights/latest` (compose default `http://10.116.0.2:8080`)
2. Verify the sealed bundle: signatures, D24 completeness (every declared participant has a leaf), owner trust root on **local disk** (`config/challenges.toml`, `config/measurements.toml`), no forged leaves.
3. `set_weights` on-chain.

If you skip verification, a bad gateway can publish fake weights. That is the job: consensus check on a sealed result, not a second eval farm.

Unsealed or decode-error latest is a **burn vector** (`sealed: false`, uid 0 = 100%). Do not treat that as a real seal. Do not submit it, and do not submit a previously verified seal (LKG) while latest is unsealed. A sealed vector that is a pure burn to the registered owner or a validator-permit UID must not be submitted either — Yuma would pay that hotkey, which is not a burn.

## Not your job

- Run evals
- Rent Lium
- Promote champions (`POST /v1/admin/promote` is master / operator; Relearn is off)
- Adjudicate Bounty reports (`POST /challenge/bounty/v1/admin/adjudicate`)
- Read the Bounty public feed. Bounty adjudications live in CortexLM/backend;
  the challenge service on master fetches them and signs leaves, and you verify
  the sealed bundle like any other challenge. An epoch where that host could
  not read the feed still carries a full bounty leaf set — every leaf a
  `NoScore`, reason `ChallengeInternal` (6) — so D24 holds and the 7000 bps
  burns to uid 0. That is fail-closed working, not a bundle to dissent on.

## Run

Binary: [`bins/validator`](../../bins/validator) (`validator-bin`, bin name `validator`).
Compose role: [`deploy/compose/role-validator.yml`](../../deploy/compose/role-validator.yml)
(no gateway, no challenge services at all).
Env: [`deploy/env/validator.env.example`](../../deploy/env/validator.env.example).

```bash
# required: BASE_ROLE=validator, BASE_NETUID, database URL, BASE_CHAIN_ENDPOINT
# BASE_GATEWAY_ENDPOINT = master gateway (prod pin: https://chain.joinbase.ai)
# wallet: BASE_VALIDATOR_WALLET or BASE_VALIDATOR_MNEMONIC_FILE (for set_weights)

./deploy/scripts/materialize-env.sh
docker compose -f docker-compose.yml \
  -f deploy/compose/role-validator.yml up -d
```

The process probes `/v1/weights/latest`, runs `compare_bundle` against the local trust root, and submits on Match. No signing key → verify still runs, `set_weights` does not.
