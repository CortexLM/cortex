# Guest runner adaptors (operator capability)

An **adaptor** is what `proof-vm-guest-agent` execs when a signed topic's
`constraints.params` select an in-guest runner. It is baked into the guest
image by the operator (`bake-rootfs.sh --runner <id>=<dir>`) under
`/opt/proof/runners/<id>/`, where `<id>` is exactly the value the topic puts
in `constraints.params.in_guest_benchmark_runner` (alias `baseline_runner`;
shape `[a-z0-9][a-z0-9_-]{1,63}`). No adaptor is compiled into any Proof
binary. A topic that names an id this image does not carry fails closed
(`RlmToHost::Failed` → 503, no row). One **versioned reference adaptor**
ships under [`rlm_fc_in_guest_harbor/`](rlm_fc_in_guest_harbor/) so operators
can bake Harbor evaluate with miner artefact attach; it is still selected
only when a signed topic names that runner id.

This directory holds the **contract** and, when a live gap needs a bakeable
fix, a reference adaptor directory named after the runner id. The Harbor
CLI, its venv, and the task pack stay operator content (`--overlay` /
`--chroot-hook` / `--extra-pkgs`, or the topic-pinned pack). Proof binaries
stay generalist: the words below are generic knobs; every value is topic
data.

The contract lives in `crates/proof-vm-guest/src/runner.rs`; this is the
operator's view of it.

## Entrypoints

| File | Job | Must write under `$PROOF_OUTPUT_DIR` |
|------|-----|--------------------------------------|
| `run` (required) | `Baseline`, `Evaluate` | `report.json` — `{"primary_value": <finite number>, "claim_holds": bool, "flops_used": <int or omit>, "evidence": {...}}` |
| `inspect` | `Inspect` (anti-cheat rules, **before any paid inference**) | `checklist.json` — `[{"id": "<rule id>", "pass": bool, "evidence": "..."}]`; a rule left out is recorded **red** |
| `propose_rules` (optional) | `ProposeRules` | `rules.json` — `[{"id": "<slug>", "text": "..."}]`; without this entrypoint the agent proposes the signed topic's own `checklist` |

A non-zero exit with no document, a missing document, a non-finite
`primary_value`, or a run that outlives `PROOF_DEADLINE_S` is a failed job.
The agent never fills in a value — and neither may the adaptor: a trial that
produced no measurement is reported as what it is (the topic decides whether
that counts as zero or fails the run), never as some other number that
happened to be lying around.

## Environment

| Variable | Meaning |
|----------|---------|
| `PROOF_RUNNER_ID`, `PROOF_JOB` | runner id; `baseline` / `evaluate` / `inspect` / `propose_rules` |
| `PROOF_TOPIC_ID`, `PROOF_CUSTOM_ID`, `PROOF_PRIMARY_METRIC`, `PROOF_METRIC_DIRECTION` | identities; `max` / `min` |
| `PROOF_SUBMISSION_DIGEST`, `PROOF_ARTIFACT_DIGEST` | the run's identities (echoed into the report by the agent) |
| `PROOF_ARTIFACT_DIR` | the miner's artefact, fetched **by the agent** (streamed under a 64 MiB cap), verified against `PROOF_ARTIFACT_DIGEST`, unpacked (set only when the request carries a locator; always set for `evaluate`). `$PROOF_WORK_DIR/artifact.tar` holds the verbatim bytes |
| `PROOF_PACK_DIR`, `PROOF_PACK_DIGEST` | the topic-pinned experiment pack, staged by the host at boot and verified by the agent (paid jobs) |
| `PROOF_MODEL_PIN`, `PROOF_TASK_SLICE` | `constraints.model_pin` / `constraints.task_slice` when the topic carries them |
| `PROOF_SEED`, `PROOF_DEADLINE_S`, `PROOF_DECLARED_FLOPS`, `PROOF_FLOPS_BUDGET` | run parameters from the signed topic and the submission |
| `PROOF_CLAIM_FILE` | the miner's claim text (`run`) |
| `PROOF_RULES_FILE` | the rule set to tick (`inspect`); `PROOF_TOPIC_FILE` the signed topic (`propose_rules`) |
| `PROOF_OUTPUT_DIR`, `PROOF_WORK_DIR` | where to write the answer; scratch on the writable disk |
| `PROOF_SECRETS_DIR`, `PROOF_SECRET_FILES` | owner key material staged by the KVM host (`PROOF_VM_AGENT_OWNER_KEY_DIR`), by file name. **Read them; never print them** — the agent redacts their values from every log tail and evidence string it sends back, but not from anything you write elsewhere |
| `PROOF_PARAM_<KEY>` | one per `constraints.params` entry (key upper-cased, `-` → `_`). This is how a topic tells its adaptor which tasks, agent, concurrency, key file, … to use — **the adaptor never hardcodes them**. Two signed names that collide after that mapping (`foo-bar` / `foo_bar`) are refused before anything runs |
| `PROOF_MINER_ENV_NAMES`, `PROOF_MINER_ENV_DIR` | the **miner's own** keys for this run (paid jobs only), for the variables the signed topic declared in `constraints.params.miner_byok` / `miner_env_allowlist`. Each is exported under its own name and also written to `$PROOF_MINER_ENV_DIR/<NAME>` (0600). Unset when the topic asks for none. Same rule as the owner files: **read them; never print them** — the agent redacts their values from the log tail and evidence it sends back |

