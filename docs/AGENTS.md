# Documentation contract

Documentation describes the current Python implementation. Historical evidence
is not operational proof, and frozen specifications are compatibility artifacts,
not live product instructions.

## Canonical pages

| Subject | Page |
| --- | --- |
| system topology and source map | `ARCHITECTURE.md` |
| Proof control plane and RLM | `PROOF.md` |
| Bounty intake and score | `BOUNTY.md` |
| compatibility names and domains | `NAMING.md` |
| trust assumptions and residual risk | `THREAT_MODEL.md` |
| operator release checklist | `OPERATOR_SECURITY.md` |
| environment and service settings | `reference/configuration.md` |
| miner workflows | `external-miner/` |

Every reader-facing page must be linked from `index.md` directly or through one
clearly indexed section. Keep one canonical page per topic and link to it instead
of copying the same contract into a second runbook.

## Frozen and historical material

`BUNDLE_SPEC.md`, `DESIGN_CHALLENGE.md` and `PRISM.md` are byte-pinned by
`scripts/check_repo.py`. Do not edit them. Design, Prism and Relearn are not live
products. The Relearn files under `external-miner/` and `proof-tbench.md` remain
short historical pointers so old URLs do not disappear.

## API changes

When Bounty or Proof changes a public route, payload, authentication rule, quota,
timeout, scoring rule or failure response, update its miner guide in the same
change. Examples must run against the Python CLI or current HTTP surface.

Use exact capability language. Offline fake-provider tests prove deterministic
control-plane behavior; they do not prove a live KVM boot, scientific validity,
provider execution or on-chain payment. Never call an empty or guessed digest a
pin.

Do not add phase reports, audit logs, generated transcripts, evidence dumps or
duplicate README files. Release history belongs in `CHANGELOG.md`; temporary
validation output belongs in CI artifacts or private operator storage.
