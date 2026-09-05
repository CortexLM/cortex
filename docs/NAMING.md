# Naming: Cortex vs leftover `base` identifiers

**Product name:** Cortex  
**Org / repo:** [`CortexLM/cortex`](https://github.com/CortexLM/cortex)

This repo used to ship as “BASE” / `BaseIntelligence/base`. Human-facing
docs and GitHub metadata now say Cortex. A large set of **wire, deploy, and
crypto identifiers still spell `base` / `BASE_*`**. Changing those forks the
subnet or breaks measured miner CVMs and live droplets. Do not “fix” them
in a drive-by rename.

## What to write in new code and docs

| Use | Spelling |
|-----|----------|
| Product, README, PR titles, comments that mean the project | **Cortex** |
| This GitHub repository | `CortexLM/cortex` |
| Typed config crate keys (canonical) | `BASE_*` |
| Typed config crate aliases (optional) | `CORTEX_*` (same suffix) |
| Cryptographic domain tags, signing context | `base-*-v1` (frozen) |
| Compose project, host paths, GHCR package path | `base` / `/opt/base` / `ghcr.io/baseintelligence/base/*` |

## Challenge ids vs crate names

The same rule applies inside the challenge tree. A challenge id is a **wire**
identifier: it is signed into every leaf, routed at `/challenge/{id}/…`, and
hashed into the trust root. A crate or service name is not.

Live products (trust-root rows, compose services, miner docs):

| Product | Challenge id (wire) | Crates / service / env prefix |
|---------|--------------------|-------------------------------|
| Bounty | `bounty` | `bounty-*`, `bounty-challenge`, `BOUNTY_*` |
| Proof | `proof` | `proof-*`, `proof-challenge`, `PROOF_*` |

`relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and
`prism` are **removed as products**: no challenge bins, no compose
services, no emission. Those historical wire ids still have no trust-root
row, so no leaf may verify under them. Do not reintroduce the product
crates or profiles. Historical miner stubs stay under
[`docs/external-miner/`](external-miner/) so old links do not 404.

Leftover `prism-*` crate names are the **Lium harvest stack** used by Proof
(`harvest-pod`, `prism-lium*`, `prism-store*`, `prism-recipe`, …), not a
live Prism challenge.

## Environment variables

`crates/config` reads **`BASE_*` as the canonical names**. Matching
`CORTEX_*` names are accepted as aliases (`CORTEX_ROLE` ≡ `BASE_ROLE`,
`CORTEX_CONFIG` ≡ `BASE_CONFIG`, …).

- If both are set, **`BASE_*` wins**.
- Deploy compose, droplet env files, and miner CVM `app-compose.json` must
  keep emitting `BASE_*`. Those strings are measured (RTMR3 / pin continuity).
- Other crates and scripts may still read `BASE_*` directly. Do not rename
  those readers unless you also keep the `BASE_*` spelling.

Canonical keys today: `BASE_ROLE`, `BASE_NETUID`, `BASE_CHAIN_ENDPOINT`,
`BASE_CHAIN_ENDPOINTS`, `BASE_GATEWAY_ENDPOINT`, `BASE_DATABASE_URL`,
`BASE_DATABASE_URL_FILE`, `BASE_EPOCH_LENGTH`, `BASE_MIN_SHARE_MASS_BPS`,
`BASE_ROTATION_EPOCHS`, `BASE_MIN_PEER_SAMPLE`, `BASE_MAX_COLLATERAL_AGE_SECS`,
`BASE_DOMAIN`, `BASE_CONFIG`. Many other `BASE_*` knobs exist outside
`crates/config` (gateway owner flag, challenge secrets, deploy paths). Those
are also frozen spellings.

Optional local config file remains `base.toml` (path from `BASE_CONFIG` /
`CORTEX_CONFIG`).

## Cryptographic domain tags

`crates/crypto` binds signatures to static tags. Examples (not exhaustive):

- signing context: `base-sr25519-v1`
- `base-bundle-v1`, `base-rawweight-v1`, `base-dissent-v1`, `base-root-v1`
- `base-attest-v1`, `base-trustroot-v1`
- design: `base-design-round-id-v1`, `base-design-submission-v1`, `base-design-pair-id-v1`

Renaming a tag makes old signatures unverifiable. Leave them.

## Deployed filesystem and image paths

Leave these as `base` even in new docs that mention them:

| Kind | Example |
|------|---------|
| Droplet checkout | `/opt/base` |
| Miner CVM secret dir | `/run/base/` |
| Validator LKG default | `/var/lib/base/last-sealed.bundle` |
| Age identity | `/etc/base/age-identity.txt` |
| Compose project | `COMPOSE_PROJECT_NAME=base` |
| Sandbox name prefix | `base-design-` (pinned in `DESIGN_CHALLENGE.md`) |
| GHCR packages | `ghcr.io/baseintelligence/base/<suffix>` |
| systemd units | `base-real-seal.timer`, `base-burn-seal.timer` |
| Host nicknames | `base-staging`, `base-prod`, … |

GHCR still publishes under the historical `baseintelligence/base` package
path. Image **contents** are this Cortex tree; the registry path is a pin,
not the product name.

## On-chain and public hostnames

- Live gateway hostname `chain.joinbase.ai` is operator DNS, not a GitHub
  brand string. Do not retarget it from a docs-only PR.
- Subnet netuid and on-chain names stay whatever the chain already uses.

## Public miner repos (other GitHub repositories)

Live miner docs live in this repo:

- Bounty: [`docs/external-miner/bounty.md`](external-miner/bounty.md)
- Proof: [`docs/external-miner/proof.md`](external-miner/proof.md)

Off/archived pointers (`relearn.md`, `relearn-image.md`, `relearn-agent.md`,
`relearn-mm.md`) stay so historical links do not 404. They are not live
products. Frozen specs (`docs/DESIGN_CHALLENGE.md`, `docs/PRISM.md`) stay
archived; do not send miners there as live work.

## Postgres

Applied SQL migrations are append-only. Tables created for retired
challenges (for example `design_rating` in
`crates/db/migrations/0006_design_challenge.sql`) stay in Postgres. Do not
drop them as part of product cleanup or branding work. Accessors in
deleted design crates are gone; do not reintroduce those crates to “use”
the table.

## Historical docs

`docs/evidence/` and `docs/spikes/` may still say BASE / BaseIntelligence.
Those paths are non-normative. Do not rewrite them in a cleanup pass
(see [`AGENTS.md`](AGENTS.md)).
