# Cortex deploy (compose)

Operator reference for Cortex's autonomous research network. For the product
purpose and current research limits, start with the
[overview](../docs/OVERVIEW.md). This guide describes deployment, not evidence
that the complete Proof research loop is ready.

## Services

| Service | Profile | Image |
|---------|---------|--------|
| `postgres` | default | `postgres@sha256:33f9…` (16) |
| `validator` | default | build `deploy/Dockerfile` target `validator` |
| `updater` | optional **`auto-update`** | build target `updater` |
| `socket-proxy` | default | `tecnativa/docker-socket-proxy@sha256:9e4b…` |
| `gateway` | **`master`** | build target `gateway` |
| `bounty-challenge` | default; disabled on validator hosts | build target `bounty-challenge` |
| `proof-challenge` | default; disabled on validator hosts | build target `proof-challenge` |

The base Compose file has **5 default services**, without gateway or updater.
Adding `--profile master` adds the gateway. Operator hosts must also use the role
overlays: `role-master.yml` disables the validator, and `role-validator.yml`
keeps only Postgres and the validator. A co-located validator is for local E2E,
not a second production weight submitter.

## Hard rules

- No floating image tags (digest pins only).
- `/var/run/docker.sock` only on `socket-proxy` (read-only).
- socket-proxy allowlist: `CONTAINERS=1 IMAGES=1 POST=1` (matches `updater`).
- Secrets via age-decrypted env files mode **0600** under `deploy/env/*.env` — never in images or cloud-init.

## Quick start (local)

Master + gateway + validator + challenges on **testnet 541**, with an ephemeral
cloudflared public URL for the gateway. Procedure:
[`docs/runbooks/local-testnet-e2e.md`](../docs/runbooks/local-testnet-e2e.md).

```bash
./deploy/scripts/local-e2e.sh --help
./deploy/scripts/materialize-env.sh
./deploy/scripts/local-e2e.sh --dry-run
./deploy/scripts/local-e2e.sh --smoke    # or --live when wallets are present
```

