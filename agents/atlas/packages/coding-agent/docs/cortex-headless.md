# Cortex headless worker

The trusted host entrypoint is `src/cortex/headless-cli.ts`. It wraps the existing
`CortexRuntime`, `TreeBudget`, `FileBudgetJournal`, and `CortexServiceBroker`; it is
not the interactive CLI, a daemon, or a replacement runtime.

From the fork root, using **installed** dependencies (no `npx` download):

```sh
/absolute/atlas/node_modules/.bin/tsx --tsconfig /absolute/atlas/tsconfig.json \
  /absolute/atlas/packages/coding-agent/src/cortex/headless-cli.ts \
  --launch /private/scope/launch.json
```

Or use `node --import /absolute/atlas/node_modules/tsx/dist/loader.mjs` with the
same source entrypoint, setting `TSX_TSCONFIG_PATH=/absolute/atlas/tsconfig.json`.
`--stdin` instead of `--launch PATH` reads one JSON document until EOF (maximum
256 KiB, 30-second input timeout). The controller should spawn with an explicit
minimal environment, pipe stdout, and close stdin after the JSON. No credentials
belong in arguments, environment, prompts, workspace, or launch JSON.

## Launch contract

All fields are required; unknown fields are rejected. `CortexLaunchConfig` in
`src/cortex/headless-config.ts` is the authoritative TypeScript type:

```ts
{
  schema_version: 1;
  runtime_id: string;
  deadline_ms: number;
  resume: boolean;
  scope: { role: "experiment" | "atlas"; id: string; commitment: string };
  controller_socket: string;
  state_dir: string;
  workspace: string;
  sandbox_dir: string;
  model_config_file: string;
  kernel: {
    image: string; memoryMb: number; workspaceMb: number;
    cpus: number; pids: number; seconds: number;
  };
  budget: {
    maxDepth: number; maxChildren: number; maxConcurrentCalls: number;
    maxCalls: number; maxReservedTokens: number; maxReservedMicroUsd: number;
    timeoutMs: number;
  };
  prompt: string;
}
```

`runtime_id` is a canonical lowercase UUID (versions 1–8, RFC variant);
`deadline_ms` is the original absolute Unix-millisecond deadline, a positive safe
integer. The Rust controller must persist both immutably **before the first
spawn**. `resume: false` is only for the first attempt with empty runtime state.
Every subsequent attempt must send `resume: true`, even when the previous worker
died before writing any checkpoint.

Paths must be absolute, canonical, nonsymlinked and nonoverlapping. Pre-create
their immediate parent directories as owner-private (0700). The runner creates
`state_dir`, `workspace`, and `sandbox_dir` as 0700 if absent. They must be
dedicated to this scope, not reused for another worker. The existing controller
Unix socket must be owner-private (0600 or 0700) in a private directory. The
launch/model/key files must be owner-only regular files, without hard links.
All paths must belong to the worker's effective UID. Never mount private state,
controller IPC, credentials or sandbox launchers into kernels.

The kernel image must already be installed and digest-pinned; there is no image
pull or local unsandboxed fallback. Kernel resource ceilings match the existing
supervisor: 65536 MiB memory, 4096 MiB workspace, 64 integer CPUs, 1024 PIDs, 86400
seconds. Budget fields are positive safe integers, depth at most 16 and timeout
at most 86400000 ms. The prompt is nonempty and at most 128 KiB.
The absolute deadline must not exceed the worker's current time plus
`budget.timeoutMs`; an earlier deadline caps that timeout. The journal stores
exactly the supplied deadline, not a new spawn-relative deadline. Every kernel
launcher also carries that deadline: the supervisor refuses expired launches and
caps its timeout and lifetime against it, including delayed starts after a crash.

## Private operator model configuration

`model_config_file` holds exactly this schema; the key is in a separate private
file, not JSON:

```ts
{
  schema_version: 1;
  provider: string;
  model: string;
  api: "openai-responses" | "openai-completions" | "anthropic-messages";
  baseUrl: string;
  allowLoopbackHttp?: true;
  apiKeyFile: string;
  reasoning: boolean;
  contextWindow: number;
  maxTokens: number;
  cost: { input: number; output: number; cacheRead: number; cacheWrite: number };
}
```

`baseUrl` defaults to HTTPS with no userinfo, query, fragment, or credentials.
An explicit `allowLoopbackHttp: true` permits HTTP only for literal authorities
`127.0.0.1` and `[::1]` (optional port), for an operator-controlled local proxy.
Omit the flag otherwise; `false`, strings, and other values are rejected.
`localhost`, other loopback addresses, abbreviated/integer/octal IPv4, expanded
IPv6, nonloopback HTTP, userinfo, query, and fragment are rejected. For example,
`http://127.0.0.1:8080/v1` is valid only with the flag. No DNS-based exception or
automatic protocol downgrade exists. The adapter supports text input and API-key
authentication only.
`apiKeyFile` contains a nonempty, single printable-ASCII key, optionally ending
in whitespace, maximum 16 KiB. The model file is at most 64 KiB. Neither file may
be under the workspace, state directory, or sandbox inventory. Keys are read
into host-only in-memory auth storage, never copied into runtime configuration.
No Factory/Prime configuration, OAuth, environment key fallback, command-backed
credentials, provider discovery, retry, or alternate model is used. Set the
actual operator-approved provider/model/endpoint/prices; there is no default.
No inference is needed to validate the configuration, and endpoint availability
is established by the first bounded request, not a separate paid preflight.

