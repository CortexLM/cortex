# Cortex architecture

Technical map of Cortex's autonomous research network. For the purpose and
research-reuse model, start with the [overview](OVERVIEW.md). The
[whitepaper comparison](WHITEPAPER.md) distinguishes proposed mechanisms from
current implementation. Normative byte contracts live in the frozen specs:

| Spec | Status | Role |
|------|--------|------|
| [`BUNDLE_SPEC.md`](./BUNDLE_SPEC.md) | **FROZEN** | Epoch bundle SCALE layout, merkle, aggregation, on-chain payload bounds |
| [`DESIGN_CHALLENGE.md`](./DESIGN_CHALLENGE.md) | archived freeze | Retired `design` product (not live) |
| [`PRISM.md`](./PRISM.md) | archived | Retired `prism` product (Lium rails reused by Proof harvest) |
| [`BOUNTY.md`](./BOUNTY.md) | live | `bounty` — paired bug reports |
| [`PROOF.md`](./PROOF.md) | live | `proof` — operator-published research topics, digest-pinned RLM judge |

Do not restate those contracts here. Link them.

Audiences (do not mix):

- Miners: [`external-miner/README.md`](./external-miner/README.md) indexes the two live challenges (`bounty`, `proof`)
- Validators: [`external-miner/validators.md`](./external-miner/validators.md)

Security claim and what it excludes: [`THREAT_MODEL.md`](./THREAT_MODEL.md).  
Operator checklist: [`OPERATOR_SECURITY.md`](./OPERATOR_SECURITY.md).  
Runbooks: [`runbooks/`](./runbooks/).

---

## 1. Goals

- Coordinate reproducible research and bug reports, evaluate evidence, and verify reward allocation.
- Gateway runs on the **master** host. With `BASE_GATEWAY_REQUIRE_OWNER=1`, startup asserts hotkey == on-chain `SubnetOwnerHotkey` or exits `2` before bind; advisory local/staging configuration is distinct.
- Validators **recompute** the weight vector from a signed, merkle-rooted epoch bundle. Challenge keys and measurements come from **owner-signed local files**, never from gateway HTTP.
- CRV4 timelock commit-reveal on Bittensor testnet/mainnet as configured. Reveal is automatic on-chain.
- Live challenges accept miner work over **HTTP** (Bounty → pair + reports; Proof → topic_id + artifact). Proof miners pay Lium when a key is present.

---

## 2. Process topology

```text
Miner clients
  │ HTTP: Bounty pair/reports or Proof topic/experiment
  ▼
Master host (role-master overlay + master profile)
  gateway · postgres · socket-proxy
  bounty-challenge · proof-challenge
  │                    │
  │                    ├─ Proof harvest → Lium evaluation pod (nll / throughput)
  │                    └─ HTTPS + bearer file → dedicated KVM host (custom topics)
  │                         proof-vm-orchestrator: jailer/Firecracker RLM VM per topic,
  │                         sister Firecracker guest (no network) per miner run, or —
  │                         for topics whose signed params select an in-guest runner —
  │                         one experiment Firecracker VM per paid job (pack staged
  │                         over vsock, operator adaptor inside, destroyed after)
  │ signed bundles
  ▼
Validator host (role-validator overlay)
  validator · postgres · local owner-signed trust roots
  │ peer cross-checks; no challenge execution
  ▼
Bittensor: verified reward weights
```

The master overlay disables the co-located validator; local E2E may explicitly
enable one. `updater` is an optional `auto-update` profile. TLS currently
terminates in the host reverse proxy, not in the gateway process.

