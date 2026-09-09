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

- `.github/workflows/ci.yml` — auto-deploy staging after its `ci` job succeeds on `main`; `deploy-staging.yml` is the manual lane
- `.github/workflows/deploy-prod.yml` — on push of `v*.*.*` tags from `main` (and manual dispatch with SHA)

**Prod release flow (tag-based):**
1. CI passes on `main` for commit X.
2. `images.yml` builds/pushes GHCR digests for X, promotes pin services into `deploy/pins/staging.json`, commits digests + pins to `main`.
3. Staging droplets may still deploy with `--build-from source` (iteration); the pin ladder is what authorizes prod.
4. Operator cuts `git tag vX.Y.Z` on commit X and pushes the tag.
5. `deploy-prod.yml` preflight: CI green for X; `origin/main` staging pins `commit_sha == X`.
6. Fail-closed Postgres backup (SSH dump on prod master → DO Spaces), then `promote.sh --env prod --confirm-prod` per service.
7. Both prod hosts: `remote-deploy.sh --build-from registry` (pull GHCR `@sha256`, retag to Compose tags, `up --no-build`).
8. Smoke `/healthz`. `environment: production` (enable required reviewers in GitHub UI).

**`ctx` CLI release (miner installer):** `.github/workflows/release-ctx.yml`
builds `bins/ctx` for linux / darwin (amd64 + arm64) and windows and attaches
`ctx-*.tar.gz` / `.zip` plus `SHA256SUMS.txt` to the GitHub Release.
`scripts/install-ctx.sh` (the `curl … | sh` in the miner docs) installs from
`releases/latest` — or `CTX_VERSION=vX.Y.Z` — and refuses a release without
those files, so a release published without this workflow installs nothing.
The **release tag is a publishing label**; the **build ref is where the
sources come from**:

| Trigger | Attaches to | Builds from |
|---------|-------------|-------------|
| push tag `v*.*.*` | that tag | the tagged commit |
| `workflow_dispatch` | `tag` (must already exist) | `build_ref` (default `main` = tip at run time) |

```bash
# 1. Cut the label on tip (the tag push runs release-ctx from the tagged commit).
gh release create vX.Y.Z --target main --title "ctx CLI vX.Y.Z" --notes "…"
# 2. (Re)build ctx from the tip of main and attach it to that existing tag —
#    also the fix for a release that has no ctx assets.
gh workflow run release-ctx.yml -f tag=vX.Y.Z
gh workflow run release-ctx.yml -f tag=vX.Y.Z -f build_ref=<sha-on-main>   # pin a build
# 3. Confirm the six assets, then install the way a miner does.
gh run watch && gh release view vX.Y.Z --json assets -q '.assets[].name'
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh
```

A dispatch never checks the tag out: an old tag predates `bins/ctx` and used
to fail with `package ID specification ctx did not match any packages`. The
`resolve` job refuses a build ref that is not a commit on `main` (miners
install what this publishes; an unmerged branch or a fork is never released)
or has no `bins/ctx`, refuses a `tag` that does not exist (a typo cannot
mint a release), builds all five archives cold — no cargo cache — from one
resolved commit, and appends that commit to the release notes. Runs for
the same tag queue (`concurrency`) instead of racing on the upload. Any
`v*.*.*` tag push also triggers `deploy-prod.yml`, whose preflight fails
closed unless CI is green for that SHA and `deploy/pins/staging.json` carries
it — a ctx-only label therefore shows a failed `deploy-prod` preflight and
deploys nothing. `releases/latest` is GitHub's newest non-draft,
non-prerelease release, ordered by the tagged commit's date rather than the
publish date — cut labels on tip, and mark experimental labels pre-release to
keep them off the default install path.

Required GitHub secrets:

| Secret | Purpose |
|--------|---------|
| `STAGING_SSH_KEY` | private key for droplet SSH |
| `STAGING_MASTER_HOST` | public IPv4 of `base-staging` |
| `STAGING_VALIDATOR_HOST` | public IPv4 of `base-staging-validator` |
| `STAGING_MASTER_GATEWAY_URL` | optional, default `http://10.116.0.2:8080` |
| `PROD_HOST` | public IPv4 of `base-prod` |
| `PROD_SSH_KEY` | optional override of staging key |
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

# 3) After staging is healthy, promote same digest to prod
./deploy/scripts/promote.sh \
  --env prod --service validator --confirm-prod \
  --image ghcr.io/org/validator@sha256:<64-hex>

# 4) Rollback = re-promote previous snapshot
./deploy/scripts/promote.sh --env staging --service validator --rollback

# 5) Restore drill (scratch DB row-count match)
./deploy/scripts/pg-restore-drill.sh --s3-uri s3://base-backups/pg/staging/<stamp>.sql.gz
```

Pin files: `deploy/pins/staging.json`, `deploy/pins/prod.json`.  
Staging promote **never** writes the prod pin. Prod promote requires staging ladder + `--confirm-prod`.  
Updater consumes `BASE_UPDATER_DESIRED_IMAGE` (also written to `deploy/pins/<env>.desired.env`).

Verify locally: `./deploy/scripts/verify-task-43.sh`
