# 40 — `AGENTS.md` must stay accurate

[`AGENTS.md`](../AGENTS.md) at the repo root is the agent and operator
contract: the monorepo map, the non-negotiables, wallet/key roles, challenge
verification duties, the gate list, and where to read what. It is not a
human doc article, so it survives the no-`docs/` rule — but only while it is
true.

`deploy/AGENTS.md` is the deploy-scoped equivalent and is subject to the same
duty.

## Update it in the same PR when you change

| Change | What to fix in `AGENTS.md` |
|--------|----------------------------|
| Add / remove / rename a crate, bin, or top-level directory | monorepo map table |
| Add / remove / rename an HTTP route, admin route, or auth requirement | challenge verification section; the matching [`contracts/`](contracts/README.md) file |
| Change a challenge's rounds, quotas, scoring, or elimination | verification section; `contracts/DESIGN_CHALLENGE.md` / `contracts/PRISM.md`; [`contracts/external-miner/`](contracts/external-miner/README.md); the public miner repo |
| Add / remove / rename a local or CI command | commands + required-gates lists (and [`20-pre-prod-local.md`](20-pre-prod-local.md), `README.md`, `ci.yml`) |
| Change wallet, key, or token requirements | wallet / key roles table |
| Change deploy topology, compose profile, or secret path | non-negotiables + `deploy/AGENTS.md` |
| Add a rule agents must follow | a numbered `.rules/` file, then link it from `AGENTS.md` |

## Invariants `AGENTS.md` must keep stating

- Read `.rules/` first — that line stays at the top.
- Gateway runs on **master only**; validators point at the master gateway and
  have **no challenge exec**.
- `evil-gateway` is test-only and never enabled on prod hosts.
- Digest-only images in deploy paths.
- Secrets via age under `deploy/env/` / `deploy/secrets/`, never baked into
  images or cloud-init.
- `BASE_*` env vars, deployed paths, GHCR package paths, and `base-*-v1`
  domain tags are frozen ([`60-naming.md`](60-naming.md)).
- BIP39 mnemonics, `secretPhrase`, and raw hotkey / coldkey seeds never leave
  a mode-`0600` file: not in argv, Chat, GitHub Actions secrets, compose
  `environment:`, or a `644` agent `audit.jsonl`
  ([`70-secrets-mnemonics.md`](70-secrets-mnemonics.md)).
- `GET /v1/weights/latest` is fail-closed: a burn vector, never a 404.
- `unsafe_code = forbid`; no `unwrap` / `expect` in non-test code.

## Machine check

`cargo run -p xtask -- rules-check` fails if `AGENTS.md` stops pointing at
`.rules/` in its first lines, if this file / `AGENTS.md` /
`contracts/THREAT_MODEL.md` stop linking
[`70-secrets-mnemonics.md`](70-secrets-mnemonics.md), or if any of its
relative links dangle. It cannot tell you that a table is out of date — that
part is your job, and it is the reason the PR attestation exists.
