# Proof operator reference

Proof is 80% of Cortex emission. An operator supplies a research objective; a
topic-scoped recursive language-model agent installs its environment, proposes
measurable rules, creates private evaluation material, measures a baseline and
publishes miner documentation. Miners submit code and artifacts against the
resulting signed topic.

No challenge catalog is compiled into Cortex. Topic statements, metrics,
checklists, endpoints, model pins, runner IDs, experiment packs and holdouts are
operator state produced or approved at runtime.

## Topology

The master is the public control plane. It stores topics, submissions, evidence,
jobs, nonce reservations and score history in SQLite. It reaches a separate VM
host over authenticated HTTPS.

The VM host owns Firecracker, jailer, the kernel, installed rootfs images and
experiment packs. It keeps one persistent topic VM per `topic_id`. Setup and
evaluation agent state lives there. Every baseline or paid miner evaluation uses
a fresh dedicated experiment VM that has no network interface. Successful
experiment VMs are destroyed before their result returns; failed VMs are
retained for private diagnosis.

Production requires real `/dev/kvm`. The fake hypervisor is only a deterministic
test boundary. The master never falls back to running miner or RLM commands on
its own host.

## Topic setup

An authenticated operator sends a `SetupPolicy` to
`POST /challenge/proof/v1/admin/proof/setup`. Required fields are a dynamic
`topic_id` and an objective. The operator may pin a metric, custom runner,
experiment pack, resource budget, epoch window and payout policy. Defaults are
upper bounds; generated content can tighten them but cannot loosen them.

Setup is journaled by a digest of the policy. Retrying the same policy resumes
the same job and requires the same private environment names and values. Reusing
a topic ID with different policy data is a conflict.

Inside the topic VM, the RLM may inspect and install through bounded tools. It
must export:

- a metric with direction and improvement floor;
- a deterministic evaluator and environment digest;
- a private holdout commitment;
- public checklist rules and declarative topic endpoints;
- miner-facing Markdown documentation;
- a measured baseline report from a dedicated experiment VM.

The host verifies exported bytes and evidence bindings. The control plane checks
topic, job, image, environment, holdout, script, report and policy digests,
resource use, baseline metrics and confirmed teardown. Only then does the Proof
key sign topic schema version 2 under `base-proof-topic-v1` and publish it open.
Holdout records and operator credentials are never included in the public topic.

## Recursive language-model engine

Cortex implements its own RLM. It does not depend on Prime Agent or another
agent harness. The host exposes a narrow JSON protocol for model completion,
recursive delegation, VM actions, compaction archive reads and scoped knowledge.

One immutable `AgentLimits` object bounds the entire recursive tree: model calls,
tool calls, tokens, recursion depth, wall time, per-tool timeout, completion size
and context size. A child consumes the same counters and deadline as its parent.
Cancellation or restart cannot reset them.

Before context exceeds its byte budget, the engine stores removed exchanges
verbatim in a content-addressed archive. The compacted prompt retains policy,
phase state, evidence digests, open questions and recent exchanges. The agent can
read an archived item by exact digest; compaction is not lossy deletion and does
not summarize evidence into an unverified claim.

External actions use a durable intent/result journal. An intent is committed
before execution. When resuming an interrupted agent, completed calls are reused
and uncertain paid VM operations require host reconciliation. They are never
silently repeated.

An explicit research retry with `resume: true` can continue an interrupted agent
only while its original topic VM is still running and its guest checkpoint is
available. The host restores the original deadline, reserved inference and tool
budgets, verified checks and measured reports. The request, model and limits must
remain identical; changing the resume flag alone does not change job identity.
Completed experiments are reused. Missing progress, an expired deadline or a
missing guest checkpoint refuses recovery without allocating a fresh budget.

Each VM command records its execution ID, exact action and conversation frame
before dispatch. If its response is lost, the guest requests the existing result
by that ID. The host verifies the original request, topic, job, image, artifact,
pack and completed experiment destruction before restoring the evidence. This
lookup does not execute a command or consume another tool reservation. Missing,
running, failed or unbound results remain blocked, as do legacy intents without
the required identity. Pending work blocks new inference and a final verdict.
This does not implement automatic recovery after a KVM-host reboot: host startup
retains the old VMs, and this recovery path requires the original running guest.

When retrying an identical setup policy or recovering an accepted master job, the
Proof client contacts the same research job. Only the authenticated host response
`409 {"error":"research_resume_required"}` permits one follow-up request with
`request.resume: true`; all other envelope fields stay identical. Other errors
and timeouts do not trigger that retry. The host and guest retain their original
budgets and may still refuse recovery when evidence is unavailable.

Topic observations enter shared knowledge as untrusted and private. Public or
cross-topic visibility requires an owner-signed approval over the exact content,
scope and verification evidence. Approved memory supplies context; it cannot
change a published topic without a new signed revision.

