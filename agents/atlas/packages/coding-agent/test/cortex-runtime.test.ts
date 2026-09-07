import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import {
	completeSimple,
	type FauxResponseFactory,
	fauxAssistantMessage,
	fauxToolCall,
	streamSimple,
} from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";
import type { AgentSessionEvent } from "../src/core/agent-session.js";
import { ReplKernelManager } from "../src/core/kernel/index.js";
import { cortexRuntimeBinding } from "../src/cortex/binding.js";
import { FileBudgetJournal } from "../src/cortex/budget-journal.js";
import { createCortexInference } from "../src/cortex/inference.js";
import { type CortexScope, TreeBudget } from "../src/cortex/policy.js";
import { CortexRuntime } from "../src/cortex/runtime.js";
import { CortexSandbox } from "../src/cortex/sandbox.js";
import { createHarness, getMessageText } from "./suite/harness.js";

const limits = {
	maxDepth: 2,
	maxChildren: 3,
	maxConcurrentCalls: 3,
	maxCalls: 20,
	maxReservedTokens: 10_000_000,
	maxReservedMicroUsd: 10_000_000,
	timeoutMs: 60_000,
};
const image = process.env.CORTEX_TEST_KERNEL_IMAGE;
const exec = promisify(execFile);

describe("Cortex inference enforcement", () => {
	it("bounds direct completions used by compaction as well as agent calls", async () => {
		const harness = await createHarness();
		const budget = new TreeBudget({ ...limits, maxCalls: 1 });
		const inference = createCortexInference(
			harness.getModel(),
			harness.session.modelRegistry,
			budget,
			new AbortController().signal,
		);
		try {
			harness.setResponses([fauxAssistantMessage("summary"), fauxAssistantMessage("must not run")]);
			const result = await completeSimple(inference.model, { messages: [] });
			expect(getMessageText(result)).toBe("summary");
			await expect(completeSimple(inference.model, { messages: [] })).rejects.toThrow(/budget/);
			expect(harness.faux.state.callCount).toBe(1);
			expect(budget.snapshot().active).toBe(0);
		} finally {
			inference.dispose();
			harness.cleanup();
		}
	});

	it("does not forward provider error bodies in results or partial events", async () => {
		const harness = await createHarness();
		const budget = new TreeBudget(limits);
		const inference = createCortexInference(
			harness.getModel(),
			harness.session.modelRegistry,
			budget,
			new AbortController().signal,
		);
		try {
			harness.setResponses([
				fauxAssistantMessage([], {
					stopReason: "error",
					errorMessage: "Authorization: Bearer synthetic-secret-fixture",
				}),
			]);
			const response = streamSimple(inference.model, { messages: [] });
			const events = [];
			for await (const event of response) events.push(event);
			const result = await response.result();
			expect(result.stopReason).toBe("error");
			expect(result.errorMessage).toBe("Cortex inference failed or was cancelled");
			expect(JSON.stringify({ events, result })).not.toContain("synthetic-secret-fixture");
			expect(budget.snapshot().active).toBe(0);
		} finally {
			inference.dispose();
			harness.cleanup();
		}
	});
});

