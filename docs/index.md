# Cortex documentation

- [Architecture](ARCHITECTURE.md): processes, source map and trust boundaries.
- [Proof](PROOF.md): topic setup, recursive agent, VM evaluation and rewards.
- [Bounty](BOUNTY.md): pairing, intake, external-feed scoring and emission.
- [Threat model](THREAT_MODEL.md): security claims, trust boundaries and limits.
- [Operator security](OPERATOR_SECURITY.md): deployment and release checklist.
- [Configuration](reference/configuration.md): Python services and credential files.
- [Trust-root ceremony](how-to/trust-root.md): generate keys, sign and verify operator documents.
- [Build a Proof guest rootfs](how-to/build-guest-rootfs.md): convert the pinned Python guest stage into measured ext4 bytes.
- [Proof miner guide](external-miner/proof.md): discover topics and submit signed artifacts.
- [Bounty miner guide](external-miner/bounty.md): pair an account and report a vulnerability.
- [Validator guide](external-miner/validators.md): verify sealed weights and submit on-chain.
- [Bundle specification](BUNDLE_SPEC.md): frozen consensus wire contract.
- [Naming](NAMING.md): preserved `BASE_*` variables and signature domains.
- [Deployment](../deploy/README.md): master, validator and dedicated KVM host.

The archived [Design](DESIGN_CHALLENGE.md), [Prism](PRISM.md) and
[Relearn](external-miner/relearn.md) documents describe retired products. Their
linked specifications and research references remain historical material, not
live work or additional emission recipients. Historical evidence is not proof
that this Python rewrite has been deployed.
