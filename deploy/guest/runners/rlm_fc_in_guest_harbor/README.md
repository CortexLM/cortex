# Runner `rlm_fc_in_guest_harbor`

Versioned **reference adaptor** for in-guest Harbor evaluate. It is operator
bake content, not compiled into any Proof binary. A signed topic selects it
only by putting this exact id in `constraints.params.baseline_runner`
(alias `in_guest_benchmark_runner`). Any other id still fails closed until
that id is baked.

**Everything the adaptor decides is topic data.** Which tasks are scored,
how many, under which wall clocks, what a crashed harness counts as, and
how each anti-cheat rule is ticked all come from the signed
`constraints.params` (exported by the guest as `PROOF_PARAM_*`),
`constraints.task_slice` / `model_pin`, and the topic-pinned pack
(`experiment_pack_digest`). Nothing in this directory names a benchmark, a
task, a slice, a rule id, or a timeout number. The same adaptor therefore
serves **any** custom-family topic whose pack is a directory of Harbor
tasks: point a topic's params at it and the run is defined by that topic.
The generic knobs are shape-checked by `proof-experiment::RunPolicy` on the
control plane (before any experiment VM) and in the guest (before this
adaptor runs), so a typo in a signed value is a 503 with the knob named,
never a run under some other meaning.

## Miner harness interface

Evaluate discovers, in order:

1. `$PROOF_ARTIFACT_DIR/harness.json` (also `recipe/harness.json`,
   `agent/harness.json`, `recipe/agent/harness.json`)
2. A Python agent directory at `$PROOF_ARTIFACT_DIR/agent/` then
   `$PROOF_ARTIFACT_DIR/recipe/agent/`
3. A script at `run.sh` / `recipe/run.sh` / `harness.sh`

`harness.json`:

```json
{ "kind": "python", "import_path": "agent.agent:YourClass" }
{ "kind": "harbor", "import_path": "agent.agent:YourClass" }
{ "kind": "script", "entry": "recipe/run.sh" }
{ "kind": "builtin", "name": "oracle" }
```

