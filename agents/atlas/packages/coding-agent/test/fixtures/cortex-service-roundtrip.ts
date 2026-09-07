import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai";
import { cortexRuntimeBinding } from "../../src/cortex/binding.js";
import { FileBudgetJournal } from "../../src/cortex/budget-journal.js";
import { type CortexScope, TreeBudget } from "../../src/cortex/policy.js";
import { CortexRuntime } from "../../src/cortex/runtime.js";
import { CortexSandbox } from "../../src/cortex/sandbox.js";
import { CortexServiceBroker } from "../../src/cortex/service-broker.js";
import { createHarness, getMessageText } from "../suite/harness.js";

const socket = process.env.CORTEX_BRIDGE_SOCKET;
const image = process.env.CORTEX_TEST_KERNEL_IMAGE;
if (!socket || !image || !process.env.CORTEX_BRIDGE_SCOPE) throw new Error("Missing local bridge fixture settings");
const scope = JSON.parse(process.env.CORTEX_BRIDGE_SCOPE) as CortexScope;
const harness = await createHarness();
const dir = await mkdtemp(join(tmpdir(), "cortex-bridge-rlm-"));
const sandbox = new CortexSandbox(join(dir, "sandbox"), {
	image,
	memoryMb: 512,
	workspaceMb: 64,
	cpus: 1,
	pids: 64,
	seconds: 60,
});
const budget = new TreeBudget(
	{
		maxDepth: 2,
		maxChildren: 2,
		maxConcurrentCalls: 2,
		maxCalls: 8,
		maxReservedTokens: 10_000_000,
		maxReservedMicroUsd: 10_000_000,
		timeoutMs: 60_000,
	},
	Date.now,
	new FileBudgetJournal(join(dir, "budget.json"), cortexRuntimeBinding(scope, harness.getModel(), sandbox.limits)),
);
const broker = new CortexServiceBroker(socket, scope);
let events = 0;
const runtime = new CortexRuntime({
	scope,
	stateDir: join(dir, "state"),
	workspace: join(dir, "work"),
	sandbox,
	model: harness.getModel(),
	modelRegistry: harness.session.modelRegistry,
	authStorage: harness.authStorage,
	budget,
	broker,
	record: () => {
		events++;
	},
});
harness.setResponses([
	fauxAssistantMessage(
		fauxToolCall("ipython", {
			code: `
from rlm import host_request
quote = await host_request('cortex.call', {'operation': 'quote', 'arguments': {}})
assert quote['quote']['recipe_digest'] == ${JSON.stringify(scope.commitment)}
report = await host_request('cortex.call', {'operation': 'report', 'arguments': {'text': 'Untrusted local agent finding'}})
assert report['authoritative'] is False
await host_request('cortex.call', {'operation': 'collect', 'arguments': {}})
retained = await host_request('cortex.call', {'operation': 'read_evidence', 'arguments': {}})
assert retained['evidence']['passed'] is True
print('retained-through-private-controller')
`,
		}),
		{ stopReason: "toolUse" },
	),
	fauxAssistantMessage("Controller retained the synthetic observations."),
]);
try {
	const root = await runtime.start();
	await runtime.prompt("Run the local controller integration fixture.");
	assert(root.messages.some((message) => getMessageText(message).includes("retained-through-private-controller")));
	assert(events > 0);
	const signal = new AbortController().signal;
	await assert.rejects(broker.call(scope, "collect", { measurements: [] }, signal));
	await assert.rejects(broker.call({ ...scope, id: "foreign" }, "read_evidence", {}, signal));
} finally {
	await runtime.stop();
	harness.cleanup();
	await rm(dir, { recursive: true, force: true });
}