| Binary / crate | Role |
|----------------|------|
| `gateway` | Master-only: registry, reverse proxy, bundle seal/serve; mounts public website [`SITE_API.md`](./SITE_API.md) (`GET /v1/site/*`) |
| `validator` | Fetch/mirror bundle, verify, recompute, peer cross-check, CRV4 submit, dissent |
| `bounty-challenge` | **Master-only:** internal pair/reports/adjudicate; **reads** CortexLM/backend public API for scoring and signs leaves from those rows. An unreadable feed pays nobody — `E` is covered with `ChallengeInternal`, share burns to uid 0 — rather than scoring offline |
| `proof-challenge` | **Master-only:** signed topics, holdout loading, evaluation orchestration. Library payout is a sum of WTA/discovery topic masses; the binary has no automatic leaf-emission loop yet |
| `proof-vm-orchestrator` | **Host with a working `/dev/kvm`** — production: a dedicated DO droplet (`g-8vcpu-32gb`, nyc1, nested `/dev/kvm`) on the VPC, never colocated on the CP; staging: colocation on the CP droplet with nested `/dev/kvm` is an allowed exception, proven on `cortex-staging` (fragile → provision the dedicated droplet if the boot fails); never Lium: Firecracker + jailer agent behind HTTPS + a bearer file. One RLM microVM per Proof topic from the digest the control plane pins, sister miner guest with no network per paid run, host-stamped `sandboxed` / `flops_used`; for topics whose signed params select an in-guest runner, one dedicated experiment microVM per paid job under configurable caps (lock 16 vCPU / 32 GiB, disk ≥ 16 GiB), pinned pack staged over vsock, destroyed after the job. Client side is `proof-vm-fc::FirecrackerOrchestrator`; runbooks [`runbooks/proof-vm-orchestrator.md`](runbooks/proof-vm-orchestrator.md), [`runbooks/proof-experiment-vms.md`](runbooks/proof-experiment-vms.md) |
| `updater` | Digest-pinned rollouts via `docker-socket-proxy` (master) |
| `trustroot` | Offline keygen / sign / verify for owner-signed TOML |
| `bundle` | SCALE types, seal, verify (`PROTOCOL_VERSION`) |
| `aggregate` | Integer aggregation (Hamilton house 65535) |
| `chain` / `chain-live` | Shared chain trait + deterministic test backend / production JSON-RPC client and signed weight submission |
| `trustroot` (lib) | Load local signed challenges/measurements; dual-accept rotation |
| `base-attest-*` | Parse / replay / policy for TDX quotes (bundle measurement pin) |
| `crosscheck` / `dissent` | Peer roots and three-outcome policy |
| `db` | Postgres persistence (bundles, evidence, dissent, challenge tables) |
| `xtask` | loc-cap, consensus-lint, metadata-snapshot, spec / design / external-docs gates |

---

## 3. Data flow (one epoch)

This is the bundle pipeline. Bounty drives its emitter; Proof's corresponding
payout/signing helpers still need service wiring. See
[implementation status](COMPLETENESS.md#proof-challenge).

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

Current emission posture: `bounty = 2000`, `proof = 8000` bps (sum 10000).
Proof-weighted 20%/80% regardless of eval digest. Proof's eval digest is
pinned (`sha256:78b614a1…`); live submits still 503 until harvest is wired,
a baseline is sealed, and ≥1 topic is open. Do not invent a sha256.
`relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and
`prism` have no row (product code removed), so they earn 0 and a leaf
claiming those ids fails the trust-root check. Each live challenge signs
leaves under its **own** key; no two rows share one.

The gateway database persists raw weights and sealed bundles; backend routing
remains in memory. Neither is **trust authority**: challenge keys, emission
shares, and measurements come from the owner-signed local files (D18, D23).

Ceremony: [`config/CEREMONY.md`](../config/CEREMONY.md).  
Rotation: [`runbooks/trust-root-rotation.md`](./runbooks/trust-root-rotation.md) (D21).

---

## 5. Compose profiles

| Profile | Services |
|---------|----------|
| default | postgres, validator, socket-proxy, bounty-challenge, proof-challenge |
| `master` | adds gateway; use the master role overlay on operator hosts |
| `role-master` overlay | disables validator; challenges stay on master |
| `role-validator` overlay | disables gateway, updater, challenges, socket-proxy; keeps postgres and validator |
| `auto-update` | optional updater; not part of the default stack |
| `evil-gateway` | **test-only** adversarial harness (task 48). Never prod. |

See [`deploy/README.md`](../deploy/README.md) and root [`docker-compose.yml`](../docker-compose.yml).

---

## 6. What this architecture does **not** claim

See D19 in [`THREAT_MODEL.md`](./THREAT_MODEL.md). Short form:

- Challenge score honesty is out of scope.
- Owner honesty is out of scope (owner signs roots and runs gateway).
- Non-equivocation is **peer-consensus + local evidence**, not a public on-chain `(epoch → bundle_root)` anchor.
- Gateway HA is **not** claimed (R9): restart policy + manual failover only.
- A complete autonomous research judge, durable shared research collection, and
  synthesis/adoption loop are **not** implemented. See [the paper-to-code comparison](WHITEPAPER.md).

---

## 7. Related docs

| Doc | Purpose |
|-----|---------|
| [`THREAT_MODEL.md`](./THREAT_MODEL.md) | D19 verbatim, D5, D11, R12 |
| [`OPERATOR_SECURITY.md`](./OPERATOR_SECURITY.md) | Checklist |
| [`runbooks/trust-root-rotation.md`](./runbooks/trust-root-rotation.md) | D21 dual-accept |
| [`runbooks/promote-rollback-restore.md`](./runbooks/promote-rollback-restore.md) | Digest promote, rollback, `pg_dump` |
| [`runbooks/gateway-failover.md`](./runbooks/gateway-failover.md) | Manual failover (R9) |
| [`external-miner/README.md`](./external-miner/README.md) | Miner HTTP path + `protocol_version` badge |
| [`../README.md`](../README.md) | Repo bootstrap |
