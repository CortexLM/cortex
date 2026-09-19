# Python service configuration

The master reads environment variables through `MasterConfig.from_env()`.
For variables beginning with `BASE_`, the matching `CORTEX_` alias is accepted;
conflicting values are an error. Existing names and crypto domains are preserved.

## Master

| Variable | Meaning |
| --- | --- |
| `BASE_NETUID` | Required subnet UID |
| `BASE_CHAIN_ENDPOINT` | Bittensor network name or RPC endpoint |
| `BASE_STATE_DIR` | Durable private state directory; default `/var/lib/cortex` |
| `BASE_OWNER_PUBKEY_FILE` | Trust-root signing public key |
| `BASE_CHALLENGES_FILE` | Signed challenge configuration; adjacent `.sig` required |
| `BASE_MEASUREMENTS_FILE` | Signed measurement configuration; adjacent `.sig` required |
| `BASE_GATEWAY_SK_FILE` | Gateway seal seed file |
| `BOUNTY_SK_FILE` | Bounty leaf seed file |
| `PROOF_SK_FILE` | Proof topic and leaf seed file |
| `BOUNTY_SESSION_SECRET_FILE` | Separate pairing session secret |
| `BASE_GATEWAY_ADMIN_TOKEN_FILE` | Required operator bearer file |
| `BOUNTY_BACKEND_PUBLIC_URL` | CortexLM/backend HTTPS public feed |
| `PROOF_VM_ORCHESTRATOR_URL` | Dedicated VM host HTTPS origin |
| `PROOF_VM_ORCHESTRATOR_TOKEN_FILE` | Rotating bearer file for that host |
| `PROOF_VM_ORCHESTRATOR_CA_FILE` | CA file for the host's TLS certificate |
| `PROOF_RLM_VM_IMAGE_DIGEST` | Exact installed rootfs pin, `sha256:<hex>` |
| `PROOF_RLM_VM_VCPUS` | Shared topic/experiment vCPU request, 1-16; default 16 |
| `PROOF_RLM_VM_MEM_MIB` | Shared topic/experiment RAM request in MiB, 128-32768; default 32768 |
| `PROOF_RLM_VM_DISK_MIB` | Shared topic/experiment writable disk request in MiB, 16384-1048576; default 32768 |
| `PROOF_INFERENCE_OFFER_COMMITMENT` | Exact host inference configuration commitment |
| `PROOF_VM_RUNNER_CUSTOM_IDS` | Comma-separated registered custom runner IDs; empty by default |

Use [the master example](../../deploy/env/master.env.example) with
[the standalone master Compose role](../../deploy/compose/role-master.yml).
Keys and tokens are files with mode 0400 or 0600, never image build arguments.
Signing seeds are 32 bytes or 64 hexadecimal characters. SQLite state and miner
credential vaults must be backed by durable private storage.

