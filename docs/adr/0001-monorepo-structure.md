# ADR 0001: Cortex v3 monorepo structure

- Status: accepted for branch `v3`, 2026-10-26. `main` is unchanged until Principal SN100
  decides to merge `v3`.
- Order: founder, 2026-10-26: one repository, `CortexLM/cortex`, with `CortexLM/hypertrain`
  imported and later archived. Principal SN100 decision D13: every batch ships a short ADR
  with its code.
- Labels: **[V]** verified in code, history or a run on the date above, **[A]** reported by a
  document, **[H]** hypothesis, **[?]** unknown.

## Context

- Cortex `main` 008610a1: 973 commits, 251 files, distribution `cortex-subnet` (package
  `src/cortex`), Python >=3.12, uv 0.11.17, CI on `blacksmith-4vcpu-ubuntu-2404`, image
  publication on a GitHub-hosted runner (4e47aa98, b97f3920). [V]
- Hypertrain `main` 679eb5b3: 236 commits, 478 files, no tag, 32 branches, distribution
  `hypertrain` (package `src/hypertrain`), Python ==3.12.\*, `torch==2.14.0` from the PyTorch
  CPU index, `opendecision` pinned to a Git commit, uv 0.12.17, six workflows. [V]
- Production runs `ghcr.io/cortexlm/hypertrain@sha256:f09e6bd9…df56`, built by
  `CortexLM/hypertrain/.github/workflows/images.yml` on a GitHub-hosted runner and checked
  with `gh attestation verify --signer-workflow …/images.yml --deny-self-hosted-runners`.
  [A `livrables/forge/00-deploy-runbook.md:113-125`, last §10, 2026-10-26]
- The supervisor requires the image label `org.opencontainers.image.source` to equal the
  registry row's `source`, and looks the build provenance up in that same repository
  (`src/cortex/challenges/supervisor.py:85-108`). [V]
- Three hypertrain properties must not move by a single byte [V]:
  - golden leaf roots: `hypertrain/tests/trainer/test_golden_replay.py:25`,
    `hypertrain/tests/protocol/test_od_fields.py:14`, gate `hypertrain/scripts/check-proof.sh`;
  - `implementation_hash`: nine files and the AST of the `owned` store methods, with paths
    relative to `src/hypertrain` (`hypertrain/src/hypertrain/challenge/store.py:4249-4318`);
  - protocol constants: `hypertrain/src/hypertrain/protocol/`.

  None of them reads a path above the hypertrain project root (no `git rev-parse
  --show-toplevel`; `store.py:4260` resolves `parents[1]` of its own file).

## Decision

### 1. Layout

```text
/                          Cortex, unchanged: src/cortex, tests, scripts, config, deploy, docs,
                           pyproject.toml, uv.lock
/hypertrain/               self-contained project, byte-identical to CortexLM/hypertrain 679eb5b3
                           apart from its .github/ directory (moved, see 5):
                           pyproject.toml, uv.lock, src/hypertrain, tests, scripts, docs, docker,
                           deploy, experiments, README.md, CHANGELOG.md, LICENSE, NOTICE
/.github/workflows/        every workflow; GitHub reads no other directory
/docs/adr/                 monorepo ADRs, NNNN-title.md
/docs/rfcs/0001-cortex-v3/ RFC 0001 lots (arrive from rfc/cortex-v3)
/docs/provenance/          import records: hypertrain-import.md
```

Import paths and distribution names do not change: `import cortex` and `import hypertrain`.
Both projects are Apache-2.0; `hypertrain/` keeps its own `LICENSE` and `NOTICE`, which
Apache-2.0 §4(d) requires to travel with the code.

### 2. Lockfiles and tooling

Two independent uv projects with two lockfiles, and no root uv workspace. A job runs uv in
its project directory (`defaults.run.working-directory: hypertrain`,
`cache-dependency-glob: hypertrain/uv.lock`). Each project keeps its uv pin. A hypertrain
command is the one in `hypertrain/README.md`, run from `hypertrain/`. Root-level rules that
apply to the whole tree stay at the root: `.gitignore` (which ruff honours inside
`hypertrain/`) and the secret scan of `scripts/check_repo.py` (63f08dcd).

### 3. docs/ and scripts/