describe.skipIf(!image)("Cortex real isolated Python runtime", () => {
	it("enforces the supervisor deadline and removes the kernel with its subprocesses", async () => {
		if (!image) throw new Error("Missing kernel image");
		const dir = await mkdtemp(join(tmpdir(), "cortex-deadline-"));
		const work = join(dir, "work");
		await mkdir(work);
		const sandbox = new CortexSandbox(join(dir, "sandbox"), {
			image,
			memoryMb: 512,
			workspaceMb: 64,
			cpus: 1,
			pids: 64,
			seconds: 2,
		});
		const launcher = await sandbox.prepare(work);
		const config = JSON.parse(await readFile(launcher.replace(/\.sh$/, ".json"), "utf8")) as { name: string };
		const kernel = new ReplKernelManager({ python: launcher, inheritEnv: false, cwd: work });
		try {
			await kernel.start();
			await expect(
				kernel.execute("import subprocess, time\nchild = subprocess.Popen(['sleep', '60'])\ntime.sleep(60)"),
			).rejects.toThrow(/shut down|exited/);
			expect(kernel.isRunning).toBe(false);
			const containers = await exec("/usr/bin/docker", [
				"ps",
				"--all",
				"--filter",
				`label=cortex.kernel=${config.name}`,
				"--format={{.ID}}",
			]);
			expect(containers.stdout.trim()).toBe("");
		} finally {
			await kernel.shutdown();
			await sandbox.stop();
			await rm(dir, { recursive: true, force: true });
		}
	}, 20_000);

	it("runs persistent Python and a real recursive child with shared accounting", async () => {
		if (!image) throw new Error("Set CORTEX_TEST_KERNEL_IMAGE to the built image digest");
		const harness = await createHarness();
		const dir = await mkdtemp(join(tmpdir(), "cortex-rlm-"));
		const events: { sessionId: string; event: AgentSessionEvent }[] = [];
		const brokerCalls: string[] = [];
		const sandbox = new CortexSandbox(join(dir, "sandbox"), {
			image,
			memoryMb: 512,
			workspaceMb: 64,
			cpus: 1,
			pids: 64,
			seconds: 60,
		});
		const scope: CortexScope = { role: "atlas", id: "round1", commitment: "a".repeat(64) };
		const budget = new TreeBudget(
			limits,
			Date.now,
			new FileBudgetJournal(
				join(dir, "budget.json"),
				cortexRuntimeBinding(scope, harness.getModel(), sandbox.limits),
			),
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
			broker: {
				call: async (_scope, operation) => {
					brokerCalls.push(operation);
					return { evidence: "controller-retained" };
				},
			},
			record: (sessionId, event) => events.push({ sessionId, event }),
		});
		const response: FauxResponseFactory = (context) => {
			const child = context.messages.some(
				(message) => message.role === "user" && getMessageText(message).includes("[task from parent]\n\nCHILD"),
			);
			const results = context.messages.filter((message) => message.role === "toolResult").length;
			if (child && results === 0) {
				return fauxAssistantMessage(
					fauxToolCall("ipython", {
						code: "from rlm import host_request\nanswer = 6 * 7\nawait host_request('cortex.reply', {'text': str(answer)})",
					}),
					{ stopReason: "toolUse" },
				);
			}
			if (!child && results === 0) {
				return fauxAssistantMessage(
					fauxToolCall("ipython", {
						code: "x = 40\nhandle = await rlm('CHILD', name='research-child')\nprint(handle.rlm_child_id)",
					}),
					{ stopReason: "toolUse" },
				);
			}
			if (!child && results === 1) {
				return fauxAssistantMessage(
					fauxToolCall("ipython", {
						code: "from rlm import host_request\nprint(x + 2)\nawait host_request('cortex.call', {'operation': 'read_evidence', 'arguments': {}})",
					}),
					{ stopReason: "toolUse" },
				);
			}
			return fauxAssistantMessage(child ? "Child complete" : "Root complete");
		};
		harness.setResponses(Array.from({ length: 15 }, () => response));
		try {
			const root = await runtime.start();
			await runtime.prompt("ROOT");
			await expect
				.poll(async () => (await root.listRlmSubagents()).subagents[0]?.status, { timeout: 15_000 })
				.toBe("completed");
			expect(budget.snapshot().children).toBe(1);
			expect(new Set(events.map((entry) => entry.sessionId)).size).toBe(2);
			expect(brokerCalls).toEqual(["read_evidence"]);
			expect(
				root.messages.filter((m) => m.role === "toolResult").some((m) => getMessageText(m).includes("42")),
			).toBe(true);
			expect(budget.snapshot().calls).toBe(harness.faux.state.callCount);
			expect(
				root.messages.some((message) => getMessageText(message).includes("Untrusted research finding from child")),
			).toBe(true);
		} finally {
			await runtime.stop();
			harness.cleanup();
			await rm(dir, { recursive: true, force: true });
		}
	}, 60_000);

	it("excludes controller files, credentials, network and sibling workspaces; restores opaque snapshots", async () => {
		if (!image) throw new Error("Missing kernel image");
		const dir = await mkdtemp(join(tmpdir(), "cortex-isolation-"));
		const work = join(dir, "workspace");
		await mkdir(work);
		const sentinel = join(dir, "controller-only");
		await writeFile(sentinel, "private-fixture");
		const sandbox = new CortexSandbox(join(dir, "sandbox"), {
			image,
			memoryMb: 512,
			workspaceMb: 64,
			cpus: 1,
			pids: 64,
			seconds: 60,
		});
		const make = async () =>
			new ReplKernelManager({
				python: await sandbox.prepare(work),
				inheritEnv: false,
				cwd: work,
				env: { CORTEX_TEST_SECRET: "not-for-kernel" },
				snapshot: { path: "/work/state.dill", manifestPath: "/work/state.json" },
			});
		const writer = await make();
		try {
			const result = await writer.execute(`
import os, pathlib, socket
assert os.getuid() == 65532
assert 'CORTEX_TEST_SECRET' not in os.environ
assert not pathlib.Path(${JSON.stringify(sentinel)}).exists()
assert not pathlib.Path('/var/run/docker.sock').exists()
assert set(os.listdir('/proc/1/root/root')) == set() if os.access('/proc/1/root/root', os.R_OK) else True
try:
    socket.create_connection(('192.0.2.1', 80), timeout=0.2)
    raise AssertionError('network escaped')
except OSError:
    pass
x = 42
print('isolated')
`);
			expect(result.status, result.stderr).toBe("ok");
			expect(result.stdout).toContain("isolated");
			expect((await writer.snapshotState())?.saved).toContain("x");
			await writer.shutdown({ snapshot: true });
			const reader = await make();
			try {
				expect((await reader.restoreState())?.restored).toContain("x");
				expect((await reader.execute("print(x)")).stdout.trim()).toBe("42");
			} finally {
				await reader.shutdown({ snapshot: true });
			}
		} finally {
			await writer.shutdown();
			await sandbox.stop();
			await rm(dir, { recursive: true, force: true });
		}
	}, 60_000);
});