One resource shape applies to the persistent topic VM and each fresh experiment
VM. The examples explicitly request 1 vCPU, 1024 MiB RAM and 16384 MiB disk;
omitting these settings keeps the runtime defaults in the table. The host must
advertise compatible `resource_caps` in authenticated `/v1/health`; missing,
malformed or insufficient ceilings make Proof unavailable before submission
acceptance. Oversized requests are rejected, never clamped. Attaching an existing
topic VM also requires its image and resource shape to match exactly: changing
these settings does not resize a running topic. See the
[small-host sizing guide](../../deploy/README.md#small-host-sizing).

## VM host

`cortex vm-host --config /etc/proof-vm/host.toml` starts the HTTPS orchestrator.
The [configuration example](../../deploy/env/proof-vm-host.toml.example) deliberately
contains empty image/offer pins until the operator installs actual artifacts.
The certificate SAN must match every configured hostname or IP used by the master.

`cortex vm-host --config /etc/proof-vm/host.toml --check` checks local prerequisites
and prints JSON with `ready`, `scope` and per-check results; exit status is zero
only when every check passes. It checks KVM capability, artifact hashes,
executable files, private paths, TLS certificate/key/SAN validity and the signed
inference configuration without creating state, starting VMs or making external
requests. It does not execute the binaries or verify a live TLS trust chain.
The separate [KVM smoke procedure](../../deploy/README.md#local-checks-and-kvm-smoke)
boots guests and checks their lifecycle.

`[host]` selects the verified kernel, jailer, Firecracker binary, persistent
state, immutable pack directory, capacities and explicitly registered custom IDs.
`[images]` maps actual rootfs SHA-256 hashes to installed files. `[caps]` can lower
but cannot exceed 16 vCPU or 32 GiB RAM; writable disk is at least 16 GiB.
These per-VM ceilings are exposed as `resource_caps` in authenticated health.
`[host].max_topics` and `max_experiments` separately limit concurrent guests;
size the physical host for both kinds running together, plus host overhead and
retained failure artifacts. Ceilings are not aggregate host capacity reservations.
`[[egress]]` allows exact IPv4 CIDR/port/protocol combinations for topic setup.
Experiment guests have no NIC. The host is a dedicated KVM-capable Droplet on
the VPC; it is not a control-plane subprocess fallback.

`[inference]` selects the model and a private API key file; the default model is
`deepseek/deepseek-v4.1-flash`. The key stays on the host. Guest inference runs
through a budgeted callback and is unavailable until evaluation preflight passes.
`[limits]` accepts the fields defined by `AgentLimits`: calls, tool calls, tokens,
recursion depth, wall time, tool deadline, completion size and context size.

Optional shared memory:

```toml
[knowledge]
state_db = "/var/lib/proof/knowledge.sqlite3"
owner_public_file = "/etc/proof-vm/knowledge-owner.pub"
```

The knowledge owner key is Ed25519, separate from Bittensor hotkeys. Guest
observations are untrusted and private by default. The authenticated host API
lists proposals at `GET /v1/knowledge/pending`; `POST /v1/knowledge/approve`
requires the exact observation and its owner-signed `KnowledgeApproval`.
Approving a public observation explicitly authorizes sharing it between topics.

## Lium adapter boundary

The Lium adapter is a library boundary for explicit operator integration.
`MasterConfig` does not instantiate it, and no `BASE_*` or `PROOF_*` setting
currently enables live Lium harvest. A compatible GPU-isolated guest wire and
digest-pinned evaluator image are still required; the runtime image is not that
evaluator.

`LiumAdapterConfig` requires these operator inputs:

| Field | Requirement |
| --- | --- |
| `api_key_file` | Private provider API-key file |
| `ssh_private_key_file`, `ssh_public_key_file` | Operator SSH identity files |
| `ssh_known_hosts_file` | Operator-provisioned OpenSSH host keys for the provider endpoints |
| `image_repository` | Untagged GHCR repository; the signed executor pin supplies the exact image digest |
| `gpu_name`, `max_price_per_hour` | Exact GPU class and positive price ceiling; rent shape remains `1x` |
| `max_lifetime_hours` | Provider lifetime ceiling, 1 to 24 hours, default 6 |
| `request_timeout_seconds`, `running_timeout_seconds`, `poll_interval_seconds` | Bounded provider request, readiness and polling intervals |

The known-hosts file must be regular, nonempty, at most 1 MiB, owned by root or
the service user, and not writable by group or others. The private SSH key is
at most 16 KiB and has no group/other permissions. Symlinks and special files
are refused. The transport uses `StrictHostKeyChecking=yes`, a dedicated
known-hosts snapshot and no global trust file or ambient SSH configuration.
Readiness and rent validate trust before provider spending; execution rechecks
the files before starting SSH. Unknown keys are never accepted automatically.

The injected backend additionally needs
`PrivateFileMaterialSource(root, topic_public_key=...)`, using an independently
pinned Proof public key and an owned directory with mode 0700 containing
`<topic.content_digest()>.json` files. Each file must be an owned regular file
with mode 0600, one link and at most 128 MiB; symlinks are refused. It contains
the exact `SetupExport` for that signed topic revision. The verified
`HarvestMaterial` binds the pack,
holdout, runner script and FLOPs/wall budgets; the signed topic must contain
`params.experiment_pack_digest = "sha256:<actual-pack-digest>"`. Private exports
are not public topic documents or repository content. Missing material or artifact
bytes refuses execution before rent.

The [executor contract](../PROOF.md#executor-offers) describes versioned request
commitments, returned evidence and the separate experiment-VM teardown requirement.

## Owner topic setup

`POST /challenge/proof/v1/admin/proof/setup` accepts `{"policy": {...}, "env": {...}}`
with the operator bearer. A minimal policy contains `topic_id` and `objective`.
The agent selects a measured custom metric if `metric` is omitted. If several
custom IDs are registered, select one through `custom_id`. Platform resource
budgets have defaults and can be tightened in the policy. An explicit metric
fixes its name, direction and minimum improvement. Agent proposals may tighten
that improvement floor, never loosen it.

The command accepts a policy file without putting credentials on the command line:

```bash
cortex topic-create --gateway https://gateway.example \
  --token-file /run/secrets/operator-token --policy /private/topic-policy.json
```

An optional `--env-file` reads a private JSON map of declared miner BYOK values.
The setup response is the public signed topic; private holdout records are never
included. Topic IDs are dynamic data, not a catalog committed in this repository.

## Live model smoke

This is an opt-in paid API test with synthetic experiment results. It exercises
recursive delegation and context compaction; it does not prove real VM execution.

```bash
uv run python scripts/rlm_smoke.py \
  --key-file /private/openrouter.key --state-dir /private/rlm-smoke
```

Use a fresh state directory for a fresh run. `--resume` preserves the original
budget and deadlines; it cannot reset an exhausted run or replay an uncertain VM job.
