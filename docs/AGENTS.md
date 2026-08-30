# AGENTS.md — docs

How to treat documentation in this repo.

## Normative vs non-normative

| Kind | Paths | Treat as |
|------|-------|----------|
| **Normative** | `ARCHITECTURE.md`, `NAMING.md`, frozen specs (`BUNDLE_SPEC.md`, `DESIGN_CHALLENGE.md`, `PRISM.md`, …), `THREAT_MODEL.md`, `OPERATOR_SECURITY.md`, `COMPLETENESS.md`, `runbooks/`, `external-miner/` | Source of truth for contracts, ops, naming, and status |
| **Non-normative** | `evidence/`, `spikes/` | Historical ops notes / experiments. **Do not** implement against them as spec; **do not** delete in cleanup passes without an explicit ops decision |

When a spike or evidence report conflicts with a frozen spec or runbook, the normative doc wins.

## Runbook index

| Runbook | Use when |
|---------|----------|
| [`runbooks/promote-rollback-restore.md`](runbooks/promote-rollback-restore.md) | Digest promote, rollback, Postgres backup/restore |
| [`runbooks/local-testnet-e2e.md`](runbooks/local-testnet-e2e.md) | Local laptop/VM full subnet stack on testnet 541 + ephemeral gateway tunnel |
| [`runbooks/staging-testnet-e2e.md`](runbooks/staging-testnet-e2e.md) | Staging droplet testnet end-to-end validation |
| [`runbooks/trust-root-rotation.md`](runbooks/trust-root-rotation.md) | Trust-root key rotation |
| [`runbooks/gateway-failover.md`](runbooks/gateway-failover.md) | Gateway kill/restart / failover checks |
| [`runbooks/measurement-repin-socket-proxy.md`](runbooks/measurement-repin-socket-proxy.md) | Socket-proxy measurement re-pin |
| [`runbooks/design-enable-and-emission.md`](runbooks/design-enable-and-emission.md) | Design keygen + emission unlock |
| [`runbooks/prism-enable-lium-and-emission.md`](runbooks/prism-enable-lium-and-emission.md) | Prism Lium + emission |

Deploy topology and CI lanes: [`../deploy/README.md`](../deploy/README.md) and [`../deploy/AGENTS.md`](../deploy/AGENTS.md).  
Repo-wide agent contract: [`../AGENTS.md`](../AGENTS.md).

## Challenge public miner repos

Public miner docs live **outside** this monorepo (examples + human guides only — no control-plane code):

| Challenge | Repo |
|-----------|------|
| Relearn | [`CortexLM/relearn`](https://github.com/CortexLM/relearn) |
| Bounty | this repo [`external-miner/bounty.md`](./external-miner/bounty.md) |

`docs/external-miner/relearn.md` is the short Cortex pointer. The long Relearn guide lives in [`CortexLM/relearn`](https://github.com/CortexLM/relearn). Bounty pairing lives in this repo. Validators: [`external-miner/validators.md`](./external-miner/validators.md). When challenge APIs change, update **both** the public repo (when one exists) and `external-miner/` (see root [`../AGENTS.md`](../AGENTS.md) § Challenge public docs).

## Challenge / local E2E verification

When updating challenge or local-subnet docs/runbooks, keep these invariants:

- **Master-only eval** — `relearn-challenge` and `bounty-challenge` run on master; validator has **no challenge exec** (fetch sealed weights only).
- **Simulate submissions** — Relearn: `POST /v1/submissions` then poll `GET /v1/submissions/{id}`; Bounty: pair + `POST /v1/reports`. Do not treat `/health` alone as proof. A regression must not become champion.
- **Relearn promote** — after clean `awaiting_admin`, operator bearer `POST /v1/admin/promote`; then leaf → seal path.
- **Bounty adjudicate** — operator bearer `POST /v1/admin/adjudicate` (`valid` / `already_fixed_not_prod` / `invalid_malicious` / `duplicate`).
- **No host Sim in staging/prod** — Docker only; `SimSandbox` / `BASE_ALLOW_HOST_SIM` are CI/local opt-in.
- **Seal path** — `POST /v1/weights/raw` → seal → `GET /v1/weights/latest` with `sealed: true` (unsealed burn fallback is always available). That path needs `challenge_sk` + `gateway_sk`, **not** a gateway owner wallet. Validator wallets are for on-chain submit only.
- Normative local procedure: [`runbooks/local-testnet-e2e.md`](runbooks/local-testnet-e2e.md). Repo contract: [`../AGENTS.md`](../AGENTS.md) § Challenge verification.