Follow the runbook's build and key prerequisites before `--smoke`. Health checks
alone do not validate scoring; simulate submissions and verify a real sealed
bundle. See [the challenge verification contract](../AGENTS.md#challenge-verification-mandatory-path-coverage).

## Age secrets (production)

```bash
# On operator machine
age -r "$RECIPIENT" -o deploy/env/postgres.env.age deploy/env/postgres.env
# On droplet (identity delivered out of band)
export AGE_IDENTITY=/etc/base/age-identity.txt
./deploy/scripts/materialize-env.sh
```


## Host topology (4 hosts: 2 staging + 2 prod)

| Host | Droplet | VPC IP | Role | Hotkey | Gateway |
|------|---------|--------|------|--------|---------|
| staging master | `base-staging` (`68.183.23.51`) | 10.116.0.2 | network master | **yes** (`BASE_GATEWAY_HOTKEY`) | **yes** (`--profile master`) — public API **`staging.api.joinbase.ai`** (`BASE_DOMAIN`, cleartext `:80`/`:8080`) |
| staging validator | `base-staging-validator` | 10.116.0.4 | normal validator | **no** | **no** — uses master gateway over VPC `:8080` |
| prod master | `base-prod` | 10.116.0.3 | network master | yes | yes |
| prod validator | `base-prod-validator` | 10.116.0.5 (assigned) | normal validator | **no** | **no** — uses prod master gateway over VPC `:8080` |

DNS (operator): `staging.api.joinbase.ai` **A** → staging master public IPv4 (`STAGING_MASTER_HOST` / `68.183.23.51`).
The hotkey column refers to gateway owner identity. Validators need their own
wallet to submit weights; they do not need the owner's key.

Deploy (manual or via CI):

```bash
export BASE_SSH_IDENTITY=~/.ssh/id_ed25519

# Staging master (testnet 541)
./deploy/scripts/remote-deploy.sh \
  --host root@68.183.23.51 --role master --env staging \
  --bootstrap-secrets-from root@68.183.23.51

# Staging validator (points at master VPC gateway)
./deploy/scripts/remote-deploy.sh \
  --host root@142.93.197.253 --role validator --env staging \
  --gateway-endpoint http://10.116.0.2:8080 \
  --bootstrap-secrets-from root@68.183.23.51

# Prod master
./deploy/scripts/remote-deploy.sh \
  --host root@206.189.224.155 --role master --env prod \
  --bootstrap-secrets-from root@206.189.224.155

# Prod validator (points at prod master VPC gateway)
./deploy/scripts/remote-deploy.sh \
  --host root@<prod-validator-ip> --role validator --env prod \
  --gateway-endpoint http://10.116.0.3:8080 \
  --bootstrap-secrets-from root@206.189.224.155
```

### Compose matrix (role × env)

| File | Purpose |
|------|---------|
| `deploy/compose/role-master.yml` | gateway profile, VPC `:8080` publish, loopback tunnels |
| `deploy/compose/role-validator.yml` | gateway disabled, VPC gateway endpoint |
| `deploy/compose/env-staging.yml` | testnet 541, `wss://test.finney.opentensor.ai:443`, 3s coordination |
| `deploy/compose/env-prod.yml` | mainnet, conservative intervals |
| `deploy/compose/env-local.yml` | local-only overlay (on top of staging); used by `local-e2e.sh` |
`remote-deploy.sh --env staging|prod --role master|validator` selects the correct
combination. Verify locally: `./deploy/scripts/assert-compose-matrix.sh`.

### Auto CI deploy

**The DigitalOcean staging soak is retired** (owner decision, 2026-09-10). CI no
longer deploys staging droplets: `ci.yml` is fmt/clippy/test/deny/xtask only and
`deploy-staging.yml` is deleted. `deploy/compose/env-staging.yml` stays — it is
the testnet overlay `local-e2e.sh` builds on, not a CI deploy lane.

- `.github/workflows/images.yml` — on push to `main`: build/push GHCR digests, then record prod pins as the `prod-pins-<sha>` artifact
- `.github/workflows/deploy-prod.yml` — on a successful `images` run on `main`, on `v*.*.*` tags, or manual dispatch with a SHA

**Prod deploy flow (every main update):**
1. CI passes on `main` for commit X; `images.yml` builds/pushes GHCR digests for X.
2. `images.yml` job `prod-pins` runs `promote.sh --env prod` over those digests and uploads `deploy/pins/prod.json` + `deploy/digests/X.json` as artifact `prod-pins-X`. **Nothing is pushed to `main`** — branch protection (PR + Greptile review) rejects a CI pin commit with GH013.
3. `deploy-prod.yml` preflight: X is an ancestor of `origin/main`, CI is green for X (it polls, since `ci` and `images` run in parallel), and the `images` run for X has a live `prod-pins-X` artifact.
4. Fail-closed Postgres backup (SSH dump on prod master → DO Spaces).
5. Both prod hosts: `remote-deploy.sh --build-from registry` (pull GHCR `@sha256`, retag to Compose tags, `up --no-build`).
6. Smoke `/healthz` (fail-closed).

`deploy/pins/prod.json` in git is a **template**, not the deployed state: CI
derives the deployed pins per commit from the GHCR digests and keeps them in the
run artifact. **Rollback = dispatch `deploy-prod` with the previous good commit
SHA** (its `images` run artifact is still the pin set for that commit); the
in-tree `promote.sh --rollback` path stays for local/manual pin work.

Artifacts expire. If the pin artifact for the commit you want is gone,
`deploy-prod` preflight refuses rather than deploying something unpinned — re-run
that commit's `images` run, or dispatch `images.yml` on a ref pointing at it
(`prod-pins` runs on manual dispatch too, exactly for this recovery).

Prod hosts pull GHCR anonymously (`remote-deploy.sh` never logs in), so the
`ghcr-public` job must keep the packages public.

Required GitHub secrets:

| Secret | Purpose |
|--------|---------|
| `PROD_HOST` | public IPv4 of `base-prod` |
| `PROD_SSH_KEY` | private key for prod droplet SSH (falls back to `STAGING_SSH_KEY`, which is the same operator key) |
| `PROD_VALIDATOR_HOST` | public IPv4 of `base-prod-validator` |
| `PROD_MASTER_GATEWAY_URL` | optional, default `http://10.116.0.3:8080` |
| `BASE_BACKUP_ENDPOINT` | DO Spaces endpoint (e.g. `https://nyc3.digitaloceanspaces.com`) — **required for prod promote (fail-closed)** |
| `SPACES_ACCESS_KEY_ID` | Spaces access key (fallback: `AWS_ACCESS_KEY_ID`) |
| `SPACES_SECRET_ACCESS_KEY` | Spaces secret (fallback: `AWS_SECRET_ACCESS_KEY`) |
| `BASE_BACKUP_BUCKET` | optional, default `base-backups` |

> **Not AWS EKS.** Network services stay on Docker Compose on DigitalOcean droplets. A separate DOKS cluster on this account (`basecrawl-prod-nyc3`) is unrelated and must not host Cortex.


## Infrastructure (DigitalOcean)

Terraform lives in [`terraform/`](./terraform/): staging and production master
and validator droplets, plus the firewall. See the four-host topology above and
the Terraform inputs for sizes. Cloud-init installs Docker + Compose only.

Age delivery helpers:

```bash
# Encrypt (operator machine; recipient = age public key)
./deploy/scripts/age-encrypt-env.sh \
  --recipient "$(age-keygen -y /path/to/age-identity.txt)" \
  --src-dir deploy/env \
  --out-dir /tmp/base-env-age

# After OOB identity install on the droplet:
./deploy/scripts/age-push-env.sh --host root@DROPLET_IP --age-dir /tmp/base-env-age --materialize
```

See [`terraform/README.md`](./terraform/README.md) for apply steps and R11 notes.

### SSH access from CI

The `base-hosts` firewall allows port 22 from the operator IP only. GitHub
runners get ephemeral Azure addresses that cannot be allowlisted ahead of time,
so the deploy jobs use [`.github/actions/do-firewall`](../.github/actions/do-firewall):
it adds an inbound rule for the runner's own `/32`, and an `if: always()` step
removes exactly that rule afterwards. Port 22 is closed to the world at rest.

This needs a `DIGITALOCEAN_TOKEN` repository secret. Two caveats:

- A runner killed between the two steps leaves its `/32` behind. Audit with
  `doctl compute firewall get base-hosts` and delete anything that is not the
  operator IP.
- `terraform apply` rewrites the firewall's whole rule set, so it will drop a
  live ephemeral rule. Do not apply Terraform while a deploy is running.

## Test-only: evil-gateway profile (task 48)

**Never enable in production.** Adversarial staging harness:

```bash
docker compose --profile evil-gateway config --services   # must list evil-gateway
docker compose --profile master config --services         # must NOT list evil-gateway
./deploy/scripts/assert-evil-gateway-not-default.sh
```

Offline proofs (no live TAO): `cargo test -p validator a48_`


## Promotion pipeline (task 43)

Digest-only rollout with backup-before-pin and fail-closed prod.

```bash
# 1) CI (or local) records digests after build
./deploy/scripts/record-image-digests.sh

# 2) Promote known-good digest to staging (backs up Postgres first)
export PGHOST=... PGUSER=... PGPASSWORD=... PGDATABASE=base
export BASE_BACKUP_ENDPOINT=https://nyc3.digitaloceanspaces.com   # or local MinIO
export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
export BASE_BACKUP_BUCKET=base-backups
./deploy/scripts/promote.sh \
  --env staging --service validator \
  --image ghcr.io/org/validator@sha256:<64-hex>

# 3) Promote a digest to prod
#    --force-prod skips the staging-digest ladder, which no longer exists:
#    nothing writes deploy/pins/staging.json since the staging soak was retired.
./deploy/scripts/promote.sh \
  --env prod --service validator --confirm-prod --force-prod \
  --image ghcr.io/org/validator@sha256:<64-hex>

# 4) Rollback = re-promote previous snapshot
./deploy/scripts/promote.sh --env staging --service validator --rollback

# 5) Restore drill (scratch DB row-count match)
./deploy/scripts/pg-restore-drill.sh --s3-uri s3://base-backups/pg/staging/<stamp>.sql.gz
```

Pin files: `deploy/pins/staging.json`, `deploy/pins/prod.json`.  
Staging promote **never** writes the prod pin; the staging pin file is now only a
local/manual scratch env (`verify-task-43.sh` exercises it) and no workflow
writes it. Prod promote still requires `--confirm-prod`.  
Updater consumes `BASE_UPDATER_DESIRED_IMAGE` (also written to `deploy/pins/<env>.desired.env`).  
In CI the prod rollback is a `deploy-prod` dispatch on the previous good commit
SHA, not a pin-file edit — pins are rebuilt from that commit's GHCR digests.

Verify locally: `./deploy/scripts/verify-task-43.sh`