The default operator model is `deepseek/deepseek-v4.1-flash` through an
OpenAI-compatible OpenRouter offer. The offer is signed and binds the provider
origin, model, modes, limits and configuration commitment. The provider API key
is a private host file. A closed, expired, mismatched or unreachable offer is a
readiness failure.

## Topic documents

An open topic binds its statement, revision, payout mode, metric, budgets,
signed params, checklist, sealed baseline, holdout commitment, evaluation image
digest, inference-offer commitment, executor constraints, generated docs,
declarative endpoints and epoch window.

`custom` metrics require an operator-registered custom ID. The built-in `nll`
and `throughput` families retain global floors and require a compatible exact
`1x` executor offer. A topic may require one executor-offer commitment or a
shorter deadline; it cannot select an arbitrary machine or loosen the platform
ceiling.

Topic endpoints expose only three purposes: documentation, submission and
results. They dispatch to the same validated control-plane operations. Generated
content cannot install an arbitrary control-plane route or upstream proxy.

## Submission intake

Miners discover open topics through `GET /v1/proof/topics`. A submission carries
the topic ID, hotkey, exact artifact SHA-256, optional URI, declared FLOPs, claim,
canonical training manifest, a random single-use 64-hex nonce and a hotkey
signature.

The `base-proof-submit-v1` payload binds, in order, hotkey, topic, artifact
digest, declared FLOPs, claim, canonical manifest and nonce. It deliberately
does not bind transport URI or environment values. Those values are checked
before signature verification and nonce reservation, so a malformed BYOK map
cannot consume a valid nonce.

The service rejects before creating a row when the topic is missing, closed,
outside its epoch window, missing a baseline, incompatible with the registered
backend, or bound to an unavailable offer. Signature and nonce failures are 401.
Unknown or malformed environment variables are 400. Infrastructure readiness
failures are 503.

Custom topics require artifact content unless the compatibility URI path is
explicitly used. Uploaded bytes win when both are supplied. Artifacts are
uncompressed tar files with at least one regular nonempty file, at most 5 MiB at
intake, no absolute/traversal path, duplicate entry, symlink, device or special
file, and an exact digest. The VM host verifies the same bytes again before any
experiment.

Topic params may declare `miner_byok` and a name allowlist. Accepted values go
to a 0700 per-job vault with 0600 files and are not stored on the submission row.
The vault directory is written before the queued row, so a crash between the two
leaves an orphan that startup reconciliation removes, never a queued job without
its declared credentials.
The `miner_byok` name is itself a declaration and does not need to appear again
in `miner_env_allowlist`; the VM host validates it before starting the agent.
Deferred work reads them after restart. They enter only the paid guest process
and a private guest directory, join the redaction set, and are deleted when the
job reaches a terminal outcome. A missing required secret leaves deferred work
untouched and returns 503; it never substitutes the owner key.

The service reserves `(hotkey, nonce)` transactionally before starting paid
work. The canonical signed payload digest is stored with that reservation.
Evaluation continues after the HTTP client disconnects. Miners persist a private
receipt before the initial POST and can recover pending or terminal state using
the original signed envelope; lookup never includes `env` or `artifact_uri`,
never reserves a new nonce and returns data only for an exact hotkey, signature,
nonce and payload match.

## Evaluation and anti-cheat

The job freezes the exact topic revision. Later revisions cannot change the
metric or baseline used for an accepted submission. Before paid inference,
Cortex checks manifest/holdout contamination, signed checklist preconditions,
artifact identity, runner and pack availability, executor compatibility and
budgets. A red anti-cheat item persists a reject without provider spend.

The RLM evaluates the untrusted claim and artifact in its topic VM while every
measured command runs in a dedicated experiment VM. An accepted verdict must
reference a report produced by that host job, include every rule result, match
the primary metric value and carry confirmed experiment teardown. Reported FLOPs
come from guest measurement. Custom topics treat declared FLOPs as signed
metadata; built-in metric families enforce measured budgets.

For `nll` and `throughput`, evaluation additionally requires executor offer ID,
offer commitment and configuration commitment, all baseline holdout splits, a
per-split regression ceiling, and the throughput quality floor. Any missing
provenance or measurement is an infrastructure error, not a zero fabricated by
the control plane.

## Executor offers

`GET /v1/proof/executor` is always readable and reports readiness plus a bounded
reason. `POST /v1/admin/proof/executor` rotates or closes the owner-signed live
offer. An offer binds schema, ID, open/closed status, exact `1x` shape, a
digest-scoped template, maximum proof deadline and provider configuration
commitment.

The Python Lium lifecycle validates the offer, constructs a one-GPU plan, uses a
stable job ID, enforces the deadline, bounds transport output, verifies report
provenance, and requires confirmed teardown before returning. Raw guest output
and provider error text are discarded at this boundary: they may contain private
holdout content as well as credentials. Public errors and durable report
rationales use fixed diagnostics, never excerpts of those outputs.

