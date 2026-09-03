# Cortex architecture

Operator-facing map of the control plane. Normative byte contracts live in the frozen specs:

| Spec | Status | Role |
|------|--------|------|
| [`BUNDLE_SPEC.md`](./BUNDLE_SPEC.md) | **FROZEN** | Epoch bundle SCALE layout, merkle, aggregation, on-chain payload bounds |
| [`DESIGN_CHALLENGE.md`](./DESIGN_CHALLENGE.md) | archived freeze | Retired `design` product (not live) |
| [`PRISM.md`](./PRISM.md) | archived | Retired `prism` product (Lium rails reused by Relearn) |
| [`RELEARN.md`](./RELEARN.md) | live | `relearn` post-training factory (HTTP submit; miners pay Lium) |
| [`RELEARN-IMAGE.md`](./RELEARN-IMAGE.md) | live | `relearn-image` — Cosmos3 fine-tunes judged by Q-Judger (crates keep the `t2i` spelling) |
| [`RELEARN-AGENT.md`](./RELEARN-AGENT.md) | live | `relearn-agent` — replayed tool traces on the same base checkpoint as `relearn` |
| [`RELEARN-MM.md`](./RELEARN-MM.md) | **off** | `relearn-mm` — no trust-root row, no emission; `mm` compose profile only |
| [`BOUNTY.md`](./BOUNTY.md) | live | `bounty` — paired bug reports |

Do not restate those contracts here. Link them.

Audiences (do not mix):

- Miners: [`external-miner/README.md`](./external-miner/README.md) indexes all four challenges
- Validators: [`external-miner/validators.md`](./external-miner/validators.md)

Security claim and what it excludes: [`THREAT_MODEL.md`](./THREAT_MODEL.md).  
Operator checklist: [`OPERATOR_SECURITY.md`](./OPERATOR_SECURITY.md).  
Runbooks: [`runbooks/`](./runbooks/).

---

## 1. Goals

- Lighter Rust control plane than the prior Python stack.
- Gateway runs **only** as subnet owner (master). Startup asserts hotkey == on-chain `SubnetOwnerHotkey` or exits `2` before bind.
- Validators **recompute** the weight vector from a signed, merkle-rooted epoch bundle. Challenge keys and measurements come from **owner-signed local files**, never from gateway HTTP.
- CRV4 timelock commit-reveal on Bittensor testnet/mainnet as configured. Reveal is automatic on-chain.
- Live challenges accept miner work over **HTTP** (Relearn → Lium/sim eval; Bounty → pair + reports). Miners pay Lium when a key is present.

---

## 2. Process topology

```text
                    ┌─────────────────────────────────────┐
                    │  Master host (compose profile master) │
                    │  postgres · gateway · validator ·     │
                    │  updater · socket-proxy ·             │
                    │  relearn-challenge ·                  │
                    │  relearn-t2i-challenge (relearn-image)│
                    │  relearn-agent-challenge ·            │
                    │  bounty-challenge                     │
                    └───────────────┬─────────────────────┘
                                    │ TLS terminates in gateway (D20)
                                    │ /challenge/{id}/*  /v1/bundle/*
                    ┌───────────────▼─────────────────────┐
                    │  Other validator hosts (no gateway,   │
                    │  no challenge services / socket-proxy)│
                    │  validator · local trust roots        │
                    │  peer root exchange (HTTPS + hotkey)  │
                    └───────────────┬─────────────────────┘
                                    │ HTTP submit
                    ┌───────────────▼─────────────────────┐
                    │  Miner clients                       │
                    │  relearn artifact digest + Lium BYOK  │
                    │  image / agent artifact + manifest    │
                    │  bounty pair + bug reports            │
                    └─────────────────────────────────────┘
```

| Binary / crate | Role |
|----------------|------|
| `gateway` | Master-only: registry, reverse proxy, bundle seal/serve, sole TLS owner; mounts marketing [`SITE_API.md`](./SITE_API.md) (`GET /v1/site/*`) |
| `validator` | Fetch/mirror bundle, verify, recompute, peer cross-check, CRV4 submit, dissent |
| `relearn-challenge` | **Master-only:** digest freeze, operator holdout unseal (commitment-checked), Lium/sim eval, contamination / public–holdout / canary / shuffle gates, operator promote, sign leaves |
| `relearn-t2i-challenge` | **Master-only** (serves `relearn-image`): frozen prompt cells at pinned seeds, Q-Judger scoring, pillar / replay / contamination / capability-canary gates, sign leaves |
| `relearn-agent-challenge` | **Master-only:** episode replay, trace grounding, tool-ablation and observation-shuffle arms, contamination / canary gates, sign leaves |
| `relearn-mm-challenge` | **Off** (`mm` profile): text-intact rerun, vision + agentic holdout with a pixel-shuffle control. No trust-root row, so it emits nothing |
| `bounty-challenge` | **Master-only:** internal pair/reports/adjudicate; **reads** CortexLM/backend public API for scoring; sign leaves |
| `updater` | Digest-pinned rollouts via `docker-socket-proxy` (master) |
| `trustroot` | Offline keygen / sign / verify for owner-signed TOML |
| `bundle` | SCALE types, seal, verify (`PROTOCOL_VERSION`) |
| `aggregate` | Integer aggregation (Hamilton house 65535) |
| `chain` | Chain client trait + SDK wiring |
| `trustroot` (lib) | Load local signed challenges/measurements; dual-accept rotation |
| `base-attest-*` | Parse / replay / policy for TDX quotes (bundle measurement pin) |
| `crosscheck` / `dissent` | Peer roots and three-outcome policy |
| `db` | Postgres persistence (bundles, evidence, dissent, challenge tables) |
| `xtask` | loc-cap, consensus-lint, metadata-snapshot, spec / design / external-docs gates |

