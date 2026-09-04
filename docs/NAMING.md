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

| Product | Challenge id (wire) | Crates / service / env prefix |
|---------|--------------------|-------------------------------|
| Relearn | `relearn` | `relearn-*`, `relearn-challenge`, `RELEARN_*` |
| Relearn Image | `relearn-image` | `relearn-t2i-*`, `relearn-t2i-challenge`, `RELEARN_T2I_*` |
| Relearn Agent | `relearn-agent` | `relearn-agent-*`, `relearn-agent-challenge`, `RELEARN_AGENT_*` |
| Bounty | `bounty` | `bounty-*`, `bounty-challenge`, `BOUNTY_*` |
| Proof | `proof` | `proof-*`, `proof-challenge`, `PROOF_*` |

Relearn Image keeps the pre-launch `t2i` spelling in its crates, service, env
prefix, pin filename, and deployed paths. That is not laziness: its
`base-relearn-t2i-*` domain tags are hashed into the committed
`holdout_commitment`, so renaming them would invalidate the pin and every
operator holdout file generated against it. Same reasoning as `BASE_*` above —
rename the product, freeze the identifiers that are measured.

`relearn-mm` is off and has no row in `config/challenges.toml`. Its crates and
its `mm`-profile compose service still exist; nothing routes or signs under
that id.

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

Live miner docs: [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
Short pointer in this repo: [`docs/external-miner/relearn.md`](external-miner/relearn.md).
Design and Prism public repos are historical, not live miner paths.

## Postgres

`design_rating` is a **live table** (see `crates/design-db`,
`crates/design-store-pg`). It is unrelated to the removed unused crate
`crates/design-rating`. Do not drop the table or its accessors as part of
branding work.

## Historical docs

`docs/evidence/` and `docs/spikes/` may still say BASE / BaseIntelligence.
Those paths are non-normative. Do not rewrite them in a cleanup pass
(see [`AGENTS.md`](AGENTS.md)).
