# Staging testnet E2E runbook

End-to-end testnet 541 procedure on the 2-host staging pair.

## Prerequisites

- Staging master (`base-staging`, 68.183.23.51 / 10.116.0.2) running master role
- Staging validator (`base-staging-validator`, 142.93.197.253 / 10.116.0.4) running validator role
- Both deployed from the same `main` commit via `deploy-staging.yml` or manual `remote-deploy.sh`
- `deploy/secrets/bounty_sk`, `deploy/secrets/proof_sk`, and `deploy/secrets/gateway_sk` present on master (mode 0400, uid 65532)
- `deploy/env/*.env` materialized on both hosts (mode 0600)

## Verify staging master

```bash
ssh root@68.183.23.51
cd /opt/base
docker compose -f docker-compose.yml -f deploy/compose/role-master.yml -f deploy/compose/env-staging.yml --profile master ps
# All services Up, postgres healthy; gateway + bounty-challenge + proof-challenge on master
curl -fsS http://127.0.0.1:18080/healthz   # validator tunnel (if co-located; droplet master has no validator)
curl -fsS http://127.0.0.1:8080/healthz    # gateway
```

## Verify staging validator

```bash
ssh root@142.93.197.253
cd /opt/base
docker compose -f docker-compose.yml -f deploy/compose/role-validator.yml -f deploy/compose/env-staging.yml ps
# validator + postgres + socket-proxy Up (no gateway, no challenge exec)
docker logs $(docker ps -q --filter name=validator) 2>&1 | tail -20
# Look for: "Match epoch=" lines (bundle signature valid → coordination loop healthy)
```

If validator logs show `bundle gateway signature invalid`:
1. Confirm both hosts run the same commit: `git -C /opt/base rev-parse HEAD`
2. Confirm gateway_sk on master matches the key used to sign the bundle
3. Redeploy master: `remote-deploy.sh --host root@68.183.23.51 --role master --env staging`

## Verify bundle seal (master)

```bash
ssh root@68.183.23.51
curl -fsS http://127.0.0.1:8080/v1/weights/latest
# JSON: sealed true after a real seal; burn fallback is sealed:false (uid0=100%) — do not submit that
```

## Register challenge backends (required after gateway restart)

The gateway registry is **in-memory**. After every redeploy/restart, challenge
proxy routes return `503 no healthy backends for challenge_id=…` until backends
are registered. `remote-deploy.sh` (master) re-seeds automatically; to do it by
hand:

```bash
# From this repo (against a reachable gateway; reads deploy/secrets/gateway_admin_token):
GATEWAY_URL=http://staging.api.joinbase.ai ./deploy/scripts/register-challenge-backends.sh

# Or on the droplet (admin bearer required after #100):
TOKEN=$(tr -d '[:space:]' </opt/base/deploy/secrets/gateway_admin_token)
curl -fsS -X POST http://127.0.0.1:8080/v1/admin/backends \
  -H 'content-type: application/json' \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"challenge_id":"bounty","base_url":"http://bounty-challenge:8096","weight":1}'
curl -fsS -X POST http://127.0.0.1:8080/v1/admin/backends \
  -H 'content-type: application/json' \
  -H "Authorization: Bearer ${TOKEN}" \
  -d '{"challenge_id":"proof","base_url":"http://proof-challenge:8100","weight":1}'
unset TOKEN
curl -fsS http://staging.api.joinbase.ai/challenge/bounty/health
curl -fsS http://staging.api.joinbase.ai/challenge/proof/health
```

Public staging API: cleartext **`http://staging.api.joinbase.ai`** (`/healthz`,
`/challenge/{bounty|proof}/v1/...`). Challenge liveness is `/health` (not `/healthz`).

## Verify challenge identities

```bash
ssh root@68.183.23.51
curl -fsS http://127.0.0.1:8080/challenge/bounty/v1/status
# JSON includes challenge_id=bounty, can_score, scoring_backend
curl -fsS http://127.0.0.1:8080/challenge/proof/v1/status
# JSON includes challenge_id=proof, can_score, proxy_model (RLM judge id)
```

## Testnet chain (read-only smoke)

The validator uses `FakeChain` by default. To verify live testnet connectivity:

```bash
# On operator machine (not on droplet — requires cargo)
cargo run -p xtask -- metadata-snapshot --check
# Verifies metadata/testnet.lock matches live Finney testnet
```

## Deploying a new commit to staging

1. Push to `main` — `ci.yml` runs, then `deploy-staging.yml` auto-deploys both hosts.
2. Or manual:
   ```bash
   ./deploy/scripts/remote-deploy.sh --host root@68.183.23.51 --role master --env staging --build-from source
   ./deploy/scripts/remote-deploy.sh --host root@142.93.197.253 --role validator --env staging --build-from source
   ```
3. Post-deploy: CI checks validator `/healthz` (fail-closed) and greps for `Match epoch=` within 180s.

## Rollback

```bash
# Redeploy previous known-good commit
git checkout <good-sha>
./deploy/scripts/remote-deploy.sh --host root@68.183.23.51 --role master --env staging --build-from source
./deploy/scripts/remote-deploy.sh --host root@142.93.197.253 --role validator --env staging --build-from source
```

## Known limitations (see docs/COMPLETENESS.md)

- `FakeChain` is the default backend; `BASE_CHAIN_BACKEND=live` switches to `chain-live`.
- CRV4 tlock encryption is implemented (`tle` / Drand Quicknet); when commit-reveal is off, `set_weights` is used instead.
- Proof live submits stay **503** until harvest is wired, a baseline is sealed, and ≥1 topic is open.
