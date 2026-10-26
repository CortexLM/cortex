# Changelog

## [0.1.0] - unreleased

### Added
- Signed protocol, JSON schemas and ledger (delayed rewards, burn/clawback).
- Challenge service, trainer (deterministic rounds, island layouts), auditor, aggregator, miner client.
- drand-beacon sampled replay audits with Merkle commitments.
- CPU simulations: DiLoCo state-policy study, parity grid, mechanism tables.
- GPU determinism Phase A on RTX 5090 hosts (bitwise-identical across 3 hosts).
- `kimi-k3-pro-tiny` test architecture (MLA, shared-expert MoE, aux-loss-free routing bias),
  permanently registered; see `docs/adr/0006` at the monorepo root.
- Read-only model registry `hypertrain.models.registry` (arch -> module, per-arch rank bound and
  reduction orders), covered by `implementation_hash`. `ModelSpec.arch` names a registry entry
  (omitted = `decoder`; existing manifest bytes and run_ids unchanged); v2 manifests carry
  `kimi-k3-pro-tiny` on a one-GPU island, v1 manifests refuse it (D18, ADR 0006).
- Docs, CI, image and release workflows, Dockerfiles.

### Not yet
- Quality parity with centralized training is not claimed.