Monorepo-wide records go in the root `docs/` (ADRs, RFCs, provenance, architecture).
Hypertrain's operator, protocol and miner pages stay in `hypertrain/docs/`. Its scripts
resolve them from the project root, and `hypertrain/tests/test_publish_exclusions.py` scans
them. `scripts/` stays Cortex's and `hypertrain/scripts/` stays hypertrain's: CI calls
`scripts/check-proof.sh` and the tests call `scripts/*.py` relative to `hypertrain/`.

### 4. Provenance

Hypertrain `main` is rewritten in a throwaway clone with `git filter-repo
--to-subdirectory-filter hypertrain`. Its issue and PR references are rewritten to
`CortexLM/hypertrain#N`. The result is merged with `--allow-unrelated-histories`, Cortex
first parent (e61461c3). Authors, dates and per-commit trees are kept. The full
old-to-new commit map, the tree-identity check and the `git log --follow` proofs are in
[the provenance record](../provenance/hypertrain-import.md). No tag is imported (hypertrain
has none) and no other branch is imported. Open hypertrain work moves later with the same
filter-repo command or `git am --directory=hypertrain`. Issues #25, #28, #29, #32 and #34
stay in `CortexLM/hypertrain` until archival.

### 5. Workflows

Cortex workflows keep their file names: the production verification names
`CortexLM/cortex/.github/workflows/images.yml` as the signer. Imported workflows get a
`hypertrain-` prefix, and each one's `name:` is its file stem.