`HOME`, `XDG_RUNTIME_DIR`, `PATH`, `LANG` are set for the run-as user;
nothing else of the agent's environment is inherited. stdout / stderr are
drained into a rolling tail (64 KiB total) while the process runs; write
logs you need to keep under `$PROOF_WORK_DIR`.

## Skeleton (contract only — no harness)

The shape every `run` has. Everything a harness needs is a `PROOF_PARAM_*`
from the signed topic; the two marked lines are the operator's, outside git.

```bash
#!/bin/bash
set -euo pipefail
: "${PROOF_PACK_DIR:?}" "${PROOF_OUTPUT_DIR:?}" "${PROOF_WORK_DIR:?}" "${PROOF_JOB:?}"

# Inputs are topic data. Refuse what the topic did not say; default nothing.
tasks_rel="${PROOF_PARAM_TASKS_DIR:?constraints.params.tasks_dir is required}"
case "$tasks_rel" in /*|*..*) echo "tasks_dir must be a plain relative path" >&2; exit 2 ;; esac
tasks="$PROOF_PACK_DIR/$tasks_rel"
[ -d "$tasks" ] || { echo "no $tasks in the staged pack" >&2; exit 2; }

# A provider key, when the topic names one: read from the staged file into
# the variable the topic names; the value never reaches stdout / stderr.
if [ -n "${PROOF_PARAM_INFERENCE_KEY_FILE:-}" ]; then
    : "${PROOF_PARAM_INFERENCE_KEY_ENV:?inference_key_env is required with inference_key_file}"
    export "$PROOF_PARAM_INFERENCE_KEY_ENV"="$(tr -d '\n' < "$PROOF_SECRETS_DIR/$PROOF_PARAM_INFERENCE_KEY_FILE")"
fi

# A topic that makes the *miner* pay for the provider instead (miner_byok).
# The agent both exports the variable and writes it to a 0600 file named
# after it; read the file, because that is the form that survives anything
# the harness does to its own environment. Never echo it, and never fall
# back to the owner key above — a miner run on the operator's credentials is
# the one outcome this path exists to prevent.
if [ -n "${PROOF_PARAM_MINER_BYOK:-}" ]; then
    byok_file="${PROOF_MINER_ENV_DIR:?miner_byok topic with no miner env staged}/$PROOF_PARAM_MINER_BYOK"
    [ -r "$byok_file" ] || { echo "the miner did not supply $PROOF_PARAM_MINER_BYOK" >&2; exit 2; }
    export "$PROOF_PARAM_MINER_BYOK"="$(tr -d '\n' < "$byok_file")"
fi

# Rootful docker when an overlay shipped dockerd; otherwise a rootless
# API socket for harnesses that speak to a Docker daemon. Do not alias
# docker-compose to podman-compose — Compose v2 is `docker compose`.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [ -S /var/run/docker.sock ]; then
    export DOCKER_HOST="unix:///var/run/docker.sock"
elif command -v podman >/dev/null 2>&1; then
    mkdir -p "$XDG_RUNTIME_DIR/podman"
    podman system service --time=0 "unix://$XDG_RUNTIME_DIR/podman/podman.sock" > "$PROOF_WORK_DIR/podman-service.log" 2>&1 &
    trap 'kill $! 2>/dev/null || true' EXIT
    export DOCKER_HOST="unix://$XDG_RUNTIME_DIR/podman/podman.sock"
fi

# OPERATOR: invoke your harness here over "$tasks" with the topic's params
#           (agent, model = "${PROOF_MODEL_PIN:-}", concurrency, ...),
#           held to PROOF_DEADLINE_S, writing under "$PROOF_WORK_DIR".
# OPERATOR: turn its per-trial outputs into report.json. A trial with no
#           measurement is no measurement — never another field's value.
echo "no harness wired into this skeleton" >&2
exit 2
```

