# Runner `rlm_fc_in_guest_harbor`

Versioned **reference adaptor** for in-guest Harbor evaluate. It is operator
bake content, not compiled into any Proof binary. A signed topic selects it
only by putting this exact id in `constraints.params.baseline_runner`
(alias `in_guest_benchmark_runner`). Any other id still fails closed until
that id is baked.

This tree exists because the live overlay
`/var/lib/proof/overlays/harbor-harness/opt/proof/harness/run-harbor` (and the
thin adaptor that `exec`'d it) **ignored `$PROOF_ARTIFACT_DIR`** on evaluate
(`harbor run … -a terminus-2` on the operator pack) and always loaded the
**owner** key from `PROOF_SECRETS_DIR`. Inspect already saw the artefact;
evaluate did not use the miner code. That gap is the bug this adaptor closes.

The evaluate path is **not** Terminus-2-only. Custom Python is the primary
miner harness; Harbor `BaseAgent` subclasses, an explicit `harness.json`, and
a classic `run.sh` are also accepted. Built-in names such as `terminus-2` are
used only for **baseline** with no miner artefact, or when the miner names
one in `harness.json`.

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
| `python` (primary) | Custom Python class (`Agent` / `ProofAgent` / any class named in `import_path`). Need not subclass Harbor `BaseAgent`. Harbor `-a` is the in-tree wrapper `proof_python_agent:ProofPythonAgent`, which imports **your** class from the artefact only. |
| `harbor` | Harbor `BaseAgent` / `BaseInstalledAgent` subclass, passed as `-a module:Class` |
| `script` | Miner executable relative to the artefact. Docker, filtered tasks, and BYOK are already set. Writes `report.json` or a Harbor jobs dir. Never wrapped as `terminus-2`. |
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
that file is refused: that would ignore miner code the same way `terminus-2`
did. The named module is resolved in the evaluate import env (artefact
parent only) and **rejected** if its origin is outside the staged artefact.

## Agent selection (miner attach surface)

| Job | Artefact | What runs |
|-----|----------|-----------|
| `evaluate` | custom Python at `agent/` / `recipe/agent/` | wrapper `-a proof_python_agent:ProofPythonAgent` |
| `evaluate` | Harbor `BaseAgent` dir | that import path |
| `evaluate` | `harness.json` | the named kind |
| `evaluate` | `$PROOF_ARTIFACT_DIR/recipe/run.sh` only | **script harness** — not wrapped as `terminus-2` |
| `evaluate` | artefact staged, nothing matching | **fail closed** — never the topic agent |
| `baseline` | `PROOF_ARTIFACT_DIR` unset | topic `PROOF_PARAM_HARBOR_AGENT` (operator pack agent, e.g. `terminus-2`) |
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
  run.sh            # optional script harness; evaluate execs it, does not wrap terminus-2
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

Off-limits in the artefact (inspect fails the named rule):

- `no_eval_short_circuit` (and `skip_eval` / `skip_verifier` / `always_pass_eval` / `short_circuit_eval`)
- `no_tb4_hardcoding` (and `tb4_answers` / `hardcoded_tb4`)

A file/byte-limit truncation marks the scan incomplete and fails those
off-limits rules. Unknown rule ids fail closed. Do not quote those markers
in miner code or README inside the tar.

## BYOK

If the topic sets `constraints.params.miner_byok` (`PROOF_PARAM_MINER_BYOK`):

- **evaluate:** export that variable from
  `$PROOF_MINER_ENV_DIR/$PROOF_PARAM_MINER_BYOK` (`0600`). Missing file →
  fail closed. **No** owner `PROOF_SECRETS_DIR` fallback.
- **baseline:** use the miner file when it is staged; otherwise the owner
  key `PROOF_SECRETS_DIR/$PROOF_PARAM_INFERENCE_KEY_FILE` into
  `PROOF_PARAM_INFERENCE_KEY_ENV` (operator-paid reference run).

Never print key material. The guest agent also redacts log tails.

## Environment (adaptor inputs)

From the guest contract (`deploy/guest/runners/README.md`): `PROOF_JOB`,
`PROOF_PACK_DIR`, `PROOF_ARTIFACT_DIR`, `PROOF_OUTPUT_DIR`, `PROOF_WORK_DIR`,
`PROOF_SEED`, `PROOF_MODEL_PIN`, `PROOF_TASK_SLICE`, `PROOF_PARAM_*`,
`PROOF_MINER_ENV_DIR` / `PROOF_MINER_ENV_NAMES`, `PROOF_SECRETS_DIR`.

Topic params this adaptor reads (all optional except `tasks_dir`; never
hardcoded to a benchmark name):

| Param | Env | Role |
|-------|------|------|
| `tasks_dir` | `PROOF_PARAM_TASKS_DIR` | Relative path under the pack. Refused if absolute or contains `..` |
| `max_task_duration_s` | `PROOF_PARAM_MAX_TASK_DURATION_S` | Drop pack tasks whose duration metadata is ≥ this many seconds (default **3600**). Pack `filter.json` may only **lower** the ceiling. Adaptor `duration_hints.json` (n15 x0017 walls) still applies. |
| `task_filter` | `PROOF_PARAM_TASK_FILTER` | Optional relative pack path to `filter.json` / allow-list |
| `exclude_unknown_duration` | `PROOF_PARAM_EXCLUDE_UNKNOWN_DURATION` | `true` to drop tasks with no duration metadata |
| `harbor_agent` | `PROOF_PARAM_HARBOR_AGENT` | Topic built-in for **baseline only** when no miner harness |
| `miner_byok` | `PROOF_PARAM_MINER_BYOK` | Miner key name |
| `inference_key_file` / `inference_key_env` | owner key for baseline when miner_byok is unused |
| `n_concurrent` / `n_attempts` | Harbor `--n-concurrent` / `--n-attempts` (default `1`) |
| `harbor_environment` | `PROOF_PARAM_HARBOR_ENVIRONMENT` | Passed as Harbor `--env` (default **`docker`**). `no-network` / `none` are ignored — agents need the internet. |

Tasks stay the operator pack. The miner attach surface is the **harness**,
not the task list. Before Harbor runs, the adaptor copies surviving tasks
to `$PROOF_WORK_DIR/tasks-filtered` and rewrites Harbor `network_mode` to
**`public`** on that copy (Docker `no-network` is unsupported on this guest
and blocked agent OpenRouter calls; n15 hit `ValueError network_mode=no-network
unsupported` on batched-eval-parity). Host nftables on the VM TAP remain the
egress allowlist; this rewrite does not open the host. A filtered copy with
zero tasks fails closed.

Operator pack hint (pack content, not compiled in): ship `filter.json` with
`max_duration_s`, optional `allow` / `deny` directory names (aliases match
`biped` → `biped-contact-dynamics`), and/or `task_durations.json`. See
`harness/pack_filter.example.json`. Tasks whose `task.toml` `[agent]
timeout_sec` (or equivalent) is ≥ 1 hour are dropped even without that file.
Adaptor `harness/duration_hints.json` records retained n15 x0017 walls so
`biped` / `formal-crypto` / `cad` / `data-anon` drop even with no timeout.
Do not put hour-plus tasks in the default scorable pack for `n_concurrent`
baselines or miner evals.

After the copy, `ensure_verifier.py` injects pytest into environment /
verifier Dockerfiles so Harbor's verifier cannot score 0 from `pytest:
command not found` (n15 biped + cad). That is an image/env hole, not a
true-zero miner reward. Partial pytest assertion failures remain real 0s.

## Outputs

`run` writes `$PROOF_OUTPUT_DIR/report.json`:

```json
{
  "primary_value": 0.73,
  "claim_holds": true,
  "evidence": {
    "trials": [{"name": "task__1", "reward": 1.0}],
    "n_measured": 1,
    "mean_reward": 0.73,
    "harbor_run_tail": "…",
    "agent": "proof_python_agent:ProofPythonAgent",
    "agent_source": "artifact_dir/recipe/agent",
    "harness_kind": "python"
  }
}
```

`primary_value` is the mean of **every** Harbor trial
`verifier_result.rewards.reward`. Evidence may truncate the serialized
trial list; the mean does not. No measured trial, or a nonzero Harbor
exit, → fail closed, no invented number and no leftover `report.json`.

`inspect` writes `$PROOF_OUTPUT_DIR/checklist.json` (no Harbor, no keys).

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
`run` execs `harness/run-harbor` next to it. Do **not** keep the old overlay
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

Optional overlay layout if you still ship harness files under
`/opt/proof/harness/`: copy `harness/run-harbor` + `summarize.py` there
**and** point the adaptor `run` at them only after this tree's evaluate
behavior is in that copy. The in-tree `run` does not exec the old overlay.

`tests/` is CI-only; omit it on metal if you want a smaller copy.

Topic params must name this runner id and a relative `tasks_dir` inside the
pinned pack. Re-pin `experiment_pack_digest` when the pack tar bytes change.

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
