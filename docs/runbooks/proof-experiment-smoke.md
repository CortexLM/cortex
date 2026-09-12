# Runbook — Proof single-task experiment smoke (any topic, one task, minutes)

Development verification of the in-guest RLM evaluate path **without waiting
for a full pack**. The smoke is a **topic shape, not a code path**: the
signed topic document, with `constraints.params.tasks` narrowed to one
item (or `n_tasks = "1"`), drives the same run request, the same guest
agent (`proof-vm-guest-agent`), and the same operator adaptor a paid
evaluate would. It works for **any custom-family topic** whose params select
an in-guest runner — `tbench` today, the next topic tomorrow — because
nothing about a task set, a slice, a timeout, or a rule id is compiled
anywhere: [`docs/PROOF.md`](../PROOF.md) § Generic run policy,
[`deploy/guest/runners/README.md`](../../deploy/guest/runners/README.md).

What the smoke is **not**: it persists no row, mints no spend token, ticks
no checklist, seals nothing, and is not a tip. It prints the report the
guest returned. A `primary_value` printed here is evidence that the path
runs end to end on that task, not a score anywhere.

Companion runbooks: [`proof-experiment-vms.md`](proof-experiment-vms.md)
(the production path this exercises), [`proof-vm-orchestrator.md`](proof-vm-orchestrator.md)
(KVM host). Code: `deploy/scripts/proof-experiment-smoke.py` (driver),
`deploy/scripts/proof-metal-smoke.sh` (SSH hop),
`bins/proof-vm-guest-agent/tests/stdio_smoke.rs` (CI half),
`crates/proof-experiment/src/policy.rs` (`RunPolicy`).

## Three drivers, one job

| `--driver` | What runs | Needs | Proves |
|------------|-----------|-------|--------|
| `agent` (default) | The **real** `proof-vm-guest-agent --stdio`, fed the same frames the KVM host sends (`hello` → `stage_secrets` → `stage_pack` → `stage_artifact` → `run`), which resolves the adaptor, stages pack + artefact, execs `run` under the guest env contract, reads `report.json` | the agent binary, the adaptor tree, the pack tar, the harness tooling (Harbor + Docker) on this host | the guest path minus Firecracker / vsock; authoritative for the env contract |
| `exec` | The adaptor's `run` directly, with the guest env contract re-stated in Python (`adaptor_env`, kept line-for-line against `crates/proof-vm-guest/src/runner.rs::job_env`) | the adaptor tree, an unpacked pack, Harbor + Docker | the adaptor on real tooling; no Rust binary needed |
| `orch` | `POST /v1/vms` (experiment spec) → `POST /v1/vms/{id}/jobs` → `DELETE` on a KVM-host `proof-vm-orchestrator` — a **real Firecracker experiment VM** booted from the pinned image | the agent's URL, bearer **file**, CA, `PROOF_RLM_VM_IMAGE_DIGEST`; the pack on the host | the whole host path with **whatever adaptor the pinned image carries** (after a re-bake, the new one) |

Every driver takes the same inputs and prints the same summary. `--dry-run`
prints the derived job request (miner env redacted) and, for `exec`, the
full `PROOF_*` environment, then exits 0 — a structural proof for **any**
topic document with no execution:

```bash
python3 deploy/scripts/proof-experiment-smoke.py \
    --topic-id tbench --cp https://gateway.cortex.foundation/challenge/proof \
    --job evaluate --tasks <one-task> --artifact-tar recipe.tar --driver exec --pack-dir . --dry-run
```

## Inputs

