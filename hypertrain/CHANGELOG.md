# Changelog

## [0.1.0] - unreleased

### Added
- Signed protocol, JSON schemas and ledger (delayed rewards, burn/clawback).
- Challenge service, trainer (deterministic rounds, island layouts), auditor, aggregator, miner client.
- drand-beacon sampled replay audits with Merkle commitments.
- CPU simulations: DiLoCo state-policy study, parity grid, mechanism tables.
- GPU determinism Phase A on RTX 5090 hosts (bitwise-identical across 3 hosts).
- `kimi-k3-pro-tiny` test architecture (MLA, shared-expert MoE, aux-loss-free routing bias),
  opt-in through `hypertrain.models.kimi_tiny.register()`; see `docs/adr/0006` at the monorepo root.
- Docs, CI, image and release workflows, Dockerfiles.

### Changed
- CUDA network qualification accepts the SM count pinned by the signed manifest's
  `reference_spec.sm_count` instead of only 170, and rejects any device off that count with
  `qualification_sm_profile_mismatch` (opt-in B300 support).

### Not yet
- Quality parity with centralized training is not claimed.
