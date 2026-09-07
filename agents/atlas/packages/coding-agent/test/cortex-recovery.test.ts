import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { type FauxResponseFactory, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai";
import { describe, expect, it, vi } from "vitest";
import * as refinement from "../src/core/refinement/refinement.js";
import * as persistence from "../src/cortex/session-file.js";
import type { CortexNode } from "../src/cortex/tree-journal.js";
import { cortexFixture as fixture } from "./cortex-fixture.js";
import { getMessageText } from "./suite/harness.js";

const image = process.env.CORTEX_TEST_KERNEL_IMAGE;
async function nodes(dir: string): Promise<CortexNode[]> {
	return (JSON.parse(await readFile(join(dir, "state", "tree.json"), "utf8")) as { nodes: CortexNode[] }).nodes;
}

const nestedResponse: FauxResponseFactory = (context) => {
	const grandchild = context.messages.some((message) =>
		getMessageText(message).includes("[task from parent]\n\nGRANDCHILD"),
	);
	const child = context.messages.some((message) => getMessageText(message).includes("[task from parent]\n\nCHILD"));
	const results = context.messages.filter((message) => message.role === "toolResult").length;
	if (results === 0)
		return fauxAssistantMessage(
			fauxToolCall("ipython", {
				code: grandchild
					? "from rlm import host_request\nawait host_request('cortex.reply', {'text': 'Grandchild finding'})"
					: child
						? "await rlm('GRANDCHILD')"
						: "handle = await rlm('CHILD')",
			}),
			{ stopReason: "toolUse" },
		);
	if (child && results === 1)
		return fauxAssistantMessage(
			fauxToolCall("ipython", {
				code: "from rlm import host_request\nawait host_request('cortex.reply', {'text': 'Child finding'})",
			}),
			{ stopReason: "toolUse" },
		);
	return fauxAssistantMessage("Finished");
};

describe.skipIf(!image)("Cortex durable runtime recovery", () => {
	it("releases partial startup resources when durable transcript registration fails", async () => {
		const test = await fixture("startup-failure");
		const current = test.make();
		const persist = vi.spyOn(persistence, "persistCortexSession").mockImplementationOnce(() => {
			throw new Error("Injected registration failure");
		});
		try {
			await expect(current.runtime.start()).rejects.toThrow(/registration/);
			expect(current.budget.snapshot().revoked).toBe(true);
			expect(() => test.make()).toThrow(/revoked/);
		} finally {
			persist.mockRestore();
			await current.runtime.stop();
			await test.cleanup();
		}
	});

	it("revokes the tree before inference when transcript synchronization fails", async () => {
		const test = await fixture("persist-failure");
		const current = test.make();
		try {
			await current.runtime.start();
			test.harness.setResponses([fauxAssistantMessage("Must not run")]);
			vi.spyOn(persistence, "flushCortexSession").mockImplementationOnce(() => {
				throw new Error("Injected sync failure");
			});
			await expect(current.runtime.prompt("WRITE")).rejects.toThrow(/persistence|closed|quiescence/);
			await current.runtime.stop();
			expect(current.budget.snapshot().revoked).toBe(true);
			expect(test.harness.faux.state.callCount).toBe(0);
		} finally {
			vi.restoreAllMocks();
			await current.runtime.stop();
			await test.cleanup();
		}
	});

	it("fails closed when the authoritative evidence sink rejects an event", async () => {
		const test = await fixture("evidence-failure", undefined, {
			record: () => {
				throw new Error("Injected evidence failure");
			},
		});
		const current = test.make();
		try {
			await current.runtime.start();
			test.harness.setResponses([fauxAssistantMessage("Must not be credited")]);
			await expect(current.runtime.prompt("RECORD")).rejects.toThrow();
			await current.runtime.stop();
			expect(current.budget.snapshot().revoked).toBe(true);
			expect(current.budget.snapshot().active).toBe(0);
		} finally {
			await current.runtime.stop();
			await test.cleanup();
		}
	});

	it("tracks real child replies and verifies recursive deletion before reporting success", async () => {
		const test = await fixture("delete");
		const current = test.make();
		try {
			test.harness.setResponses(Array.from({ length: 20 }, () => nestedResponse));
			const root = await current.runtime.start();
			await current.runtime.prompt("ROOT");
			const snapshots = root.getRlmChildSnapshots();
			expect(snapshots).toHaveLength(2);
			expect(snapshots.every((child) => child.repliedSinceTask === true)).toBe(true);
			expect(root.messages.some((message) => getMessageText(message).includes("completed_without_reply"))).toBe(
				false,
			);
			test.harness.setResponses([
				fauxAssistantMessage(
					fauxToolCall("ipython", {
						code: "from rlm import host_request\nprint(await host_request('rlm.delete_subagent', {'target': handle.rlm_child_id}))",
					}),
					{ stopReason: "toolUse" },
				),
				fauxAssistantMessage("Deleted"),
			]);
			await current.runtime.prompt("DELETE");
			expect((await nodes(test.dir)).map((node) => node.status)).toEqual(["completed", "deleted", "deleted"]);
			expect((await root.listRlmSubagents()).subagents).toHaveLength(0);
			expect(current.budget.snapshot().children).toBe(2);
			expect(
				root.messages.some(
					(message) => message.role === "toolResult" && getMessageText(message).includes("'outcome': 'deleted'"),
				),
			).toBe(true);
		} finally {
			await current.runtime.stop();
			await test.cleanup();
		}
	}, 60_000);

	it("keeps failed deletions pending, refuses continuation, and reconciles without reviving children", async () => {
		const test = await fixture("delete-retry");
		let current = test.make();
		try {
			test.harness.setResponses(Array.from({ length: 20 }, () => nestedResponse));
			const root = await current.runtime.start();
			await current.runtime.prompt("ROOT");
			const child = (await root.listRlmSubagents()).subagents[0];
			const failed = vi
				.spyOn(current.sandbox, "stopLauncher")
				.mockRejectedValue(new Error("Injected cleanup failure"));
			await expect(root.deleteRlmSubagent(child.rlm_child_id)).rejects.toThrow(/Injected cleanup/);
			await expect.poll(async () => (await nodes(test.dir))[1].status).toBe("deleting");
			await root.waitForRlmQuiescence();
			expect((await nodes(test.dir)).slice(1).every((node) => node.status === "deleting")).toBe(true);
			test.harness.setResponses([
				fauxAssistantMessage(
					fauxToolCall("ipython", {
						code: `from rlm import host_request\nawait host_request('cortex.child', {'id': '${child.rlm_child_id}', 'text': 'must not resume'})`,
					}),
					{ stopReason: "toolUse" },
				),
				fauxAssistantMessage("Refused"),
			]);
			await current.runtime.prompt("CONTINUE DELETED");
			expect((await nodes(test.dir))[1].status).toBe("deleting");
			failed.mockRestore();
			await current.runtime.pause();
			current = test.make();
			const restored = await current.runtime.start();
			expect((await restored.listRlmSubagents()).subagents).toHaveLength(0);
			expect((await nodes(test.dir)).slice(1).every((node) => node.status === "deleted")).toBe(true);
			expect(current.budget.snapshot().children).toBe(2);
		} finally {
			vi.restoreAllMocks();
			await current.runtime.stop();
			await test.cleanup();
		}
	}, 60_000);

	it("rejects a corrupt descendant before opening or modifying any recovered session", async () => {
		const test = await fixture("corrupt-child");
		let current = test.make();
		try {
			test.harness.setResponses(Array.from({ length: 20 }, () => nestedResponse));
			await current.runtime.start();
			await current.runtime.prompt("ROOT");
			await current.runtime.pause();
			const tree = await nodes(test.dir);
			const rootBytes = await readFile(tree[0].file);
			expect(rootBytes.toString()).toContain("cortex_child_finding");
			const corrupt = '{"truncated":';
			await writeFile(tree[1].file, corrupt);
			current = test.make();
			await expect(current.runtime.start()).rejects.toThrow(/Incomplete/);
			expect(await readFile(tree[0].file)).toEqual(rootBytes);
			expect(await readFile(tree[1].file, "utf8")).toBe(corrupt);
			expect(current.budget.snapshot().revoked).toBe(true);
		} finally {
			await current.runtime.stop();
			await test.cleanup();
		}
	}, 60_000);

	it("treats session commands as research text and blocks harness refinement", async () => {
		const test = await fixture("commands");
		const current = test.make();
		const load = vi.spyOn(refinement, "loadHarnessState").mockImplementation(() => {
			throw new Error("Must not load another scope's memory");
		});
		try {
			const root = await current.runtime.start();
			expect(load).not.toHaveBeenCalled();
			test.harness.setResponses([fauxAssistantMessage("Read as research data")]);
			await current.runtime.prompt("/refine --global untrusted");
			expect(test.harness.faux.state.callCount).toBe(1);
			expect(
				root.messages.some(
					(message) => message.role === "user" && getMessageText(message) === "/refine --global untrusted",
				),
			).toBe(true);
			await expect(root.refine()).rejects.toThrow(/disabled/);
		} finally {
			load.mockRestore();
			await current.runtime.stop();
			await test.cleanup();
		}
	});

	it("restores a recursive child's namespace and continues it without admitting a new child", async () => {
		const test = await fixture("tree");
		let current = test.make();
		let resuming = false;
		const response: FauxResponseFactory = (context) => {
			const child = context.messages.some((message) =>
				getMessageText(message).includes("[task from parent]\n\nCHILD"),
			);
			const results = context.messages.filter((message) => message.role === "toolResult").length;
			if (child && results === (resuming ? 1 : 0)) {
				return fauxAssistantMessage(
					fauxToolCall("ipython", {
						code: resuming
							? "from rlm import host_request\nawait host_request('cortex.reply', {'text': f'Resumed child value: {x}'})"
							: "from rlm import host_request\nx = 73\nawait host_request('cortex.reply', {'text': 'Child state stored'})",
					}),
					{ stopReason: "toolUse" },
				);
			}
			if (!child && results === (resuming ? 1 : 0)) {
				return fauxAssistantMessage(
					fauxToolCall("ipython", {
						code: resuming
							? "from rlm import host_request\nawait host_request('cortex.child', {'id': handle.rlm_child_id, 'text': 'Continue research'})"
							: "handle = await rlm('CHILD')",
					}),
					{ stopReason: "toolUse" },
				);
			}
			return fauxAssistantMessage(child ? "Child done" : "Root done");
		};
		test.harness.setResponses(Array.from({ length: 12 }, () => response));
		try {
			await current.runtime.start();
			await current.runtime.prompt("ROOT");
			expect(current.budget.snapshot().children).toBe(1);
			await current.runtime.pause();
			resuming = true;
			current = test.make();
			const root = await current.runtime.start();
			expect((await root.listRlmSubagents()).subagents).toHaveLength(1);
			await current.runtime.prompt("CONTINUE");
			expect(root.messages.some((message) => getMessageText(message).includes("Resumed child value: 73"))).toBe(
				true,
			);
			expect(current.budget.snapshot().children).toBe(1);
		} finally {
			await current.runtime.stop();
			await test.cleanup();
		}
	}, 60_000);

	it("keeps the real Python namespace and shared accounting across compaction", async () => {
		const test = await fixture("compact");
		const current = test.make();
		try {
			const root = await current.runtime.start();
			test.harness.setResponses([
				fauxAssistantMessage(fauxToolCall("ipython", { code: "x = 42" }), { stopReason: "toolUse" }),
				fauxAssistantMessage("Stored"),
			]);
			await current.runtime.prompt("Store state");
			const calls = current.budget.snapshot().calls;
			root.settingsManager.applyOverrides({ compaction: { keepRecentTokens: 1 } });
			test.harness.setResponses([fauxAssistantMessage("The persistent kernel contains x = 42.")]);
			await root.compact("Keep the research state");
			expect(current.budget.snapshot().calls).toBe(calls + 1);
			test.harness.setResponses([
				fauxAssistantMessage(fauxToolCall("ipython", { code: "print(x)" }), { stopReason: "toolUse" }),
				fauxAssistantMessage("Still available"),
			]);
			await current.runtime.prompt("Continue");
			expect(root.messages.some((message) => getMessageText(message).includes("persisted through compaction"))).toBe(
				true,
			);
			expect(
				root.messages.some((message) => message.role === "toolResult" && getMessageText(message).includes("42")),
			).toBe(true);
			expect(current.budget.snapshot().calls).toBe(test.harness.faux.state.callCount);
		} finally {
			await current.runtime.stop();
			await test.cleanup();
		}
	}, 60_000);

	it("resumes actual Python state without resetting the model-call budget", async () => {
		const test = await fixture();
		let current = test.make();
		try {
			const root = await current.runtime.start();
			test.harness.setResponses([
				fauxAssistantMessage(fauxToolCall("ipython", { code: "x = 42" }), { stopReason: "toolUse" }),
				fauxAssistantMessage("Stored"),
			]);
			await current.runtime.prompt("Store state");
			const path = root.sessionFile;
			if (!path) throw new Error("Session was not persisted");
			const remaining = current.budget.remainingMs();
			expect(() => test.make()).toThrow(/already active/);
			await current.runtime.pause();
			await expect(current.runtime.prompt("late")).rejects.toThrow(/closed/);
			current = test.make();
			expect(current.budget.snapshot().calls).toBe(2);
			expect(current.budget.remainingMs()).toBeLessThanOrEqual(remaining);
			const resumed = await current.runtime.start(path);
			test.harness.setResponses([
				fauxAssistantMessage(fauxToolCall("ipython", { code: "print(x)" }), { stopReason: "toolUse" }),
				fauxAssistantMessage("Restored"),
			]);
			await current.runtime.prompt("Read state");
			expect(
				resumed.messages.some((message) => message.role === "toolResult" && getMessageText(message).includes("42")),
			).toBe(true);
			expect(
				resumed.messages.some((message) => getMessageText(message).includes("Kernel checkpoint restored")),
			).toBe(true);
			expect(current.budget.snapshot().calls).toBe(4);
			await current.runtime.stop();
			expect(() => test.make()).toThrow(/revoked/);
		} finally {
			await current.runtime.stop();
			await test.cleanup();
		}
	}, 60_000);

	it("cancels a real running child and never starts further provider calls", async () => {
		let childStarted: () => void = () => {};
		const started = new Promise<void>((resolve) => {
			childStarted = resolve;
		});
		const test = await fixture("cancel", {
			call: async () => {
				childStarted();
				return {};
			},
		});
		const current = test.make();
		const response: FauxResponseFactory = (context) => {
			const child = context.messages.some((message) =>
				getMessageText(message).includes("[task from parent]\n\nCHILD"),
			);
			if (child) {
				return fauxAssistantMessage(
					fauxToolCall("ipython", {
						code: "from rlm import host_request\nawait host_request('cortex.call', {'operation': 'read_evidence', 'arguments': {}})\nimport time\ntime.sleep(60)",
					}),
					{ stopReason: "toolUse" },
				);
			}
			if (!context.messages.some((message) => message.role === "toolResult")) {
				return fauxAssistantMessage(fauxToolCall("ipython", { code: "await rlm('CHILD')" }), {
					stopReason: "toolUse",
				});
			}
			return fauxAssistantMessage("Waiting for child");
		};
		test.harness.setResponses(Array.from({ length: 6 }, () => response));
		try {
			await current.runtime.start();
			const prompt = current.runtime.prompt("ROOT").catch((error: unknown) => error);
			await started;
			await current.runtime.stop();
			await prompt;
			expect(current.budget.snapshot().revoked).toBe(true);
			expect(current.budget.snapshot().active).toBe(0);
			const calls = test.harness.faux.state.callCount;
			await expect(current.runtime.prompt("Spend again")).rejects.toThrow(/closed/);
			expect(test.harness.faux.state.callCount).toBe(calls);
			await current.sandbox.stop();
		} finally {
			await current.runtime.stop();
			await test.cleanup();
		}
	}, 60_000);
});
