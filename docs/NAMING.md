# Naming and compatibility

The product and Python package are named **Cortex**. Several deployed names
begin with `BASE_` or `base-`; those strings are protocol compatibility, not a
second product.

## Stable names

Do not rename these without a versioned migration:

- environment variables beginning with `BASE_`;
- signature domains beginning with `base-`;
- SCALE fields and JSON fields defined by the frozen bundle specification;
- persisted database columns and deployed secret paths consumed by existing
  installations.

New master configuration may also use the corresponding `CORTEX_*` alias.
`MasterConfig` rejects conflicting `BASE_*` and `CORTEX_*` values instead of
choosing one. Documentation and deployment examples use `BASE_*` so operators
see one canonical spelling.

The preserved signing domains are:

| Domain | Purpose |
| --- | --- |
| `base-trustroot-v1` | owner-signed challenge and measurement documents |
| `base-rawweight-v1` | challenge leaves |
| `base-bundle-v1` | sealed epoch bundles |
| `base-root-v1` | validator root statements |
| `base-dissent-v1` | validator dissent evidence |
| `base-proof-topic-v1` | published Proof topics |
| `base-proof-submit-v1` | miner Proof submissions |
| `base-bounty-report-v1` | Bounty report fingerprints |
| `base-bounty-session-v1` | Bounty session tokens |

Inference and executor offer domains use the newer `cortex-*` prefix because
they were introduced by the Python implementation and have no deployed legacy
preimage.

## Live products

Only two challenge IDs are live:

| ID | Emission share |
| --- | ---: |
| `bounty` | 2,000 bps |
| `proof` | 8,000 bps |

The signed trust root must contain exactly those rows and total 10,000 basis
points. Design, Prism and Relearn names may appear in frozen or historical
documentation only. They are not services, routes, trust-root rows or emission
recipients.

## File and command names

Python code lives under `src/cortex`. The single executable is `cortex`, with
subcommands for master, validator, miner, VM host and offline operator work.
Deployment state defaults to `/var/lib/cortex`; compatibility paths already in
an installation may stay under `/etc/base` or `/opt/base` until an explicit
operator migration is shipped.

Never reinterpret an existing environment variable or signing domain merely to
make it look newer. Add an alias or a new versioned contract, document the
transition, and keep both sides testable during the migration window.
