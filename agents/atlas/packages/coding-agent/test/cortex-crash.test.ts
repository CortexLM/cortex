import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { readdir, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { type FauxResponseFactory, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";
import type { BudgetCheckpoint } from "../src/cortex/policy.js";
import { cortexFixture } from "./cortex-fixture.js";
import { getMessageText } from "./suite/harness.js";

const exec = promisify(execFile);
const packageRoot = fileURLToPath(new URL("..", import.meta.url));
const loader = fileURLToPath(new URL("../../../node_modules/tsx/dist/loader.mjs", import.meta.url));
const workerFile = fileURLToPath(new URL("./fixtures/cortex-crash-worker.ts", import.meta.url));

describe.skipIf(!process.env.CORTEX_TEST_KERNEL_IMAGE || process.platform !== "linux")(
	"Cortex process crash recovery",
	() => {
		it.each(["controller", "supervisor-and-controller"])(
			"recovers after SIGKILL of %s without new budgets or overlapping owners",
			async (mode) => {
				const test = await cortexFixture("crash");
				const worker = spawn(process.execPath, ["--import", loader, workerFile, test.dir], {
					cwd: packageRoot,
					env: {
						PATH: "/usr/bin:/bin",
						HOME: test.dir,
						CORTEX_TEST_KERNEL_IMAGE: process.env.CORTEX_TEST_KERNEL_IMAGE,
					},
					stdio: ["ignore", "pipe", "pipe"],
				});
				let output = "";
				worker.stdout.on("data", (chunk: Buffer) => {
					output += chunk.toString();
				});
				worker.stderr.on("data", (chunk: Buffer) => {
					output += chunk.toString();
				});
				const exited = once(worker, "exit");
				let current: ReturnType<typeof test.make> | undefined;
				let ready: { pid: number; supervisors: number[]; harnessDir: string } | undefined;
				try {
					await expect
						.poll(
							async () => {
								if (worker.exitCode !== null) throw new Error(`Crash fixture exited: ${output}`);
								const text = await readFile(join(test.dir, "ready.json"), "utf8").catch(() => "");
								if (!text) return false;
								ready = JSON.parse(text) as typeof ready;
								return true;
							},
							{ timeout: 20_000 },
						)
						.toBe(true);
					if (!ready) throw new Error("Missing crash fixture handshake");
					expect(ready.pid).toBe(worker.pid);
					expect(ready.supervisors).toHaveLength(2);
					const before = (
						JSON.parse(await readFile(join(test.dir, "budget.json"), "utf8")) as { state: BudgetCheckpoint }
					).state;
					expect(before.children).toBe(1);
					expect(() => test.make()).toThrow(/already active/);
					if (mode === "supervisor-and-controller") {
						for (const pid of ready.supervisors) process.kill(pid, "SIGKILL");
						expect(() => test.make()).toThrow(/already active/);
					}
					worker.kill("SIGKILL");
					await exited;
					current = test.make();
					const root = await current.runtime.start();
					const checkpoint = (
						JSON.parse(await readFile(join(test.dir, "budget.json"), "utf8")) as { state: BudgetCheckpoint }
					).state;
					expect(checkpoint.deadline).toBe(before.deadline);
					expect(checkpoint.calls).toBe(before.calls);
					expect(checkpoint.children).toBe(1);
					expect((await root.listRlmSubagents()).subagents[0].status).toBe("error");
					expect(root.getRlmChildSnapshots()[0].error).toMatch(/Interrupted/);
					expect(test.harness.faux.state.callCount).toBe(0);
					const response: FauxResponseFactory = (context) => {
						const child = context.messages.some((message) =>
							getMessageText(message).includes("[task from parent]\n\nCHILD"),
						);
						const resumed = context.messages.some(
							(message) =>
								message.role === "toolResult" && getMessageText(message).includes("Recovered value: 73"),
						);
						if (child && !resumed)
							return fauxAssistantMessage(
								fauxToolCall("ipython", {
									code: "from rlm import host_request\nawait host_request('cortex.reply', {'text': f'Recovered value: {x}'})\nprint(f'Recovered value: {x}')",
								}),
								{ stopReason: "toolUse" },
							);
						if (
							!child &&
							!context.messages.some((message) => getMessageText(message).includes("Recovered value: 73"))
						) {
							return fauxAssistantMessage(
								fauxToolCall("ipython", {
									code: "from rlm import host_request\nawait host_request('cortex.child', {'id': handle.rlm_child_id, 'text': 'RESUME'})",
								}),
								{ stopReason: "toolUse" },
							);
						}
						return fauxAssistantMessage("Recovered");
					};
					test.harness.setResponses(Array.from({ length: 10 }, () => response));
					await current.runtime.prompt("RESUME");
					expect(root.messages.some((message) => getMessageText(message).includes("Recovered value: 73"))).toBe(
						true,
					);
					expect(current.budget.snapshot().children).toBe(1);
					await current.runtime.stop();
					for (const file of await readdir(join(test.dir, "sandbox"))) {
						if (!file.endsWith(".json")) continue;
						const { name } = JSON.parse(await readFile(join(test.dir, "sandbox", file), "utf8")) as {
							name: string;
						};
						const remaining = await exec("/usr/bin/docker", [
							"ps",
							"-a",
							"--filter",
							`label=cortex.kernel=${name}`,
							"--format={{.ID}}",
						]);
						expect(remaining.stdout.trim()).toBe("");
					}
				} finally {
					if (worker.exitCode === null && worker.signalCode === null) worker.kill("SIGKILL");
					await exited;
					await current?.runtime.stop();
					if (ready) await rm(ready.harnessDir, { recursive: true, force: true });
					await test.cleanup();
				}
			},
			60_000,
		);
	},
);
