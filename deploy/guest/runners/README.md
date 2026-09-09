# Guest runner adaptors (operator capability)

An **adaptor** is what `proof-vm-guest-agent` execs when a signed topic's
`constraints.params` select an in-guest runner. It is baked into the guest
image by the operator (`bake-rootfs.sh --runner <id>=<dir>`) under
`/opt/proof/runners/<id>/`, where `<id>` is exactly the value the topic puts
in `constraints.params.in_guest_benchmark_runner` (alias `baseline_runner`;
shape `[a-z0-9][a-z0-9_-]{1,63}`). No adaptor is compiled into any Proof
binary and none ships by default: a topic that names an id this image does
not carry fails closed (`RlmToHost::Failed` → 503, no row).

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
The agent never fills in a value.

## Environment

| Variable | Meaning |
|----------|---------|
| `PROOF_RUNNER_ID`, `PROOF_JOB` | runner id; `baseline` / `evaluate` / `inspect` / `propose_rules` |
| `PROOF_TOPIC_ID`, `PROOF_CUSTOM_ID`, `PROOF_PRIMARY_METRIC`, `PROOF_METRIC_DIRECTION` | identities; `max` / `min` |
| `PROOF_SUBMISSION_DIGEST`, `PROOF_ARTIFACT_DIGEST` | the run's identities (echoed into the report by the agent) |
| `PROOF_ARTIFACT_DIR` | the miner's artefact, fetched **by the agent**, verified against `PROOF_ARTIFACT_DIGEST`, unpacked (set only when the request carries a locator; always set for `evaluate`). `$PROOF_WORK_DIR/artifact.tar` holds the verbatim bytes |
| `PROOF_PACK_DIR`, `PROOF_PACK_DIGEST` | the topic-pinned experiment pack, staged by the host at boot and verified by the agent (paid jobs) |
| `PROOF_MODEL_PIN`, `PROOF_TASK_SLICE` | `constraints.model_pin` / `constraints.task_slice` when the topic carries them |
| `PROOF_SEED`, `PROOF_DEADLINE_S`, `PROOF_DECLARED_FLOPS`, `PROOF_FLOPS_BUDGET` | run parameters from the signed topic and the submission |
| `PROOF_CLAIM_FILE` | the miner's claim text (`run`) |
| `PROOF_RULES_FILE` | the rule set to tick (`inspect`); `PROOF_TOPIC_FILE` the signed topic (`propose_rules`) |
| `PROOF_OUTPUT_DIR`, `PROOF_WORK_DIR` | where to write the answer; scratch on the writable disk |
| `PROOF_SECRETS_DIR`, `PROOF_SECRET_FILES` | owner key material staged by the KVM host (`PROOF_VM_AGENT_OWNER_KEY_DIR`), by file name. **Read them; never print them** — the agent redacts their values from every log tail and evidence string it sends back, but not from anything you write elsewhere |
| `PROOF_PARAM_<KEY>` | one per `constraints.params` entry (key upper-cased, `-` → `_`). This is how a topic tells its adaptor which tasks, agent, concurrency, key file, … to use — **the adaptor never hardcodes them** |

`HOME`, `XDG_RUNTIME_DIR`, `PATH`, `LANG` are set for the run-as user;
nothing else of the agent's environment is inherited.

## Rootless podman inside the guest

The baked image ships podman + crun + fuse-overlayfs + pasta/slirp4netns
with `cgroup_manager = "cgroupfs"` (there is no systemd in the guest) and
the run-as user's container store on the per-VM writable disk. Containers
are namespaces, not nested VMs — no nested KVM is used or needed. Known
limits, honestly:

- Image pulls need egress: the registry hosts must be on the KVM host's
  `PROOF_VM_AGENT_EGRESS_ALLOW` (and a resolver, `--resolver` at bake +
  `:53/udp` on the allowlist). Nothing is pre-pulled into the image.
- A harness that talks to a Docker daemon needs the podman API socket:
  `podman system service --time=0 unix://$XDG_RUNTIME_DIR/podman/podman.sock &`
  and `DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock` (the example
  adaptor does this). `docker` resolves to a podman shim; `docker-compose`
  to `podman-compose` when installed. Harnesses relying on Docker-only API
  features may still differ; verify with the harness's own smoke run on the
  baked image **before** signing a topic on it.
- Rootless networking is user-mode (pasta / slirp4netns): fine for pulls and
  API calls, slower than bridged.
- fuse-overlayfs needs `/dev/fuse` (`CONFIG_FUSE_FS`); user namespaces need
  `CONFIG_USER_NS`; `bake-rootfs.sh --check-kernel-config` lists the rest.
  A stock Firecracker microVM kernel config often lacks some of them.
- Guest-measured FLOPs: an agentic harness has no FLOP counter. Either the
  topic sets `flops_budget: 0` (then `flops_used` may be omitted) or the
  adaptor derives a figure from a topic param — never invents one.

## Example: `harbor-podman`

`deploy/guest/runners/harbor-podman/run` drives the [Harbor](https://pypi.org/project/harbor/)
CLI over rootless podman for every task directory the topic names inside
the pack. Everything it needs is a `PROOF_PARAM_*` from the signed topic:

| Param (`constraints.params`) | Meaning |
|------------------------------|---------|
| `harness_tasks` | relative path inside the pack to a directory of Harbor task dirs (required) |
| `harness_agent` | `harbor run --agent` value (required) |
| `harness_model` | `harbor run --model` value; default `constraints.model_pin` |
| `harness_concurrency` | `--n-concurrent` (default 1) |
| `harness_extra_args` | extra `harbor run` args, word-split (optional) |
| `inference_key_file` | name of the staged secret to export (required when the agent calls a model) |
| `inference_key_env` | env var name the provider expects for that key (required with `inference_key_file`) |
| `flops_per_trial` | optional accounting: `flops_used = flops_per_trial × trials` |

`primary_value` is the mean `verifier_result.rewards.reward` over every
trial `result.json` under the job dirs (a trial with `exception_info` set or
no reward counts 0); `evidence` lists per-trial rewards. Bake it as
`--runner <your topic's runner id>=deploy/guest/runners/harbor-podman
--with-harbor --harbor-version <pinned>`; the bake refuses a Harbor whose
`harbor run --help` lacks a flag the adaptor uses. It ships **no**
`inspect`: the anti-cheat inspection is the topic RLM's work — provide one
(or the run cannot reach `Evaluate`, by design).
