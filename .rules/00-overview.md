# 00 — You must read `.rules/` before you open a PR

**This directory is the contract for every coding agent and every human who
touches `CortexLM/cortex`.** Read all of it before you open a pull request, and
again before you mark one ready for review. A PR body that does not attest to
this is not reviewable and CI will fail it.

There is no `docs/` tree. The three doc surfaces are:

| Surface | Audience | Rule |
|---------|----------|------|
| [`README.md`](../README.md) | humans arriving at the repo | the only human-facing document; keep it true |
| [`AGENTS.md`](../AGENTS.md) | agents / operators | repo map, non-negotiables, verification duties |
| `.rules/` (this directory) | agents / reviewers | how to work, what must pass, how to version |

## Read order

| File | What it binds you to |
|------|----------------------|
| `00-overview.md` | this page: the reading duty itself |
| [`10-maintenance.md`](10-maintenance.md) | code and repo hygiene; keeping the repo true A→Z |
| [`20-pre-prod-local.md`](20-pre-prod-local.md) | the exact local gates that must pass before "ready" |
| [`30-pr.md`](30-pr.md) | PR shape, attestation, commit subjects |
| [`40-agents.md`](40-agents.md) | keeping `AGENTS.md` accurate in the same PR |
| [`50-versioning.md`](50-versioning.md) | automatic versioning (command + CI gate) |
| [`60-naming.md`](60-naming.md) | frozen `base` / `BASE_*` / domain-tag spellings |
| [`70-secrets-mnemonics.md`](70-secrets-mnemonics.md) | BIP39 / `secretPhrase` never leave a 0600 file |

[`contracts/`](contracts/README.md) holds the **frozen normative specs** —
bundle bytes, the design challenge freeze, prism, the threat model, and the
miner-facing docs. They are pinned by `xtask` gates. You do not need to read
all of them for every PR; you **must** read the ones your change touches, and
you must never weaken or rewrite their incentive, scoring, or consensus
semantics.

## The short version

1. Read this directory. Read the contracts you touch.
2. Make the change. Delete what you replaced; leave no dead code.
3. Update `AGENTS.md`, `README.md`, and `.rules/` in the **same** PR when
   behaviour, layout, commands, routes, or deploy shape change.
4. Run every gate in [`20-pre-prod-local.md`](20-pre-prod-local.md) locally.
   Green locally is the precondition for "ready", staging, and prod.
5. Bump the version per [`50-versioning.md`](50-versioning.md).
6. Fill the PR template's attestation honestly. Do not tick a box you did not
   earn.

## Self-enforcement

```bash
cargo run -p xtask -- rules-check
```

That gate fails if `docs/` comes back, if a `.rules/` file goes missing, if
this overview stops naming a numbered file, if `AGENTS.md` / `README.md` stop
pointing here, if `AGENTS.md` / `40-agents.md` /
`contracts/THREAT_MODEL.md` stop linking
[`70-secrets-mnemonics.md`](70-secrets-mnemonics.md), if a GitHub workflow
uses `PROD_ROTATE_MNEMONIC` or any `secrets.*MNEMONIC*` transport, if
`.dockerignore` / `.gitignore` drop `**/*mnemonic*`, if the PR template
drifts from the attestation CI requires, if `20-pre-prod-local.md` stops
listing a command CI actually runs, or if any markdown link in these surfaces
points at a file that does not exist.