| Hypertrain file | Monorepo file | Change |
| --- | --- | --- |
| `ci.yml` | `hypertrain-ci.yml` | working directory, path-filtered PR and `v3-*` push triggers, cache glob; same jobs, `blacksmith-8vcpu-ubuntu-2404`, timeouts, deselects and shard order (138 files, same order [V]); `check-proof` stays `continue-on-error` (CortexLM/hypertrain#32) |
| `heavy.yml` | `hypertrain-heavy.yml` | working directory, cache glob; `blacksmith-16vcpu-ubuntu-2404` kept; schedule and dispatch only fire from the default branch, so it is inert on `v3` |
| `miner-flow.yml` | `hypertrain-miner-flow.yml` | runs on pushes to `main`, `v3` and `v3-*` and on PRs, with path filters (the `boat/**` branches do not exist here); its pre-checkout step runs in `/` |
| `images.yml` | `hypertrain-images.yml` | v3 packages and tags (see 6); the `candidate-gpu` CUDA lane is dropped |
| `release.yml` | not ported | `v*.*.*` tags belong to Cortex releases (`images.yml`), it needs the secret `HYPERTRAIN_RELEASE_KEY` that Cortex does not hold, and this batch publishes no release |
| `gpu-vast.yml` | not ported | Vast is abandoned; it needs `VAST_API_KEY` |

Cortex `ci.yml` now also runs on pushes and PRs to `v3` and on pushes to batch branches
`v3-*` (`v3-common-impl.md`: a batch pushes `v3-<batch>`, then opens a draft PR into `v3`).
Its jobs are unchanged. `hypertrain-ci` and `hypertrain-miner-flow` also run on pushes to
`v3-*`. The `hypertrain-ci` jobs skip a PR from a `v3-*` branch of this repository: the push
run of the same head commit already reports them on the PR, and running twice would double
the Blacksmith cost. The hypertrain suite, its images and the miner flow run only when
`hypertrain/**`, the root `.gitignore` or their own workflow file changes. A docs-only
change at the root therefore runs only Cortex `ci` (three 4-vCPU jobs). Hypertrain Markdown is not excluded, because the
publish scanner test reads it. Publish jobs run on GitHub-hosted `ubuntu-24.04`. Action
references stay pinned by commit (`tests/test_supply_chain.py`).

### 6. Images built from the monorepo

| | Production (unchanged) | v3 (this ADR) |
| --- | --- | --- |
| packages | `ghcr.io/cortexlm/hypertrain`, `ghcr.io/cortexlm/hypertrain-gpu` | `ghcr.io/cortexlm/cortex/hypertrain`, `ghcr.io/cortexlm/cortex/hypertrain-gpu` |
| tags | `sha-<commit>`, `edge`, `vX.Y.Z`, `stable` | `v3-sha-<commit>` only, no alias |
| source label | `https://github.com/CortexLM/hypertrain` | `https://github.com/CortexLM/cortex` (set at build time; the Dockerfiles are unchanged) |
| signer | `CortexLM/hypertrain/.github/workflows/images.yml@refs/heads/main` | `CortexLM/cortex/.github/workflows/hypertrain-images.yml@refs/heads/v3` |

`hypertrain-images.yml` runs `hypertrain-ci` and then, on a GitHub-hosted runner, builds
and pushes each image once and attests the pushed digest. In the same job it verifies that
digest (`gh attestation verify --repo CortexLM/cortex --source-ref refs/heads/v3
--signer-workflow …/hypertrain-images.yml --deny-self-hosted-runners`), checks the labels,
runs `hypertrain/scripts/verify_image.sh` on the published server digest, and writes the
digest to the job summary. A guard step refuses any package outside
`ghcr.io/cortexlm/cortex/` and any tag other than `v3-sha-<40 hex>`.

Separate packages were chosen over `v3-` tags in the production packages for three reasons:

- One package name would carry two provenance identities, so a row that follows a channel
  could change identity silently.
- The production packages are linked to `CortexLM/hypertrain`, and writing to them from
  `CortexLM/cortex` would need a package-permission change. [H, GHCR default, not tried]
- With no `stable`/`edge` alias, a v3 image can only be registered with an explicit
  `pin = "sha256:…"`, which fails closed (`src/cortex/challenges/registry.py:17,82`). [V]

## Old-repository references to migrate (unchanged in this batch)

| Where | Reference | Migration |
| --- | --- | --- |
| s100 `deploy/challenges/registry.toml`, hypertrain row | `image = "ghcr.io/cortexlm/hypertrain"`, `source = "https://github.com/CortexLM/hypertrain"`, pin `f09e6bd9…` [A runbook:113-125] | at the pin switch: monorepo package, `source = "https://github.com/CortexLM/cortex"`, a monorepo digest; rollback = previous row |
| `livrables/forge/00-deploy-runbook.md:73,101,114` | `gh attestation verify --signer-workflow …/images.yml` (hypertrain identity) | `--repo CortexLM/cortex --signer-workflow CortexLM/cortex/.github/workflows/hypertrain-images.yml` and the new source ref |
| `livrables/forge/00-deploy-runbook.md:39-40,61` | procedure: publish `ghcr.io/cortexlm/hypertrain`, add `hypertrain/deploy/registry-row.toml` | rewrite with the pin switch |
| `hypertrain/docker/Dockerfile.server:30`, `hypertrain/docker/Dockerfile.gpu:18` | `LABEL org.opencontainers.image.source=…/CortexLM/hypertrain` | change the Dockerfiles when production moves; until then the workflow overrides the label |
| `hypertrain/deploy/registry-row.toml:12,14` | production package and source | same as the s100 row |
| `hypertrain/scripts/verify_image.sh:86` | default source | default to the monorepo |
| `hypertrain/docs/operator.md:63,71`, `hypertrain/docs/miner.md:26`, `hypertrain/README.md:14` | package names, label source, CI badge | rewrite with the switch |
| `hypertrain/deploy/k8s/base/deployment.yaml:39`, `kustomization.yaml:13`, `local/fixtures.yaml:80,133,211,262`, `versions.json:2,4` | production image and its build-job URL | re-pin to a monorepo digest |
| `hypertrain/scripts/ops/auditors.sh:6` | default auditor image | re-pin |
| `hypertrain/scripts/genuine_cuda_ci.py:35`, `hypertrain/scripts/vast_e2e.py:33` | GPU package for Vast runs | retire with Vast |
| `hypertrain/scripts/publish.sh:21,113` | publishes a snapshot to `CortexLM/hypertrain`; scanner exemption for the old build-job URL of `versions.json:4` | retire at archival, or change with `versions.json` |
| `hypertrain/src/hypertrain/miner/update.py:32`, old `release.yml`, `hypertrain/scripts/publish-allowlist.txt:217-218` | miner auto-update trusts manifests signed by the `HYPERTRAIN_RELEASE_KEY` secret of `CortexLM/hypertrain` and wheels on its releases; the allowlist entry still names the removed `release.yml` | new release path (secret in Cortex, `hypertrain-v*` tags): founder decision |
| `hypertrain/teacher.example.toml:5`, `hypertrain/tests/relay/test_transport_v2.py:2543`, `hypertrain/tests/test_publish_exclusions.py:102` | example header and fixtures | change with their subject |
| `README.md`, `docs/ecosystem.md` | links to the hypertrain repository | pointed at `hypertrain/` in this batch |

## Consequences

- No import-path churn. Golden roots, `implementation_hash` inputs and protocol constants
  are byte-identical: each of the 236 imported commits has `hypertrain/` equal to the tree
  of its original. [V, provenance record]
- CI cost: a root docs-only change does not start the twelve 8-vCPU hypertrain jobs.
  Blacksmith charges 0.004 USD per minute for 2 vCPU and scales with vCPU: 8 vCPU is 4×,
  16 vCPU is 8×. [A pricing pages read 2026-10-26] October use was about 2,751 min on
  8 vCPU (~44 USD) plus 1,584 min on 16 vCPU (~51 USD), against the 500 USD/month cap.
  [A brief]
- The two lockfiles can drift: `fastapi` is `>=0.135,<0.136` in Cortex and `>=0.133` in
  hypertrain. Code shared between the two needs a resolution decision first.
- `hypertrain-heavy` does not run on `v3`: its triggers are only `schedule` and
  `workflow_dispatch` [V], and GitHub fires those only from the default branch [A GitHub
  docs]. The native systemd tests and the od-base suite are therefore unproven on `v3`
  until it reaches `main` or someone runs it by hand.
- A path-filtered workflow that is skipped leaves its check "Pending". Before `v3` or `main`
  requires `hypertrain-ci`, add an always-running aggregate job. [A GitHub docs]
- Images built from `v3` carry the new identity. No production pin, registry row or
  supervisor changes.
- B300 is `sm_103` [A Scout 2026-10-29]. The GPU image recipe is unchanged, and whether
  its torch 2.14 cu130 wheels ship `sm_103` kernels is unknown [?]. ADR 0004 decides.

## Deferred

- Archiving `CortexLM/hypertrain`: a founder action. The archive must keep `images.yml`,
  the packages `ghcr.io/cortexlm/hypertrain{,-gpu}` (production pins them by digest; never
  delete) and their attestations, and it gets a README redirect to `CortexLM/cortex/hypertrain`.
- Switching the production pin to a monorepo digest (Principal SN100, founder): the
  registry row and the runbook command in the table above.
- The hypertrain release path for signed miner updates, the CUDA candidate lane (ADR 0004,
  B300) and running `hypertrain-heavy` before the merge.
- A single uv workspace, aligned uv versions, and the merge of `v3` into `main`.

## Rejected alternatives

1. **Fold hypertrain into Cortex's package** (one `pyproject.toml`, `src/hypertrain` next to
   `src/cortex`): this edits the imports and paths of hundreds of files. It also moves the
   `implementation_hash` inputs and the golden-root dependency set (one resolution of
   torch, numpy and fastapi for both projects), so every golden root would need
   requalification. And every change would run both suites.
