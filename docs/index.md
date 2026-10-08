# Cortex documentation

Start here: [Cortex ecosystem](ecosystem.md) and [Validator quickstart](validator-quickstart.md).

## Overview

- [Ecosystem](ecosystem.md): how Cortex relates to the apps and the challenges.
- [Architecture](ARCHITECTURE.md): processes, source map and trust boundaries.
- [Threat model](THREAT_MODEL.md): security claims, trust boundaries and limits.
- [Operator security](OPERATOR_SECURITY.md): deployment and release checklist.

## Challenges

- [Challenge containers](CHALLENGES.md): contract, leaves, proxy, registry and auto-updates.
- [Proof](PROOF.md): topic setup, recursive agent, VM evaluation and rewards.
- [Bundle specification](BUNDLE_SPEC.md): frozen consensus wire contract.
- [Naming](NAMING.md): preserved `BASE_*` variables and signature domains.

## Run a validator

- [Validator quickstart](validator-quickstart.md): one script, verify-only mode and FAQ.
- [Validator guide](external-miner/validators.md): the underlying CLI and trust files.

## Mine

- [Miner guides](external-miner/README.md): installation and challenge overview.
- [Proof miner guide](external-miner/proof.md): discover topics and submit signed artifacts.
- [Bounty miner guide](external-miner/bounty.md): pointer to the CortexLM/bounty container.

## Operate

- [Configuration](reference/configuration.md): Python services and credential files.
- [Trust-root ceremony](how-to/trust-root.md): generate keys, sign and verify operator documents.
- [Build a Proof guest rootfs](how-to/build-guest-rootfs.md): convert the pinned Python guest stage into measured ext4 bytes.
- [Deployment](../deploy/README.md): master, validator and dedicated KVM host.

Historical evidence is not proof that this Python rewrite has been deployed.