| Flag | Meaning |
|------|---------|
| `--topic-json FILE` / `--topic-id ID --cp URL` | The signed topic document (read-only GET from the public gateway is fine) |
| `--job evaluate\|baseline` | Which paid job shape |
| `--tasks NAME` / `--n-tasks 1` | The smoke selection, folded into `constraints.params` exactly as a re-signed topic would carry it. A name the pack does not hold fails closed inside the adaptor |
| `--set key=value` | Any other `constraints.params` override — `agent_exception_policy=zero`, `exec_timeout_s=900`, `task_exclude=…`, `inspect_marker_rules=…` |
| `--pack-tar FILE` | The pack tar (uncompressed). Its sha256 becomes `experiment_pack_digest` on the job; a mismatch with the topic's pin is **warned** and carried (dev may smoke a pack it is about to re-pin) |
| `--artifact-tar FILE` | Miner artefact (≤ 5 MiB, uncompressed). Staged as `proof-artefact://<sha256>`, injected like the gateway vault path |
| `--byok-env NAME` | Read the miner BYOK value from **this process's environment**. Defaults to the topic's `miner_byok` on evaluate; missing → refused before anything spawns. Never argv, redacted from every dump |
| `--owner-key-file NAME=PATH` | Baseline: owner key staged into the guest secrets dir |
| `--guest-agent PATH` | `agent` driver: the `proof-vm-guest-agent` binary |
| `--runner-dir DIR` | The adaptor tree (default: this checkout's `deploy/guest/runners/rlm_fc_in_guest_harbor`) |
| `--path-prepend DIR[:DIR]` | Dev only: dirs the adaptor's `PATH` starts with (a Harbor venv, a fake `docker`). The guest contract itself never inherits `PATH`, so the `agent` driver installs a shim copy of the adaptor that prepends it |
| `--orch-url`, `--orch-token-file`, `--orch-ca`, `--image-digest` | `orch` driver |
| `--work-root DIR` / `--keep-work` | Keep the guest work tree (`harbor.run.log`, `tasks-filtered/.proof-task-filter.json`, `harbor-jobs/`) for RCA |
| `--out FILE` | Full redacted outcome JSON |

Exit codes: `0` the guest answered `Done` (report printed); `1` `Failed` /
refused by the guest or adaptor; `2` a precondition the script checks
itself.

## Expected output (one task)

```text
[smoke] topic=tbench runner=rlm_fc_in_guest_harbor job=evaluate tasks=<task> n_tasks=- deadline_s=7200 byok=['OPENROUTER_API_KEY']
[smoke] spawn …/proof-vm-guest-agent --stdio --runners-dir … --pack-root … --work-root …
[smoke] hello → ready
[smoke] stage_pack → pack_staged
[smoke] stage_artifact → artifact_staged
[smoke] run → done
[smoke] Done — the guest returned a report (this smoke persists nothing):
{
  "primary_value": 1.0,
  "sandboxed": true,
  "n_scored": 1, "n_measured": 1, "n_agent_exceptions": 0,
  "agent_exception_policy": "fail",
  "trials": [{"name": "<task>__1", "outcome": "measured", "reward": 1.0}],
  "runner": "rlm_fc_in_guest_harbor", "harness_kind": "python", "wall_s": …
}
```

The same report shape is what `run_paid_job` binds and the control plane
turns into a row on a real submission — `primary_value` becomes
`custom_value`, `evidence.trials` travels in the artefact zip. Read the
failure text when it is not `run → done`: the guest's redacted adaptor tail
is in the `Failed` line (`harbor is not on PATH`, `no measured Harbor
trials`, `incomplete vs filtered task set`, `constraints.params.… re-sign
the topic`), and `--work-root` keeps `harbor.run.log`.

## Cursor cloud ↔ metal

A Cursor cloud VM reaches `185.8.107.43:22` but holds no key, so it can run
the **CI half** (below) and every `--dry-run`, and it can prepare the exact
Owner command. `deploy/scripts/proof-metal-smoke.sh` is that hop:

```bash
# Owner / Architecte, from a box with `ssh cortex-metal`, on this branch.
# Evaluate: the topic's miner BYOK must be exported here (never on argv;
# it travels as a 0600 file the remote shell sources and deletes).
export OPENROUTER_API_KEY=…

# authoritative guest path: a static guest agent built from this branch
cargo build --release -p proof-vm-guest-agent-bin --target x86_64-unknown-linux-musl

./deploy/scripts/proof-metal-smoke.sh --host cortex-metal \
    --topic tbench --task <one-task> --job evaluate --artifact recipe.tar \
    --guest-agent-local target/x86_64-unknown-linux-musl/release/proof-vm-guest-agent \
    --path-prepend /opt/harbor/venv/bin \
    --set agent_exception_policy=zero --set exec_timeout_s=900 \
    --out-dir ./smoke-out --keep
```

What it does on the host, read-only except its own temp dir: `ssh
BatchMode` preflight (no key → prints the command above and exits 2),
GETs the topic document, resolves the pinned pack at
`/var/lib/proof-vm/packs/sha256-<hex>.tar` (fails closed if absent), ships
**this checkout's** adaptor tree + the driver (+ artefact, + agent), and
runs the driver with `--tasks <task>`. It does **not** touch
`/var/lib/proof-vm/retained/`, the running orchestrator, the live
evaluate slot, digests, seals, or weights. `--path-prepend` names the
Harbor venv's `bin` on the host (the guest contract gives the adaptor a
fixed `PATH`); without a guest agent binary the driver falls back to
`--driver exec`.