`contextWindow` and `maxTokens` are positive safe integers; `maxTokens` must not
exceed `contextWindow`. Prices are finite nonnegative **USD per million tokens**.
Each call conservatively reserves `contextWindow + maxTokens` tokens and
`ceil((contextWindow + maxTokens) * max(cost fields))` micro-USD. Reservations
cover recursive and compaction calls and are never refunded.

`reasoning` declares model capability, not request effort. Reasoning-capable
sessions use the SDK's default thinking level (`medium`); OpenAI Responses
therefore sends `reasoning: {effort: "medium", summary: "auto"}` and includes
`reasoning.encrypted_content`. There is no headless effort override. A direct
request with effort `none` is not an equivalent compatibility check.

## Lifecycle and output

The runner writes `headless.json` and `budget.json` under `state_dir`, alongside
the existing runtime's `tree.json`, private sessions, and lease files. Its
manifest binds `runtime_id`, `deadline_ms`, scope, paths, prompt, kernel/budget
limits and nonsecret model configuration. Only `resume` and `controller_socket`
are excluded. The key bytes may rotate in the same private file.

`resume: true` requires an existing complete manifest, original budget, tree and
sessions. Missing state—including an entirely absent manifest after the first
spawn crashed—fails closed; it never creates a new budget. `resume: false`
refuses any existing run state. Changing identity, deadline (earlier or later),
scope or other immutable settings fails. A restored journal deadline must equal
the supplied original deadline exactly. Never retry a failed resume with
`resume: false`, clear state, or generate a replacement deadline to reset budget.

The Rust controller may supply a new attempt-scoped `controller_socket` after
acquiring its new database fence. Each replacement socket must pass the same
canonical path, owner-private directory/socket and workspace-exclusion checks.
Scope authorization and every other immutable binding remain unchanged. The
existing local budget lease still refuses overlapping workers; cross-host fencing
and revocation of the old attempt's IPC remain the Rust controller's responsibility.

After a crash, startup first removes inventoried kernels through the existing
runtime and reattaches the original tree with the original absolute deadline.
An interrupted root receives the same prompt as a continuation in that tree;
controller operations must therefore remain idempotent. An already-completed
root is not prompted again. A normal completion or SIGTERM/SIGINT calls
`runtime.stop()`, durably revokes the tree budget, and joins descendant cleanup.
A revoked tree cannot resume. Rust must mark graceful cancellation terminal,
reject any subsequent resume safely, and reconcile/clean resources without
resetting budget. The launcher also rejects a resume of a revoked run. SIGKILL
relies on the existing kernel supervisor's
parent-death/deadline cleanup and inventory reconciliation on next start.
Never delete journals or inventories to recover a failed worker.
The dedicated CLI opts into `manageKernelSignalsExternally()` before creating
kernels so the SDK cannot call `process.exit()` before tree shutdown completes.
Normal SDK signal handlers and synchronous exit cleanup remain unchanged.

Stdout has at most two JSON lines: `started` and one terminal `stopped`/`failed`
event (or only `failed` for startup errors). Every line has `schema_version: 1`
and `counts: {calls, reservedTokens, reservedMicroUsd, children}`. `started` has
`restored: boolean`; `stopped` has `reason: "completed" | "cancelled"`; `failed`
has a fixed code: `invalid_launch`, `unsafe_paths`, `model_config_unavailable`,
`reattachment_failed`, `runtime_failed`, `model_failed`, `deadline_exceeded`,
or `cleanup_failed`. SDK/provider stdout and stderr diagnostics are suppressed.
No prompt, model output, raw exception, credential, or scientific measurement
is emitted. Exit status is 0 for clean completion, 1 for failure, 130 for SIGINT,
143 for SIGTERM. Cleanup failures remain failures rather than clean cancellation.
Compaction warnings (for example, a session too short to compact) are not model
failures. Actual compaction errors, aborts and provider errors remain failures.

These events certify only worker lifecycle and conservative accounting. Model
text is untrusted research work, **not scientific evidence**. Evidence, reports,
experiment execution and Atlas decisions use the existing per-scope controller
IPC. The controller remains responsible for authorization, multi-host fencing,
evidence validation, spending and payment. No daemon protocol is changed.

## Focused local tests

Tests preload a faux provider or an in-memory HTTP transport double only in a
test process; production has no faux switch. The transport tests retain cold
built-in OpenAI Responses initialization, request serialization and SSE parsing,
including the 16384-context/2048-output configuration and synthetic HTTP errors.
With an already-built local kernel digest:

```sh
cd /absolute/atlas/packages/coding-agent
CORTEX_TEST_KERNEL_IMAGE=sha256:ACTUAL_LOCAL_IMAGE_DIGEST \
  ../../node_modules/.bin/tsx ../../node_modules/vitest/dist/cli.js \
  --run test/cortex-headless.test.ts test/cortex-kernel-signal-policy.test.ts
```

Without the image environment variable, isolated-runtime cases skip; schema,
private-file, CLI input/redaction tests still run. No paid provider is contacted.