Before rent, the injected backend requires the complete signed topic and miner
submission envelope, verified artifact bytes and private setup material.
`PrivateFileMaterialSource` verifies the topic with the independently pinned
Proof public key and reads `<topic.content_digest()>.json` from its private
directory. That file is a `SetupExport`, containing the exact evaluator pack and
private holdout manifest. Verification binds its environment, holdout and script
digests and its FLOPs/wall budgets to the signed topic; the signed
`params.experiment_pack_digest` must identify those exact pack bytes. Missing or
mismatched material fails before spending. A URI-only submission cannot use this
boundary until its artifact bytes are resolved and verified; this path adds no
control-plane artifact fetcher.

The backend revalidates the request commitment and private material after rent,
before executing the guest, and again after confirmed teardown, before accepting
the returned evidence. Material changed during either stage cannot produce a
score.

`HarvestRequest` and `HarvestMaterial` carry schema version 1. The request includes
the complete topic and signed submission, with a commitment binding their exact
content, job, execution plan and verified material digests. Credentials and private
export/artifact bytes are excluded from public serialization; the commitment
contains environment variable names, never their secret values or transport URI.
Returned evidence must match `request_commitment`, `topic_digest`,
`environment_digest`, `private_holdout_digest` and `inference_offer_commitment`,
in addition to the artifact, image, job and executor bindings. It must identify
an `experiment_vm_id` distinct from the rented provider pod and confirm experiment
teardown. A provider pod's deletion alone is not proof of isolated VM execution.

`LiumRestSshAdapter` implements the reviewed provider wire: digest-bound template
lookup or creation, exact `1x` offer selection, one non-retried rent POST followed
by configuration-bound job-name reconciliation, readiness polling, bounded
OpenSSH transport and delete-plus-404 confirmation. CI supplies `MockTransport`,
a fake SSH boundary and a fake guest contract and never contacts Lium.

SSH requires operator-provisioned host keys in `ssh_known_hosts_file` before
readiness or rent. Unknown or changed host keys fail with strict checking; there
is no trust on first use. The transport ignores ambient SSH configuration and
disables agents, proxies and forwarding. It revalidates the key and trust files
before execution. The runtime image includes `openssh-client`; this dependency
does not enable the adapter or supply host trust.

The adapter is deliberately not selected by the master configuration yet.
`LiumAdapterConfig` requires a private API-key file, SSH key files, a trusted
known-hosts file, the public GHCR repository, GPU class, price/lifetime limits
and control deadlines, while
`MasterConfig` does not currently expose that complete set. Execution also
requires a versioned `LiumGuestWire` that translates the complete harvest request
to a matching guest image and parses its bound result. No compatible GPU-isolated
guest, Python wire or digest-pinned `proof-eval` image is published yet. Without it,
`probe()` is false and execution refuses before contacting the pod. Until the
guest contract, image and all operator inputs are present, `nll` and `throughput`
stay fail-closed; do not substitute the runtime image, a raw template UUID or an
invented digest.

The library configuration and trust-file requirements are listed in
[configuration](reference/configuration.md#lium-adapter-boundary).

Custom Firecracker topics and Lium families are routed independently. Registering
a custom runner does not make Lium ready, and an unavailable Lium adapter does
not close a compatible custom topic.

## Rewards

Every active topic receives an equal portion of the Proof score mass. Each miner
contributes only its best accepted submission for that topic.

In `wta`, exact best-metric ties split the full topic mass. In `discovery`, the
configured pass-floor share splits equally among accepted miners. The remainder
is proportional to improvement beyond the baseline or historical champion.
Near-duplicate or previously rewarded artifact digests receive no novelty mass.
Integer largest-remainder allocation is deterministic, and a miner's global
Proof score is the sum of its topic masses.

When no topic is open, no baseline is sealed or scoring infrastructure is
unavailable, Proof emits `NoScore(ChallengeInternal)` for the expected set. Its
8,000 basis points burn to UID 0 while preserving complete bundle coverage.

## Readiness and verification

`GET /v1/status` reports `can_score`, a bounded fail-closed reason, open-topic
count, custom registrations, custom readiness, Lium readiness, public pins and
executor state. `can_score` means at least one currently open topic has a sealed
baseline and a compatible ready backend; it is not merely process health.

Before opening a live topic, verify the authenticated VM wire, boot one topic VM,
run setup through a dedicated baseline VM, fetch the signed public topic, submit
a real signed artifact, exercise invalid signature/nonce/artifact/BYOK paths,
confirm teardown, emit exact-epoch leaves, seal the gateway bundle and have an
independent validator recompute it. Observe the chain separately before claiming
payment.

Miner request details are in [the public Proof guide](external-miner/proof.md).