Expected: the block above with `trials[0].name == "<task>__1"`,
`smoke-out/outcome.json` and `smoke-out/smoke.log` locally. Attach both to
the PR / issue as the metal evidence.

### Why this run would have caught `tbench-x0032`

The retained run had 10 tasks kept, 7 measured at `0.0`, and 3 trials whose
miner `agent.py` raised `RuntimeError: Command timed out after 120 seconds`
from `environment.exec(...)` — an exception during the harness phase, so
Harbor never ran the verifier. The old adaptor treated the incomplete set
as fail-closed only: no `report.json`, guest `Failed`, gateway **503**, no
row, frontend "0 runs". Three things in this branch change that picture,
each a **topic knob**:

| Cause | Now |
|-------|-----|
| A silent `timeout_sec=120` in the miner's harness | `params.exec_timeout_s` sets the default for `exec()` calls that pass none (`PROOF_EXEC_TIMEOUT_S`, applied by `ProofPythonAgent`); a value the miner passes explicitly stays theirs |
| A harness crash turns the whole evaluate into a 503 | `params.agent_exception_policy = "zero"` scores that trial 0 (the task was not solved) with the exception in evidence; a **persisted** row instead of nothing. Default stays `fail` |
| Nobody could try one task without an hour-long full run | `--tasks <task>` (or `n_tasks=1`) on this smoke — one task, minutes |

Run the smoke on one of the three failing tasks with
`--set agent_exception_policy=zero --set exec_timeout_s=900` and read
`n_agent_exceptions` / `agent_exception_trials` in the outcome.

## The CI half (no Firecracker, no Docker, no key)

`cargo test -p proof-vm-guest-agent-bin` runs the driver against the built
guest agent with a fake adaptor (the guest contract: pack + artefact staged,
`PROOF_PARAM_TASKS` / `AGENT_EXCEPTION_POLICY` / `EXEC_TIMEOUT_S` reach the
adaptor, the BYOK file is staged and the value never printed) and with the
in-repo Harbor reference adaptor over a fake `harbor` + `docker`
(`filter_tasks` keeps one of two pack tasks, Harbor runs on the filtered
copy with the full model id, summarize scores one trial). `cargo test -p
proof-vm-guest` runs the adaptor's own Python / shell suite. `cargo test -p
proof-experiment -p proof-rlm` covers `RunPolicy` and the refuse-before-VM
gate.

## Migrating the live `tbench` document (Owner, re-sign)

The reference adaptor no longer compiles the first-15 / INFRA lists or the
`tbench` rule ids. After the guest image is re-baked from this branch, the
live document must **say** what it relied on. Params to add
(`constraints.params`, all strings; 26 today + 5 = 31 ≤ 32 — drop the
informational `benchmark` / `task_count` / `task_scope` if room is needed):

| Param | Value | Why |
|-------|-------|-----|
| `task_exclude` | `batched-eval-parity,ctr-optimization,cumulative-layout-shift,distributed-dedup,coq-block-bound` | the INFRA excludes the old `first15` mode applied (or put them in the pack's `filter.json` → `deny` and re-pin) |
| `inspect_marker_rules` | `no_eval_short_circuit:skip_eval\|skip_verifier\|always_pass_eval\|short_circuit_eval;no_tb4_hardcoding:tb4_answers\|hardcoded_tb4` | the cheat markers the old inspect compiled |
| `inspect_attested_rules` | `same_seed,miner_byok_openrouter,firecracker_sister,artefacts_zip,auto_promote_best,rlm_topic_setup_autonomous` | the host/topic-enforced rules the old inspect passed by name |
| `agent_exception_policy` | `zero` (operator decision; default `fail` keeps today's 503) | a harness crash is a failed task, not a missing run |
| `exec_timeout_s` | `900` (operator decision) | default per-command wall clock for harnesses that pass none |

Until the two `inspect_*` params are signed, every submission is a persisted
`rejected` row (red checklist, **no spend**) whose evidence names them —
visible and recoverable, never a silent pass. `constraints.task_slice`
(`tb4-first-15`) stays an informational label unless the pack gains a
`slices/tb4-first-15.json`. Nothing here needs a baseline reseal; the
scored set is unchanged (15 − 5 = 10 tasks).
