import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AgentSessionEvent } from "../src/core/agent-session.js";
import { cortexRuntimeBinding } from "../src/cortex/binding.js";
import { FileBudgetJournal } from "../src/cortex/budget-journal.js";
import { type CortexScope, TreeBudget } from "../src/cortex/policy.js";
import { type CortexBroker, CortexRuntime } from "../src/cortex/runtime.js";
import { CortexSandbox } from "../src/cortex/sandbox.js";
import { createHarness } from "./suite/harness.js";

export async function cortexFixture(
	id = "round1",
	broker: CortexBroker = { call: async () => ({}) },
	options: { dir?: string; record?: (id: string, event: AgentSessionEvent) => void; timeoutMs?: number } = {},
) {
	const image = process.env.CORTEX_TEST_KERNEL_IMAGE;
	if (!image) throw new Error("Missing kernel image");
	const dir = options.dir ?? (await mkdtemp(join(tmpdir(), "cortex-recovery-")));
	const harness = await createHarness({ provider: "cortex-recovery-test", api: "cortex-recovery-test" });
	const scope: CortexScope = { role: "atlas", id, commitment: "a".repeat(64) };
	const make = () => {
		const sandbox = new CortexSandbox(join(dir, "sandbox"), {
			image,
			memoryMb: 512,
			workspaceMb: 64,
			cpus: 1,
			pids: 64,
			seconds: 60,
		});
		const journal = new FileBudgetJournal(
			join(dir, "budget.json"),
			cortexRuntimeBinding(scope, harness.getModel(), sandbox.limits),
		);
		try {
			const budget = new TreeBudget(
				{
					maxDepth: 2,
					maxChildren: 3,
					maxConcurrentCalls: 3,
					maxCalls: 20,
					maxReservedTokens: 10_000_000,
					maxReservedMicroUsd: 10_000_000,
					timeoutMs: options.timeoutMs ?? 60_000,
				},
				Date.now,
				journal,
			);
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
				record: options.record ?? (() => {}),
			});
			return { runtime, budget, sandbox };
		} catch (error) {
			journal.release();
			throw error;
		}
	};
	return {
		dir,
		harness,
		make,
		cleanup: async () => {
			harness.cleanup();
			if (!options.dir) await rm(dir, { recursive: true, force: true });
		},
	};
}