Baked as `--runner <id>=<dir>` where `<id>` is what your topics put in
`baseline_runner`. Ship the harness itself with `--extra-pkgs` (Debian
packages), `--overlay DIR` (a tree copied over the rootfs, e.g. a venv), or
`--chroot-hook SCRIPT` (run inside the chroot; pin every version it
installs) — and smoke it on the baked image, as the run-as user, before
signing a topic on it. An adaptor that ships no `inspect` cannot reach
`Evaluate` (the anti-cheat inspection is the topic RLM's work; provide one
or the run stops before any spend, by design).

## Rootless podman inside the guest

The baked image ships podman + crun + fuse-overlayfs + pasta/slirp4netns
with `cgroup_manager = "cgroupfs"` (there is no systemd in the guest). The
store is on writable, run-as-owned paths `init.sh` creates before the agent
starts: `graphroot = /var/lib/proof/containers/storage` (the per-VM scratch
drive; also `~/.local/share/containers/storage` through the home bind) and
`runroot = /run/user/<uid>/containers` (the user's `XDG_RUNTIME_DIR` tmpfs)
— never `/var/lib/containers` or `/run/containers`, which would be
root-owned and, for the former, on the read-only rootfs. Containers are
namespaces, not nested VMs — no nested KVM is used or needed. Known limits,
honestly:

- Image pulls need egress: the registry hosts must be on the KVM host's
  `PROOF_VM_AGENT_EGRESS_ALLOW` (and a resolver, `--resolver` at bake +
  `:53/udp` on the allowlist). Nothing is pre-pulled into the image.
- A harness that talks to a Docker daemon should prefer a live `dockerd`
  (`DOCKER_HOST=unix:///var/run/docker.sock`) when the operator overlay
  ships one (rootful, native overlay, no fuse). Otherwise start a podman
  API socket: `podman system service --time=0 unix://$XDG_RUNTIME_DIR/podman/podman.sock &`
  and `DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock` (the skeleton
  does this). `docker` may resolve to a podman shim when no real docker
  binary is present. Do **not** alias `docker-compose` to `podman-compose`;
  Compose v2 is `docker compose`. Harnesses relying on Docker-only API
  features may still differ; verify with the harness's own smoke run on the
  baked image **before** signing a topic on it.
- Rootless networking is user-mode (pasta / slirp4netns): fine for pulls and
  API calls, slower than bridged.
- fuse-overlayfs needs `/dev/fuse` (`CONFIG_FUSE_FS`); user namespaces need
  `CONFIG_USER_NS`; `bake-rootfs.sh --check-kernel-config` lists the rest.
  A stock Firecracker microVM kernel config often lacks some of them.
- Guest-measured FLOPs: an agentic harness has no FLOP counter. The host
  records `flops_used` when present as telemetry and does not reject on it.
