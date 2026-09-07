# Atlas

Atlas is Cortex's Proof reward agent. This directory contains the complete
Prime Agent source fork, not an API-only imitation of its recursive runtime.
See `UPSTREAM.json` for the exact import and `LICENSE` for the MIT notices.
The upstream README describes the original product.

The Cortex integration lives in `packages/coding-agent/src/cortex/`.
The experiment and reward profiles use separate root sessions, kernels,
workspaces, capabilities, and evidence scopes. Atlas is a reward proposer,
not a wallet, infrastructure administrator, or source of trusted measurements.

## Trust boundary

The upstream Python runtime is **not a security sandbox**. Cortex runs it in
an externally restricted environment. Miner code must not share provider
credentials, signing keys, daemon sockets, or write access to authoritative
evidence with the agent controller.

Lium machines remain under the miner's account. Image commitments, monitoring,
fresh probes and externally retained logs do not prove that an administrator
has not falsified a measurement. This residual risk is part of the Proof
policy; it must never be described as complete fraud prevention.

## Development

Follow `AGENTS.md`. Install the existing lockfile with `npm ci --ignore-scripts`
and run `npm run check`; run individual changed test files from their package.
Use only local faux providers for integration tests. Importing the fork does
not authorize calling a paid model, renting a GPU, or publishing evidence.

Keep upstream notices and tests. Record intentional Cortex changes separately
from upstream upgrades so the fork remains reviewable.