---

## 3. Data flow (one epoch)

1. **Pin.** Gateway (or seal path) pins `block_hash` / metagraph root at epoch boundary.
2. **Leaves.** Challenge backends produce challenge-signed `Score` or `NoScore` leaves for the **validator-derived** expected set (D24). Tip epochs may **supersede** a leaf when the signed `payload_digest` changes for the same `(challenge, epoch, miner)`; identical digests stay idempotent.
3. **Seal.** Gateway builds `EpochBundleV1`, computes merkle root, signs the body. Tip reseal appends `epoch_bundle.revision` when leaves/merkle change; no-op if identical. **Does not** put the merkle root into the on-chain weight payload (there is no field; see BUNDLE_SPEC §12 / D5).
4. **Distribute.** `GET /v1/weights/latest` serves the newest revision of the highest chain-scale sealed epoch (`sealed: true` only for Match). Validators may also **mirror** from peers (content-addressed by root).
5. **Verify.** Each validator loads **local** `challenges.toml` + `measurements.toml` (owner-signed). Rejects leaves whose keys are not in the local trust root (D18).
6. **Cross-check.** Hotkey-authenticated peer root exchange; minimum sample (D26). Persist signed bundle + peer statements as local evidence.
7. **Recompute.** Integer aggregation per BUNDLE_SPEC. Compare to gateway `final_vector`.
8. **Outcomes (D6).** Class A: submit local vector + dissent. Quarantine: drop bad challenges if share mass survives. Class B: no submit + dissent + alarm.
9. **Submit.** `WeightsTlockPayload { hotkey, uids, values, version_key }` only. CRV4 reveal round from schedule inputs (D22). Never invent a round; never downgrade to plain `set_weights` while CR is enabled.

---

## 4. Trust roots (local only)

| Artifact | In git? | Loaded by |
|----------|---------|-----------|
| `config/owner.pubkey` | yes (public) | validators, ceremony verify |
| `config/challenges.toml` + `.sig` | yes | every validator from **disk** |
| `config/measurements.toml` + `.sig` | yes | every validator from **disk** |
| Challenge / owner mini-secrets | **never** | challenge service / offline ceremony only |

Current emission posture: `relearn = 4000`, `relearn-image = 1500`,
`relearn-agent = 1500`, `bounty = 3000` bps (sum 10000; operator can retune).
`relearn-mm` has no row, so it earns 0 and a leaf claiming that id fails the
trust-root check. Each live challenge signs leaves under its **own** key; no
two rows share one.

Gateway DB is **routing only**. It is never a source of challenge keys, emission shares, or measurements (D18, D23).

Ceremony: [`config/CEREMONY.md`](../config/CEREMONY.md).  
Rotation: [`runbooks/trust-root-rotation.md`](./runbooks/trust-root-rotation.md) (D21).  
Design emission unlock: [`runbooks/design-enable-and-emission.md`](./runbooks/design-enable-and-emission.md).

---

## 5. Compose profiles

| Profile | Services |
|---------|----------|
| default | postgres, validator, updater, socket-proxy, challenge backends |
| `master` | + gateway (owner host only); challenges stay on master |
| `role-validator` overlay | disables gateway, updater, challenges, socket-proxy |
| `evil-gateway` | **test-only** adversarial harness (task 48). Never prod. |

See [`deploy/README.md`](../deploy/README.md) and root [`docker-compose.yml`](../docker-compose.yml).

---

## 6. What this architecture does **not** claim

See D19 in [`THREAT_MODEL.md`](./THREAT_MODEL.md). Short form:

- Challenge score honesty is out of scope.
- Owner honesty is out of scope (owner signs roots and runs gateway).
- Non-equivocation is **peer-consensus + local evidence**, not a public on-chain `(epoch → bundle_root)` anchor.
- Gateway HA is **not** claimed (R9): restart policy + manual failover only.

---

## 7. Related docs

| Doc | Purpose |
|-----|---------|
| [`THREAT_MODEL.md`](./THREAT_MODEL.md) | D19 verbatim, D5, D11, R12 |
| [`OPERATOR_SECURITY.md`](./OPERATOR_SECURITY.md) | Checklist |
| [`runbooks/trust-root-rotation.md`](./runbooks/trust-root-rotation.md) | D21 dual-accept |
| [`runbooks/promote-rollback-restore.md`](./runbooks/promote-rollback-restore.md) | Digest promote, rollback, `pg_dump` |
| [`runbooks/gateway-failover.md`](./runbooks/gateway-failover.md) | Manual failover (R9) |
| [`runbooks/design-enable-and-emission.md`](./runbooks/design-enable-and-emission.md) | Design keygen + emission |
| [`external-miner/README.md`](./external-miner/README.md) | Miner HTTP path + `protocol_version` badge |
| [`../README.md`](../README.md) | Repo bootstrap |
