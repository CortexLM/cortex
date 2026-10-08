# Hypertrain docs

- [Operator guide](operator.md): local stack, image and canary, launching a run, auditors, honeypots, production on a Cortex master, budget and teardown.
- [Miner guide](miner.md): hardware, config, admission and bond, determinism, audits and fault payments, troubleshooting.
- [Protocol specification](protocol.md): envelopes, signatures, leaf scheme, round state machine, ledger, determinism checklist.
- [Mechanism specification](mechanism.md): audit math, adaptive audit rate, rewards, escrow and clawback, burn accounting, residual risks.
- [Parity specification](parity.md): topology for many replicas across datacenters, formulas, schedule, pre-registered TOST protocol, CPU grid results.
- [Challenge routes](challenge-routes.md): every HTTP route, body, auth and status code.
- [Message schemas](schemas/): JSON Schema for each signed message ([RunManifest](schemas/RunManifest.json), [Commit](schemas/Commit.json), [DeltaManifest](schemas/DeltaManifest.json), [Receipt](schemas/Receipt.json), [Forfeit](schemas/Forfeit.json), [Dispute](schemas/Dispute.json), [Finalize](schemas/Finalize.json) and the rest). Regenerate with `uv run --frozen python -m hypertrain.protocol.schema docs/schemas`.
- [Design notes](README.md).

Every local command in the two guides is run by `bash scripts/doctest_docs.sh`. Production-only commands are tagged `doctest-skip:prod`; the script lists them without running them.
