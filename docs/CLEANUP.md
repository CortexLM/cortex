# Cleanup scope and retained contracts

This cleanup removes obsolete implementation paths and clarifies the purpose of
Cortex. It does not implement the whitepaper's future mechanisms or change
scoring, signatures, migrations, image pins, or reward allocation.

## Removed

| Item | Evidence and replacement |
|------|--------------------------|
| `chain::NotImplementedChain`, `chain::LiveRpcChain`, and the old `chain/live` feature | No workspace consumers; runnable services use `chain_live::LiveChainClient`. The `ChainClient` trait and deterministic `FakeChain` test backend remain |
| Gateway `upstream_uri` | Unused private helper. Production uses `upstream_url`; regression tests now exercise that path directly |
| Prism similarity-v2 prompt and unused key-path helper | No active callers; the embedded v3 prompts and version tags are unchanged |
| Metadata storage type fields and key-flattening helper | Stored or computed values had no readers. Both supported metadata versions use the same upstream storage-entry type, so the projection is shared |
| Proof `mean_lattice` | Only its own test called it. Current payout uses the sum of WTA/discovery topic masses, not this obsolete binary average |
| Unused direct and development dependencies | Checked against package sources and target builds; manifests and the lockfile change together |
| SSH return-code dead-code allowance | The field is read by the Lium client; the field remains, only the unnecessary suppression is removed |

Storage projection tests cover hasher order, default bytes, and plain entries.
Gateway tests cover query strings and slash joining. Existing payout tests still
cover skipped topics, exact ties, discovery splits, and empty topic sets.

## Deliberately retained

- **Proof's `prism-*` dependencies:** the Lium harvest path still uses them.
  A retired product name does not make its shared libraries dead code.
- **Applied SQL migrations:** deployed databases may already contain those
  tables. History is not disposable scaffolding.
- **`BASE_*`, deployed paths, crypto domains, and compatibility routes:** these
  protect existing configuration, clients, signatures, and measurement pins.
  See [Naming](NAMING.md).
- **Frozen specs, retired miner links, evidence, and spikes:** they preserve
  verification contracts and operational history.
- **Explicit fail-closed interfaces and local test backends:** missing live
  wiring is documented, not hidden by deleting guards or inventing scores.

## Documentation changes

The [overview](OVERVIEW.md) explains research reuse for new readers and investors.
The [whitepaper guide](WHITEPAPER.md) separates the proposal from implementation.
Architecture, contribution, miner, and deployment references now use that
positioning while retaining technical contracts.

The audit also records existing Proof gaps, including partial judging,
in-memory records, and missing automatic emission. These need separate
implementation and end-to-end validation, not a branding change.