| `kind` | What runs |
|--------|-----------|
| `python` (primary) | Custom Python class (`Agent` / `ProofAgent` / any class named in `import_path`). Need not subclass Harbor `BaseAgent`. Harbor `-a` is the in-tree wrapper `proof_python_agent:ProofPythonAgent`, which imports **your** class from the artefact only and hands it the environment (wrapped for the topic's `exec_timeout_s`, § Timeouts). |
| `harbor` | Harbor `BaseAgent` / `BaseInstalledAgent` subclass, passed as `-a module:Class` |
| `script` | Miner executable relative to the artefact. Docker, the **selected** tasks, and BYOK are already set. The script sees `$PROOF_TASKS` (and `$PROOF_PACK_DIR/<tasks_dir>` rebound to a **tasks-only** materialized copy — original-pack siblings are not copied). `harbor run --path` is rewritten onto the selected tree. Summarize **drops trial names** that are not directories under `$PROOF_TASKS` and **fails closed** unless every selected task has ≥1 scored trial. A trial is measured only with matching Harbor `verifier_result.rewards.reward` **and** `verifier/reward.txt`. A miner-authored `$PROOF_OUTPUT_DIR/report.json` is deleted and ignored. Never wrapped as a built-in agent. |
| `builtin` | A Harbor built-in the **miner** opted into. Evaluate refuses a topic built-in when this file is absent. |

Custom Python `run()` may take `instruction` alone or Harbor's
`(instruction, environment, context)`. Discovery is AST-only; miner code is
not executed until the paid run.

## What Harbor `-a` actually accepts

Verified against Harbor CLI (`-a` / `--agent`) and `AgentFactory`:

| Value | Meaning |
|-------|---------|
| Built-in name | `terminus-2`, `oracle`, `claude-code`, … (baseline / explicit `harness.json` only) |
| Python import path | `module.path:ClassName` (subclass of `BaseAgent` / `BaseInstalledAgent`) |
| ACP shorthand | `acp:opencode@…` |

Harbor **does not** accept a filesystem path for `-a`. This adaptor therefore
**does not** pass `-a $PROOF_ARTIFACT_DIR/agent`. On evaluate it:

1. Detects a Python / Harbor agent directory in the unpacked artefact (AST
   scan, no miner code executed at resolve time).
2. Puts the parent of that directory on `PYTHONPATH` (plus this adaptor's
   `harness/` dir when the custom-Python wrapper is used).
3. Passes `-a module.path:ClassName` or `-a proof_python_agent:ProofPythonAgent`.

A one-line `import_path` file inside the agent dir (contents
`module.path:ClassName`) wins when several classes exist. A built-in name in
that file is refused: that would ignore miner code. The named module is
resolved in the evaluate import env (artefact parent only) and **rejected**
if its origin is outside the staged artefact.

## Agent selection (miner attach surface)

| Job | Artefact | What runs |
|-----|----------|-----------|
| `evaluate` | custom Python at `agent/` / `recipe/agent/` | wrapper `-a proof_python_agent:ProofPythonAgent` |
| `evaluate` | Harbor `BaseAgent` dir | that import path |
| `evaluate` | `harness.json` | the named kind |
| `evaluate` | `$PROOF_ARTIFACT_DIR/recipe/run.sh` only | **script harness** — not wrapped as a built-in |
| `evaluate` | artefact staged, nothing matching | **fail closed** — never the topic agent |
| `baseline` | `PROOF_ARTIFACT_DIR` unset | topic `PROOF_PARAM_HARBOR_AGENT` (operator pack agent) |
| `baseline` | artefact has a miner harness | that harness (same resolution) |
| `baseline` | artefact staged but no harness | **fail closed** — topic fallback is only when no miner artefact is staged |

## Miner artefact (uncompressed tar)

Identity is the served file, verbatim (`tar -cf`, then `sha256sum` that
file). After unpack, paths are relative to `$PROOF_ARTIFACT_DIR`.

**Preferred** (matches `tar -cf recipe.tar recipe/`):

```
recipe/
  harness.json      # optional: explicit kind (python | harbor | script | builtin)
  agent/            # PREFERRED: custom Python (class Agent) or Harbor BaseAgent
    agent.py
    import_path     # optional: one line `agent.agent:ClassName`
  run.sh            # optional script harness; evaluate execs it, does not wrap a built-in
  README.md
```

**Tar root** if you pack with `tar -cf recipe.tar -C recipe .`:

```
agent/              # same agent dir, at the unpack root
harness.json
recipe/run.sh       # optional
README.md
```

Resolution order after unpack: `$PROOF_ARTIFACT_DIR/agent` then
`$PROOF_ARTIFACT_DIR/recipe/agent`. Be explicit which layout you hash and
serve; re-tarring changes the digest.

## Task selection (`harness/filter_tasks.py`)

The scored set is a pure function of the signed topic and its pinned pack.
Selection, in order, then filters:

| Step | Source | Behaviour |
|------|--------|-----------|
| 1 | `params.tasks` (`PROOF_PARAM_TASKS`) | Exact ordered names (comma / space separated). A name the pack does not hold **fails closed** — a topic that names a task is never scored on a smaller set. **One name is the single-task smoke.** |
| 2 | `constraints.task_slice` (`PROOF_TASK_SLICE`) | Resolved **through the pack**: `slices/<label>.json` (`["task-a", …]`), `slices/<label>.txt` (one per line), or `filter.json` → `slices.<label>`. A label the pack does not define fails closed when the pack defines any slice; a pack with no slices treats the label as informational (recorded in the summary). |
| 3 | pack `filter.json` → `allow` | The pack's own default set. |
| 4 | every task directory under `params.tasks_dir` | Sorted. |
| then | `params.task_exclude` (`PROOF_PARAM_TASK_EXCLUDE`) + pack `filter.json` → `deny` | Removed. **Exact names**, no alias / prefix matching. |
| then | `params.max_task_duration_s` (pack `max_duration_s` may only **lower** it) | Drops tasks whose **known** duration is at or over the ceiling — pack `durations` / `task_durations.json` first, then duration keys in task metadata. A declared **timeout** (`agent.timeout_sec`, …) is a ceiling, not a duration, and never counts. **No gate at all when neither is set.** `params.exclude_unknown_duration = "true"` drops tasks with no duration under a gate. |
| then | `params.n_tasks` (`PROOF_PARAM_N_TASKS`) | Keep the first N of what is left (`1` is the smoke shape). |

An empty result fails closed. The kept tasks are **copied** to
`$PROOF_WORK_DIR/tasks-filtered` (never the pack itself) and
`.proof-task-filter.json` in that copy records the source, what was kept,
what was dropped, and why. `harness/pack_filter.example.json` shows the pack
side with placeholder names.

**A malformed `filter.json` fails closed, never reads as absent.** Every
present field is shape-checked: `allow` / `deny` must be lists of task
names and `allow` must not be empty (an empty allow-list is not "every
task"); `slices` a non-empty object of label → non-empty list;
`max_duration_s` a positive integer; `durations` an object of task name →
positive integer seconds (also enforced on `task_durations.json`);
`exclude_unknown_duration` a JSON boolean. Keys starting with `_` are
comments; any other unknown key is refused as a typo. A pack whose filter
is malformed therefore scores nothing until it is fixed and re-pinned,
rather than scoring a wider or less-gated set than the operator meant.

## Timeouts (topic data)

| Param | Env | Effect |
|-------|-----|--------|
| `exec_timeout_s` | `PROOF_EXEC_TIMEOUT_S` | Default `timeout_sec` for one `environment.exec(...)` call the miner's harness makes **without** its own. The custom-Python wrapper wraps the environment it hands the miner; a `timeout_sec` the miner passes explicitly is theirs and is left alone. Script harnesses read the variable. Unset = Harbor's default (no per-command timeout). |
| `timeout_multiplier` | Harbor `--timeout-multiplier` | Scales every pack-declared timeout. Passed only when signed. |
| `agent_timeout_multiplier` | Harbor `--agent-timeout-multiplier` | Harness (agent) timeout only. |
| `verifier_timeout_multiplier` | Harbor `--verifier-timeout-multiplier` | Verifier timeout only. |
| `env_build_timeout_multiplier` | Harbor `--environment-build-timeout-multiplier` | Environment build timeout only. |

The per-task agent and verifier wall clocks themselves stay pack content
(`task.toml`); the run as a whole is still held to the topic's
`eval_executor.max_proof_deadline_s` by the guest agent. Nothing here
carries a compiled number.

## Trial outcomes (`harness/summarize.py`)

`primary_value` is the mean over every **scored** trial of the selected
set. A trial is **measured** only with matching Harbor
`verifier_result.rewards.reward` **and** `verifier/reward.txt` (the paid
value is the JSON verifier reward). Fail-closed (no invented number, no
leftover `report.json`) when nothing was scored, the scored set does not
cover every selected task directory, `reward.txt` is present without
matching Harbor JSON, or a Harbor job snapshot is still running /
`finished_at=null` (same refuse as host harvest). A nonzero Harbor or
script-harness exit still scores when those checks pass; `harbor_exit`
stays in evidence.

**A task the miner's harness crashed on is topic policy** —
`params.agent_exception_policy`:

| Value | A trial with `exception_info` raised in the harness phase (agent started, verifier never started, no reward) |
|-------|------|
| `fail` (default) | No measurement. The selected set is incomplete → the run fails closed (503, no row) — today's behaviour. |
| `zero` | Scores **0.0** — the miner's harness did not solve the task (a crash, an unhandled `environment.exec` timeout). Harbor's own agent timeout is not this case: Harbor records it and still runs the verifier, so that trial is measured. The exception type and first message line land in `evidence.agent_exception_trials`; `n_measured` / `n_agent_exceptions` / `n_scored` are split out. |

Under **either** policy an unmeasured trial that is **not** a harness-phase
failure — environment build / start failure before the agent ran, a
verifier that raised, agent setup failure, a trial with no `exception_info`,
a `reward.txt` left beside the exception — is never a score. Those are the
operator's infrastructure, and inventing a 0 for them would charge the miner
for it. The topic that wants harness crashes counted as failed tasks signs
`agent_exception_policy = "zero"`; nothing about it is compiled in.

## Inspect (`inspect_scan.py`)

How each checklist rule is ticked is **signed topic data**:

| Param | Shape | Meaning |
|-------|-------|---------|
| `inspect_marker_rules` | `<rule_id>:<marker>\|<marker>;<rule_id>:<marker>` | A rule that **fails** when any marker (case-insensitive substring) appears in the artefact text or file names. Rule-id strings are never markers (naming a rule in a README is compliance language); a marker that occurs inside a rule id is refused at parse time. A file / byte-limit truncation fails every marker rule — truncated absence is not a clean pass. |
| `inspect_attested_rules` | `rule_a,rule_b` | Rules the host / topic enforce outside this scan (sandbox attestation, BYOK routing, seed, promotion policy). They pass with evidence saying so; inspect ran no inference. |

A rule the topic names in neither list **fails closed** with evidence naming
the two params — an unknown rule is never a silent pass, and no rule id or
marker list is compiled into the adaptor. Inspect never sees a key and
never runs the miner's code.

## BYOK

If the topic sets `constraints.params.miner_byok` (`PROOF_PARAM_MINER_BYOK`):

- **evaluate:** export that variable from
  `$PROOF_MINER_ENV_DIR/$PROOF_PARAM_MINER_BYOK` (`0600`). If
  `PROOF_MINER_ENV_DIR` is unset, the adaptor creates one
  (`$PROOF_SECRETS_DIR/miner` or `$PROOF_WORK_DIR/miner-env`) and copies an
  already-exported value into it. Fail closed only when the key is still
  missing after that staging — never because the dir env var was never
  set. **No** owner `PROOF_SECRETS_DIR` fallback.
- **baseline:** use the miner file when it is staged; otherwise the owner
  key `PROOF_SECRETS_DIR/$PROOF_PARAM_INFERENCE_KEY_FILE` into
  `PROOF_PARAM_INFERENCE_KEY_ENV` (operator-paid reference run).

Never print key material. The guest agent also redacts log tails.

## Environment (adaptor inputs)

From the guest contract (`deploy/guest/runners/README.md`): `PROOF_JOB`,
`PROOF_PACK_DIR`, `PROOF_ARTIFACT_DIR`, `PROOF_OUTPUT_DIR`, `PROOF_WORK_DIR`,
`PROOF_SEED`, `PROOF_MODEL_PIN`, `PROOF_TASK_SLICE`, `PROOF_DEADLINE_S`,
`PROOF_PARAM_*`, `PROOF_MINER_ENV_DIR` / `PROOF_MINER_ENV_NAMES`,
`PROOF_SECRETS_DIR`.

Topic params this adaptor reads (all optional except `tasks_dir`; none
defaults to a benchmark name):

| Param | Env | Role |
|-------|------|------|
| `tasks_dir` | `PROOF_PARAM_TASKS_DIR` | Relative path under the pack. Refused if absolute or contains `..` |
| `tasks`, `task_exclude`, `n_tasks`, `max_task_duration_s`, `exclude_unknown_duration` | `PROOF_PARAM_*` | § Task selection |
| `task_filter` | `PROOF_PARAM_TASK_FILTER` | Optional relative pack path of the filter file (default `filter.json` / `task_filter.json`) |
| `exec_timeout_s`, `timeout_multiplier`, `agent_timeout_multiplier`, `verifier_timeout_multiplier`, `env_build_timeout_multiplier` | `PROOF_PARAM_*` | § Timeouts |
| `agent_exception_policy` | `PROOF_PARAM_AGENT_EXCEPTION_POLICY` | § Trial outcomes (`fail` default / `zero`) |
| `inspect_marker_rules`, `inspect_attested_rules` | `PROOF_PARAM_INSPECT_*` | § Inspect |
| `model` | `PROOF_PARAM_MODEL` | Harbor / LiteLLM id (`openrouter/vendor/model`). Harbor `-m` is `PROOF_PARAM_MODEL` falling back to `PROOF_MODEL_PIN`. Canon `model_pin` stays `vendor/model`. An OpenRouter path fails closed unless the id already has the `openrouter/` provider prefix. |
| `harbor_agent` | `PROOF_PARAM_HARBOR_AGENT` | Topic built-in for **baseline only** when no miner harness |
| `miner_byok` | `PROOF_PARAM_MINER_BYOK` | Miner key name |
| `inference_key_file` / `inference_key_env` | owner key for baseline when miner_byok is unused |
| `n_concurrent` / `n_attempts` | Harbor `--n-concurrent` / `--n-attempts` (default `1`) |
| `harbor_environment` | `PROOF_PARAM_HARBOR_ENVIRONMENT` | Passed as Harbor `--env` (default **`docker`**). `no-network` / `none` are ignored — agents need the internet. |
| `task_network_mode` | `PROOF_PARAM_TASK_NETWORK_MODE` | `public` (default): rewrite `network_mode` / `allow_internet` on the **filtered copy** so agents can reach the pinned model (Docker `no-network` is unsupported on this guest). `keep`: leave the pack's own settings. |
| `ensure_verifier_pytest` | `PROOF_PARAM_ENSURE_VERIFIER_PYTEST` | `true` (default): inject pytest into filtered-copy environment / verifier / tests images so a missing pytest is an image error, not a false 0. `false`: leave the images as packed. |

Tasks stay the operator pack. The miner attach surface is the **harness**,
not the task list. Host nftables on the VM TAP remain the egress allowlist;
the network rewrite does not open the host.

### Harbor model id (OpenRouter)

Harbor `-m` / LiteLLM must receive the **full** OpenRouter / LiteLLM id
(`openrouter/vendor/model`), never a stripped `vendor/model`. The adaptor
sets `MODEL="${PROOF_PARAM_MODEL:-$PROOF_MODEL_PIN}"` and passes `-m "$MODEL"`.
Set `params.model` to the Harbor/LiteLLM id; leave `constraints.model_pin`
as canon `vendor/model` (`proof-canon` rejects `a/b/c`). An OpenRouter path
(`miner_byok` / `inference_key_env` = `OPENROUTER_API_KEY`) **fails closed**
unless that id already has the `openrouter/` provider prefix — the adaptor
does not rewrite the pin. `ProofPythonAgent` restores that prefix if Harbor
passed the suffix as `model_name`.

## Outputs

`run` writes `$PROOF_OUTPUT_DIR/report.json`:

```json
{
  "primary_value": 0.5,
  "claim_holds": true,
  "evidence": {
    "trials": [
      {"name": "task-a__1", "reward": 1.0, "outcome": "measured"},
      {"name": "task-b__1", "reward": 0.0, "outcome": "agent_exception",
       "exception_type": "RuntimeError", "exception_message": "Command timed out after 120 seconds"}
    ],
    "n_scored": 2,
    "n_measured": 1,
    "n_agent_exceptions": 1,
    "agent_exception_policy": "zero",
    "agent_exception_trials": [{"name": "task-b__1", "exception_type": "RuntimeError", "exception_message": "…"}],
    "mean_reward": 0.5,
    "harbor_exit": 0,
    "harbor_run_tail": "…",
    "agent": "proof_python_agent:ProofPythonAgent",
    "agent_source": "artifact_dir/recipe/agent",
    "harness_kind": "python"
  }
}
```

Evidence may truncate the serialized trial list; the mean does not.
Miner-authored `report.json` is deleted and ignored — it does not skip
Harbor summarize.

`inspect` writes `$PROOF_OUTPUT_DIR/checklist.json` (no Harbor, no keys).

## Single-task smoke

Development does not wait for a full pack. The smoke is a **topic shape**,
not a code path: a run request whose `constraints.params` carry
`tasks = "<one task>"` (or `n_tasks = "1"`) runs this same adaptor, through
the same guest agent, over exactly one task. `deploy/scripts/proof-experiment-smoke.py`
builds that request from a live topic document and drives the real guest
agent (`--driver agent`, stdio frames), the adaptor directly
(`--driver exec`), or the KVM-host orchestrator (`--driver orch`, a real
Firecracker experiment VM). Runbook:
[`docs/runbooks/proof-experiment-smoke.md`](../../../../docs/runbooks/proof-experiment-smoke.md).

## Operator bake / deploy

Harbor itself stays an operator overlay (venv / `--chroot-hook`). This
directory is the adaptor + summarize harness.

**Preferred — bake the adaptor into the guest image:**

```bash
deploy/guest/bake-rootfs.sh \
  --guest-agent target/x86_64-unknown-linux-musl/release/proof-vm-guest-agent \
  --runner rlm_fc_in_guest_harbor="$(pwd)/deploy/guest/runners/rlm_fc_in_guest_harbor" \
  --overlay /path/outside/git/harbor-venv-overlay \
  --chroot-hook /path/outside/git/install-harbor.sh \
  --resolver <allowlisted resolver> \
  --out-dir ./out
```

`--runner` copies this whole tree to `/opt/proof/runners/rlm_fc_in_guest_harbor/`.
`run` execs `harness/run-harbor` next to it. Do **not** keep an old overlay
script at `/opt/proof/harness/run-harbor` as the evaluate path.

**Metal copy (no re-bake), matching the live runners dir:**

```bash
install -d -m 0755 /var/lib/proof/runners/rlm_fc_in_guest_harbor
cp -a deploy/guest/runners/rlm_fc_in_guest_harbor/. \
  /var/lib/proof/runners/rlm_fc_in_guest_harbor/
chmod 0755 /var/lib/proof/runners/rlm_fc_in_guest_harbor/run \
  /var/lib/proof/runners/rlm_fc_in_guest_harbor/inspect \
  /var/lib/proof/runners/rlm_fc_in_guest_harbor/harness/run-harbor
```

Then re-bake or remount so the guest image actually contains those files.
Copying onto the KVM host overlay is not enough unless the guest rootfs
includes them.

`tests/` is CI-only (run by `cargo test -p proof-vm-guest`); omit it on
metal if you want a smaller copy. `host/summarize_job.py` is a KVM-host RCA
helper (nested orch `job.out`); it is not the guest `run` path. Pass
`--jobdir <any-dir>` — no baked `JOBDIR`. It writes `custom_value.txt` and
`summary.txt` under JOBDIR. `host/run-n15` waits for Harbor `curl.pid` /
`job.out` (including `--restart`) and always invokes that helper.

Host harvest (`proof-fc-harvest`) refreshes `{jail}/harvest-work` after
vsock `Done` until trial `result.json` / `verifier/reward.txt` and Harbor
`n_running` / `stats.n_running_trials` would pass fail-closed checks.
Dump-only reconstruct that still lags refuses. That is not an adaptor rewrite
of `reward.txt`.

Topic params must name this runner id and a relative `tasks_dir` inside the
pinned pack. Re-pin `experiment_pack_digest` when the pack tar bytes change.

### Migrating a topic that relied on the old compiled lists

Earlier versions of this adaptor compiled a task allow-list, an
infrastructure deny-list, and the rule ids of one topic. A topic that ran
on them must now **say** those things in its signed document (re-sign) or
its pack (re-pin). What each old behaviour becomes:

| Old (compiled) | Now (topic data) |
|----------------|------------------|
| `task_slice` label meaning "the pack minus INFRA excludes" | `params.task_exclude = "<broken-task>,<broken-task>"` **or** pack `filter.json` → `deny` (and, optionally, `slices/<label>.json` naming the set) |
| 6-task "short" allow-list + `3600 s` gate | pack `filter.json` → `allow` / `slices`, `params.max_task_duration_s`, pack `durations` |
| Rule ids ticked by name | `params.inspect_marker_rules = "<rule>:<marker>\|<marker>;…"` + `params.inspect_attested_rules = "<rule>,<rule>"` |
| Harness crash → whole run 503 | keep (default `fail`) or sign `params.agent_exception_policy = "zero"` |
| Silent `environment.exec` timeout in miner code | sign `params.exec_timeout_s`; miners may still pass their own |

Until the topic carries the inspect params, **every** submission to it is a
persisted `rejected` row (red checklist, no spend) whose evidence names the
missing params — visible and recoverable by re-signing, never a silent
pass. Until it carries a task selection, the whole `tasks_dir` is scored.

## Guest runtime (read-only rootfs)

- `TMPDIR` / `TMP` / `TEMP` → `$PROOF_WORK_DIR/tmp` (`/var/tmp` is often not
  writable on the RO rootfs).
- **Docker first.** If `dockerd` / `/var/run/docker.sock` is present (rootful
  overlay + host-tools), the adaptor uses that daemon and Harbor
  `--env docker`. Native overlay; no fuse budget. `init.sh` bind-mounts the
  engine store onto the scratch drive and starts `dockerd` when it is on
  PATH.
- Rootless `podman system service` + `DOCKER_HOST` is the fallback when
  Docker is absent, so Harbor's Docker environment can still talk to an API
  socket (same pattern as `deploy/guest/runners/README.md`).
- `docker-compose` is **not** aliased to `podman-compose`.
- `PROOF_HARNESS_SKIP_PODMAN=1` (or `PROOF_HARNESS_SKIP_RUNTIME=1`) skips
  the socket (unit tests).

## Miner-facing guide

Attach layout and BYOK: [`docs/external-miner/proof-tbench.md`](../../../../docs/external-miner/proof-tbench.md).
Generic contract: [`../README.md`](../README.md).