2. **One root uv workspace with a single `uv.lock`:** the move itself would re-resolve
   hypertrain's environment, which needs bitwise requalification, and Cortex's consensus
   dependencies would move with hypertrain's. Deferred, not refused.
3. **`git subtree add --prefix=hypertrain`:** old commits keep files at their original root
   paths, so `git log --follow` and blame break at the boundary, and there is no 1:1
   commit map. With `--squash` the history is lost.
4. **Git submodule:** this is not a monorepo. Two CI systems and two provenance identities
   would remain.
5. **`v3-` tags inside the production packages:** see section 6.

## Sources

- Founder order and decisions: `livrables/principal-sn100/2026-10-26_cortex-v3-programme.md`
  §5-6 (read 2026-10-26). Batch brief: `livrables/forge/briefs/cortex-v3/v3-monorepo.md`.
- Production pins and verification commands: `livrables/forge/00-deploy-runbook.md:38-125`
  (§4, §7 and §9-10; read 2026-10-26).
- Scout benchmark: `livrables/scout/2026-10-29_benchmark-entrainement-decentralise.md`
  (CUDA facts, licences; read 2026-10-26).
- GitHub path filters, new-branch diffs, the 3,000-file limit and pending skipped checks:
  <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax>.
  Schedule and dispatch only from the default branch:
  <https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows>.
  Both read 2026-10-26.
- Blacksmith prices: <https://www.blacksmith.sh/pricing> and
  <https://docs.blacksmith.sh/blacksmith-runners/overview> (read 2026-10-26).
