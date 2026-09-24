# Security policy

## Reporting a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/CortexLM/cortex/security/advisories/new).
Do not open a public issue or include credentials, private holdouts, miner BYOK,
exploit payloads, or wallet material in logs.

Include the affected component, commit SHA or release tag, impact, and a minimal
reproduction. State whether the issue is already public.

## Scope

The gateway, validator, Proof service, challenge supervisor and proxy, RLM, Firecracker host, miner CLI,
deployment definitions, signature formats, and sealed-weight path are in scope.
Third-party model/GPU providers and miner artifacts remain untrusted external
boundaries, but failures in Cortex's validation of them are in scope.

Compatibility names such as `BASE_*` and `base-*-v1` are deliberate protocol and
deployment pins; see [docs/NAMING.md](docs/NAMING.md).

## Supported versions

Security fixes land on `main` and ship from annotated `v*.*.*` tags. Production
deployments use exact image digests.
