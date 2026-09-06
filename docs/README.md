# Cortex documentation

Cortex is building an **autonomous research network** where reproducible findings
can improve shared methods, rather than remain isolated model checkpoints.

## Start with your goal

| Audience | Read first | Next |
|----------|------------|------|
| Investors and new readers | [Project overview](OVERVIEW.md) | [Current capabilities and limits](COMPLETENESS.md) |
| Research contributors | [Getting started](external-miner/README.md) | [Proof](external-miner/proof.md) |
| Bug hunters | [Bounty](external-miner/bounty.md) | [Troubleshooting](external-miner/troubleshoot.md) |
| Validators | [Validator guide](external-miner/validators.md) | [Architecture](ARCHITECTURE.md) |
| Developers | [Contributing](../CONTRIBUTING.md) | [Architecture](ARCHITECTURE.md), [naming and compatibility](NAMING.md) |
| Operators | [Deployment](../deploy/README.md) | [Runbooks](AGENTS.md#runbook-index), [security checklist](OPERATOR_SECURITY.md) |

## Technical reference

These documents keep the exact rules needed to operate and verify the network.
You do not need to read them to understand the project.

- [Proof](PROOF.md): topic publication, evaluation, and research rewards.
- [Bounty](BOUNTY.md): pairing, bug reports, adjudication, and rewards.
- [Bundle specification](BUNDLE_SPEC.md): signed results and reward verification.
- [Site API](SITE_API.md): public website data.
- [Threat model](THREAT_MODEL.md): security guarantees and their limits.
- [Repository cleanup notes](CLEANUP.md): what was removed and what must stay.

## Research rationale

The [whitepaper](../whitepaper.pdf), *A Proposal for a New Incentive Mechanism on
Cortex*, explains why the network exists. Start with §2 for the checkpoint and
static-evaluator critique, and §7 for research reuse and the proposed synthesis
agent.

The [plain-language reading guide and implementation comparison](WHITEPAPER.md)
separates that proposal from current code. The paper is a primary source for the
vision, not a deployment-readiness report or a replacement for current scoring
contracts.

## Historical material

[Design](DESIGN_CHALLENGE.md), [Prism](PRISM.md), [Prism recipes](PRISM_RECIPE.md),
the retired miner pages, [evidence](evidence/), and [experiments](spikes/) provide
historical background. They are **not a list of open challenges**.

Frozen specifications and operational evidence remain available for verification
and old links. Use the guides above for current participation and deployment.
