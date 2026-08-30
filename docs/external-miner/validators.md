<!-- protocol_version: 1 -->

# Validators

Relearn scoring is centralized (master + Lium). Bounty adjudication is
operator-side on master. You still run a validator.

## Job

1. Pull `GET /v1/weights/latest` from the master gateway.
   - Prod: `https://chain.joinbase.ai/v1/weights/latest`
   - Staging / VPC: `$BASE_GATEWAY_ENDPOINT/v1/weights/latest` (compose default `http://10.116.0.2:8080`)
2. Verify the sealed bundle: signatures, D24 completeness (every declared participant has a leaf), owner trust root on **local disk** (`config/challenges.toml`, `config/measurements.toml`), no forged leaves.
3. `set_weights` on-chain.

If you skip verification, a bad gateway can publish fake weights. That is the job: consensus check on a sealed result, not a second eval farm.

Unsealed or decode-error latest is a **burn vector** (`sealed: false`, uid 0 = 100%). Do not treat that as a real seal.

## Not your job

- Run evals
- Rent Lium
- Promote Relearn champions (`POST /v1/admin/promote` is master / operator)
- Adjudicate Bounty reports (`POST /challenge/bounty/v1/admin/adjudicate`)

## Run

Binary: [`bins/validator`](../../bins/validator) (`validator-bin`, bin name `validator`).
Compose role: [`deploy/compose/role-validator.yml`](../../deploy/compose/role-validator.yml)
(no gateway, no `relearn-challenge` / `bounty-challenge`).
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
