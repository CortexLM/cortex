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

## Cortex v3 monorepo

- [ADR 0001: monorepo structure](adr/0001-monorepo-structure.md): layout, lockfiles, workflows, v3 images and the old-repository references to migrate.
- [ADR 0003: Cloudflare R2 data plane](adr/0003-cloudflare-data-plane.md): direct host-to-R2 bulk transfer, 4 MiB-block multipart, content-addressed keys, presigned part offers, cost model, the [multi-region measurement plan](dataplane/measurement-plan.md) and the [test resources](dataplane/resources.md).
- [ADR 0006: `kimi-k3-pro-tiny` test profile](adr/0006-kimi-k3-pro-tiny-profile.md): Kimi K2-family MLA + MoE test architecture, gap table, B300 qualification commands, and the D18 permanent registry entry and per-run `arch`.
- [ADR 0007: SM counts per CUDA architecture](adr/0007-sm-counts-per-arch.md): the (capability, SM count) table for CUDA qualification, hash consequences and the TEST-stack image swap.
- [ADR 0008: generic CUDA executor](adr/0008-generic-cuda-executor.md): references and audit replays on any SSH + Docker GPU host, host contract, provenance record, failure model, hash consequences.
- [ADR 0010: admission hardening](adr/0010-admission-hardening.md): rotation consent, trial expiry, the owner-signed challenge and job requests in a body envelope, dead unfunded joins and the optional per-prefix cap, the admission bond (release, seizure), the elastic epoch, hash consequences.
- [ADR 0012: executor performance](adr/0012-executor-performance.md): the CUDA compute cache mount, the TEST beacon cadence (model, shipped values, head-first pusher) and the release of the store lock during a remote reference.
- [ADR 0013: executor results through R2](adr/0013-executor-artifacts-via-r2.md): the result tar goes to presigned part URLs instead of the ssh stream, declare/offer exchange, per-attempt keys, failure model, which result sizes fit which windows, the 8 GiB object ceiling.
- [ADR 0015: the intermediate dense TEST model](adr/0015-intermediate-dense-test-model.md): the 100,676,352-parameter `dense-0.1b` profile and builder geometry, the measured-replay re-audit window and gap cap, the transfer and window budget, why it could not pass on the code it was written on (`2cfc2a2b`) and what still blocks it on one executor host, prerequisites a-f (a and b merged since), hash consequences, and the [replay runbook](runbooks/intermediate-model-replay.md) with its warm-up and real builds.
- [ADR 0016: the D94 dense TEST models](adr/0016-d94-dense-test-model.md): the 31,462,656-parameter `dense-0.03b` run profile and the 44,045,568-parameter `dense-0.05b` option, sized on the S17b measurement (miner side 217-282 s and T_ref 338 s at 101 M) by the rule miner side plus reference within 250 s, their own bands, unchanged hashes.
- [ADR 0017: the miner fast path of an admission trial](adr/0017-miner-fast-path.md): the probe's four files through one bounded upload window, the trial inputs fetched side by side, the genesis inputs built before the challenge and used only on the job's pin, what could not move and why, loopback before/after numbers, hash consequences (`sources()` and the bundle move, `implementation_hash` does not).
- [Hypertrain import provenance](provenance/hypertrain-import.md): commit map, byte-identity and `git log --follow` checks.
- [Hypertrain](../hypertrain/README.md): the imported project, with its own documentation in `hypertrain/docs/`.
- [B300 host run](b300-run.md): `run_host.sh` on an AWS p6-b300 host: steps, exit codes, results, auto-stop and the two-host comparison.
- [s100 TEST stack](s100-test-stack.md): swap the TEST challenge image, TEST auditors, a CPU TEST run mined from a remote host, rollback, and the CPU-capability pin.

## Run a validator

- [Validator quickstart](validator-quickstart.md): one script, verify-only mode and FAQ.
- [Validator guide](external-miner/validators.md): the underlying CLI, the gateway-served challenge profile and preflight.

## Mine

- [Miner guides](external-miner/README.md): installation and challenge overview.
- [Proof miner guide](external-miner/proof.md): discover topics and submit signed artifacts.
- [Bounty miner guide](external-miner/bounty.md): pointer to the CortexLM/bounty container.

## Operate

- [Configuration](reference/configuration.md): Python services and credential files.
- [Build a Proof guest rootfs](how-to/build-guest-rootfs.md): convert the pinned Python guest stage into measured ext4 bytes.
- [Deployment](../deploy/README.md): master, validator and dedicated KVM host.

Historical evidence is not proof that this Python rewrite has been deployed.
