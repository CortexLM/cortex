# Security policy

## Reporting a vulnerability

Use **GitHub private vulnerability reporting** on this repository:

<https://github.com/CortexLM/cortex/security/advisories/new>

Do **not** open a public issue for a vulnerability.

There is no public security email. If private reporting is unavailable,
contact the maintainers listed in [CODEOWNERS](CODEOWNERS) through GitHub
maintainer tools (do not paste secrets into a public mention).

Please include:

- Affected component (`gateway`, `validator`, a challenge binary, a crate)
- Cortex commit SHA or release tag (`v*.*.*`)
- Impact and a minimal reproduction **without** exploit payload dumps
- Whether the issue is already public

## Scope

In scope: this control-plane repo — gateway, validator, challenge services,
deploy compose/scripts as documented, and the sealed-weight path.

Out of scope for this document: miner-submitted harnesses (untrusted by
design), third-party GPU clouds, and public miner-doc repos that contain
examples only.

## What we will not change for a “security rename”

Do not treat leftover `BASE_*` environment names, `/opt/base` host paths, or
`base-*-v1` domain tags as vulnerabilities. Those strings are measured into
live miner CVMs and on-chain/signature domains. See
[`.rules/60-naming.md`](.rules/60-naming.md) and
[`.rules/contracts/THREAT_MODEL.md`](.rules/contracts/THREAT_MODEL.md).

## Supported versions

Fixes land on `main` and ship in annotated tags `v*.*.*`. Staging tracks
`main`; production is digest-pinned from those tags.
