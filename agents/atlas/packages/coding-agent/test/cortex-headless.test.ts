import { type ChildProcess, execFile, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { once } from "node:events";
import { chmod, link, mkdir, mkdtemp, readdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { describe, expect, it } from "vitest";
import type { HeadlessEvent } from "../src/cortex/headless.js";
import {
	type CortexLaunchConfig,
	type CortexModelConfig,
	loadHeadlessModel,
	MAX_LAUNCH_BYTES,
	parseCortexLaunch,
	parseCortexModel,
	prepareHeadlessPaths,
	readHeadlessPrivateFile,
} from "../src/cortex/headless-config.js";
import { type BudgetCheckpoint, type BudgetJournal, TreeBudget } from "../src/cortex/policy.js";
import { CortexSandbox } from "../src/cortex/sandbox.js";

const exec = promisify(execFile);
const packageRoot = fileURLToPath(new URL("..", import.meta.url));
const loader = fileURLToPath(new URL("../../../node_modules/tsx/dist/loader.mjs", import.meta.url));
const cli = fileURLToPath(new URL("../src/cortex/headless-cli.ts", import.meta.url));
const faux = fileURLToPath(new URL("./fixtures/cortex-headless-provider.ts", import.meta.url));
const transport = fileURLToPath(new URL("./fixtures/cortex-headless-openai-transport.ts", import.meta.url));
const image = process.env.CORTEX_TEST_KERNEL_IMAGE;
const limits = {
	maxDepth: 2,
	maxChildren: 3,
	maxConcurrentCalls: 3,
	maxCalls: 20,
	maxReservedTokens: 10_000_000,
	maxReservedMicroUsd: 10_000_000,
	timeoutMs: 60_000,
};

async function fixture(role: "experiment" | "atlas" = "atlas") {
	const dir = await mkdtemp("/tmp/cortex-headless-");
	const launch: CortexLaunchConfig = {
		schema_version: 1,
		runtime_id: randomUUID(),
		deadline_ms: Date.now() + limits.timeoutMs,
		resume: false,
		scope: { role, id: "headless-round", commitment: "a".repeat(64) },
		controller_socket: join(dir, "controller.sock"),
		state_dir: join(dir, "state"),
		workspace: join(dir, "workspace"),
		sandbox_dir: join(dir, "sandbox"),
		model_config_file: join(dir, "model.json"),
		kernel: {
			image: image ?? `sha256:${"b".repeat(64)}`,
			memoryMb: 512,
			workspaceMb: 64,
			cpus: 1,
			pids: 64,
			seconds: 60,
		},
		budget: { ...limits },
		prompt: "PRIVATE ROOT PROMPT",
	};
	const model: CortexModelConfig = {
		schema_version: 1,
		provider: "headless-fixture",
		model: "headless-1",
		api: "openai-responses",
		baseUrl: "https://provider.invalid/v1",
		apiKeyFile: join(dir, "model-key"),
		reasoning: false,
		contextWindow: 128000,
		maxTokens: 4096,
		cost: { input: 1, output: 2, cacheRead: 0.1, cacheWrite: 1 },
	};
	await writeFile(model.apiKeyFile, "synthetic-headless-key\n", { mode: 0o600 });
	await writeFile(launch.model_config_file, JSON.stringify(model), { mode: 0o600 });
	const launchFile = join(dir, "launch.json");
	const save = () => writeFile(launchFile, JSON.stringify(launch), { mode: 0o600 });
	await save();
	const calls: unknown[] = [];
	const server = createServer(async (request, response) => {
		const chunks: Buffer[] = [];
		for await (const chunk of request) chunks.push(Buffer.from(chunk));
		calls.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));
		response.setHeader("content-type", "application/json");
		response.end(JSON.stringify({ schema_version: 1, result: { evidence: "controller-only" } }));
	});
	server.listen(launch.controller_socket);
	await once(server, "listening");
	await chmod(launch.controller_socket, 0o600);
	const workers = new Set<ChildProcess>();
	const start = (mode = "text", args = ["--launch", launchFile], preload: boolean | string = true) => {
		const child = spawn(
			process.execPath,
			[
				"--import",
				loader,
				...(preload ? ["--import", typeof preload === "string" ? preload : faux] : []),
				cli,
				...args,
			],
			{
				cwd: packageRoot,
				env: {
					PATH: "/usr/bin:/bin",
					HOME: dir,
					DO_NOT_TRACK: "1",
					PI_OFFLINE: "1",
					CORTEX_HEADLESS_TEST_MODE: mode,
					CORTEX_HEADLESS_TEST_REQUESTS: join(dir, "requests.jsonl"),
					OPENAI_API_KEY: "wrong-synthetic-environment-key",
				},
				stdio: ["pipe", "pipe", "pipe"],
			},
		);
		workers.add(child);
		let stdout = "";
		let stderr = "";
		child.stdout.on("data", (chunk: Buffer) => {
			stdout += chunk.toString();
		});
		child.stderr.on("data", (chunk: Buffer) => {
			stderr += chunk.toString();
		});
		const exit = once(child, "close").then(([code, signal]) => {
			workers.delete(child);
			return { code, signal, stdout, stderr };
		});
		return { child, exit, output: () => stdout };
	};
	const checkpoint = async () =>
		(JSON.parse(await readFile(join(launch.state_dir, "budget.json"), "utf8")) as { state: BudgetCheckpoint }).state;
	const noKernels = async () => {
		for (const file of await readdir(launch.sandbox_dir)) {
			if (!file.endsWith(".json")) continue;
			const { name } = JSON.parse(await readFile(join(launch.sandbox_dir, file), "utf8")) as { name: string };
			const remaining = await exec("/usr/bin/docker", [
				"ps",
				"-a",
				"--filter",
				`label=cortex.kernel=${name}`,
				"--format={{.ID}}",
			]);
			expect(remaining.stdout.trim()).toBe("");
		}
	};
	return {
		dir,
		launch,
		model,
		launchFile,
		save,
		start,
		checkpoint,
		calls,
		noKernels,
		rebind: async () => {
			server.closeAllConnections();
			await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
			const attempt = join(dir, "attempt-2");
			await mkdir(attempt, { mode: 0o700 });
			launch.controller_socket = join(attempt, "controller.sock");
			server.listen(launch.controller_socket);
			await once(server, "listening");
			await chmod(launch.controller_socket, 0o600);
			await save();
		},
		cleanup: async () => {
			await Promise.all(
				[...workers].map(async (worker) => {
					const closed = once(worker, "close");
					worker.kill("SIGTERM");
					await closed;
				}),
			);
			server.closeAllConnections();
			await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
			await rm(dir, { recursive: true, force: true });
		},
	};
}

function events(result: { stdout: string; stderr: string }): HeadlessEvent[] {
	expect(result.stderr).toBe("");
	expect(result.stdout).not.toMatch(/synthetic|PRIVATE ROOT PROMPT|provider.invalid|Authorization/);
	const lines = result.stdout.trim().split("\n");
	expect(lines.length).toBeLessThanOrEqual(2);
	return lines.map((line) => {
		expect(line.length).toBeLessThan(512);
		return JSON.parse(line) as HeadlessEvent;
	});
}

describe("Cortex headless configuration", () => {
	it("loads an explicit private model without credentials in its model descriptor", async () => {
		const test = await fixture();
		try {
			expect(parseCortexLaunch(test.launch)).toEqual(test.launch);
			await prepareHeadlessPaths(test.launch);
			const loaded = await loadHeadlessModel(test.launch);
			expect(loaded.apiKey).toBe("synthetic-headless-key");
			expect(loaded.model).toMatchObject({ provider: test.model.provider, id: test.model.model, input: ["text"] });
			expect(JSON.stringify(loaded.model)).not.toContain("synthetic-headless-key");
		} finally {
			await test.cleanup();
		}
	});

	it("rejects unknown fields, unbounded budgets, noncanonical paths and overlapping private directories", async () => {
		const test = await fixture();
		try {
			for (const change of [
				{ apiKey: "synthetic-secret" },
				{ schema_version: 2 },
				{ runtime_id: "not-a-uuid" },
				{ runtime_id: undefined },
				{ deadline_ms: undefined },
				{ deadline_ms: 0 },
				{ deadline_ms: 12.5 },
				{ resume: undefined },
				{ resume: "true" },
				{ scope: { ...test.launch.scope, role: "validator" } },
				{ budget: { ...limits, timeoutMs: 86_400_001 } },
				{ budget: { ...limits, maxCalls: 0 } },
				{ kernel: { ...test.launch.kernel, image: "floating:latest" } },
				{ kernel: { ...test.launch.kernel, pids: 1025 } },
				{ workspace: "relative" },
				{ workspace: `${test.dir}/../escape` },
				{ workspace: test.dir },
				{ sandbox_dir: join(test.launch.workspace, "nested") },
				{ controller_socket: join(test.launch.workspace, "socket") },
				{ model_config_file: join(test.launch.state_dir, "model.json") },
				{ prompt: "x".repeat(128 * 1024 + 1) },
			])
				expect(() => parseCortexLaunch({ ...test.launch, ...change })).toThrow("invalid_launch");
			for (const change of [
				{ apiKey: "synthetic-secret" },
				{ api: "bedrock-converse-stream" },
				{ baseUrl: "http://provider.invalid/v1" },
				{ baseUrl: "https://user:synthetic-secret@provider.invalid/v1" },
				{ baseUrl: "https://provider.invalid/v1?key=synthetic-secret" },
				{ cost: { ...test.model.cost, output: -1 } },
				{ maxTokens: Number.MAX_SAFE_INTEGER },
				{ contextWindow: 0 },
				{ apiKeyFile: "!cat /private/key" },
			])
				expect(() => parseCortexModel({ ...test.model, ...change })).toThrow("model_config_unavailable");
		} finally {
			await test.cleanup();
		}
	});

	it("allows only explicitly opted-in literal loopback HTTP endpoints", async () => {
		const test = await fixture();
		try {
			for (const baseUrl of ["http://127.0.0.1:8080/v1", "http://[::1]:8080/v1"]) {
				expect(() => parseCortexModel({ ...test.model, baseUrl })).toThrow("model_config_unavailable");
				expect(parseCortexModel({ ...test.model, baseUrl, allowLoopbackHttp: true })).toMatchObject({
					baseUrl,
					allowLoopbackHttp: true,
				});
			}
			for (const baseUrl of [
				"http://localhost:8080/v1",
				"http://127.1:8080/v1",
				"http://2130706433:8080/v1",
				"http://0177.0.0.1:8080/v1",
				"http://127.0.0.2:8080/v1",
				"http://127.0.0.1.example.test/v1",
				"http://[0:0:0:0:0:0:0:1]:8080/v1",
				"http://[::ffff:127.0.0.1]:8080/v1",
				"http://192.0.2.1/v1",
				"http://user:synthetic-secret@127.0.0.1/v1",
				"http://127.0.0.1/v1?key=synthetic-secret",
				"http://[::1]/v1#fragment",
				"http://127.0.0.1/v1?",
			])
				expect(() => parseCortexModel({ ...test.model, baseUrl, allowLoopbackHttp: true })).toThrow(
					"model_config_unavailable",
				);
			for (const flag of [false, "true", 1, null])
				expect(() => parseCortexModel({ ...test.model, allowLoopbackHttp: flag })).toThrow(
					"model_config_unavailable",
				);
			expect(parseCortexModel(test.model)).toEqual(test.model);
		} finally {
			await test.cleanup();
		}
	});

	it("pins an external absolute budget deadline rather than restarting the timeout", () => {
		let now = 1000;
		let saved: BudgetCheckpoint | undefined;
		const journal: BudgetJournal = {
			binding: "a".repeat(64),
			assertActive: () => {},
			release: () => {},
			load: () => saved,
			save: (state) => {
				saved = state;
			},
		};
		const budget = new TreeBudget(limits, () => now, journal, 4000);
		expect(budget.remainingMs()).toBe(3000);
		now = 2000;
		expect(new TreeBudget(limits, () => now, journal, 4000).remainingMs()).toBe(2000);
		expect(() => new TreeBudget(limits, () => now, journal, 3999)).toThrow(/checkpoint/);
		expect(() => new TreeBudget(limits, () => now, journal, 4001)).toThrow(/checkpoint/);
		now = 3999;
		expect(budget.remainingMs()).toBe(1);
		now = 4000;
		expect(() => budget.assertActive()).toThrow(/expired/);
		expect(() => new TreeBudget(limits, () => now, undefined, now + limits.timeoutMs + 1)).toThrow(/deadline/);
	});

	it("refuses delayed kernel launchers after the original absolute deadline without starting Docker", async () => {
		const test = await fixture();
		try {
			await prepareHeadlessPaths(test.launch);
			const deadline = Date.now() + 1000;
			const sandbox = new CortexSandbox(test.launch.sandbox_dir, test.launch.kernel, deadline);
			const launcher = await sandbox.prepare(test.launch.workspace);
			await delay(Math.max(0, deadline - Date.now()) + 25);
			await expect(
				exec(launcher, ["-m", "rlm.repl"], {
					env: { PATH: "/usr/bin:/bin", HOME: "/nonexistent" },
					timeout: 5000,
				}),
			).rejects.toMatchObject({ code: 1, stdout: "", stderr: "Cortex kernel supervisor failed\n" });
			await expect(sandbox.prepare(test.launch.workspace)).rejects.toThrow(/expired/);
		} finally {
			await test.cleanup();
		}
	});

	it.each(["absent", "empty", "partial"])(
		"refuses initial resume with %s checkpoints without minting a budget",
		async (mode) => {
			const test = await fixture();
			try {
				if (mode !== "absent") await prepareHeadlessPaths(test.launch);
				if (mode === "partial")
					await writeFile(join(test.launch.state_dir, "headless.json"), '{"schema_version":1}', { mode: 0o600 });
				test.launch.resume = true;
				await test.save();
				const result = await test.start().exit;
				expect(result.code).toBe(1);
				expect(events(result)).toMatchObject([
					{ event: "failed", code: "reattachment_failed", counts: { calls: 0 } },
				]);
				expect(await readdir(test.launch.state_dir)).not.toContain("budget.json");
				expect(await readdir(test.launch.state_dir)).not.toContain("tree.json");
				expect(await readdir(test.launch.sandbox_dir)).toEqual([]);
			} finally {
				await test.cleanup();
			}
		},
	);

	it("fails closed on readable-by-others, symlinked, hardlinked, missing and oversized configuration", async () => {
		const test = await fixture();
		try {
			await chmod(test.model.apiKeyFile, 0o644);
			await expect(loadHeadlessModel(test.launch)).rejects.toThrow("model_config_unavailable");
			await chmod(test.model.apiKeyFile, 0o600);
			await link(test.model.apiKeyFile, join(test.dir, "key-copy"));
			await expect(loadHeadlessModel(test.launch)).rejects.toThrow("model_config_unavailable");
			await rm(join(test.dir, "key-copy"));
			await symlink(test.model.apiKeyFile, join(test.dir, "key-link"));
			await writeFile(
				test.launch.model_config_file,
				JSON.stringify({ ...test.model, apiKeyFile: join(test.dir, "key-link") }),
			);
			await expect(loadHeadlessModel(test.launch)).rejects.toThrow("model_config_unavailable");
			await writeFile(test.launch.model_config_file, "synthetic-secret { malformed");
			await expect(loadHeadlessModel(test.launch)).rejects.toThrow("model_config_unavailable");
			await writeFile(test.launch.model_config_file, JSON.stringify(test.model));
			await writeFile(test.model.apiKeyFile, "a".repeat(16 * 1024 + 1));
			await expect(loadHeadlessModel(test.launch)).rejects.toThrow("model_config_unavailable");
			await rm(test.model.apiKeyFile);
			await expect(loadHeadlessModel(test.launch)).rejects.toThrow("model_config_unavailable");
			await chmod(test.dir, 0o755);
			await expect(readHeadlessPrivateFile(test.launch.model_config_file, 64 * 1024)).rejects.toThrow();
		} finally {
			await test.cleanup();
		}
	});

	it("rejects symlinked runtime paths and public controller sockets", async () => {
		const test = await fixture();
		try {
			await mkdir(join(test.dir, "other"), { mode: 0o700 });
			await symlink(join(test.dir, "other"), test.launch.state_dir);
			await expect(prepareHeadlessPaths(test.launch)).rejects.toThrow("unsafe_paths");
			await rm(test.launch.state_dir);
			await chmod(test.launch.controller_socket, 0o666);
			await expect(prepareHeadlessPaths(test.launch)).rejects.toThrow("unsafe_paths");
		} finally {
			await test.cleanup();
		}
	});

	it("exits nonzero without inference when the key is unavailable despite ambient credentials", async () => {
		const test = await fixture();
		try {
			await rm(test.model.apiKeyFile);
			const result = await test.start().exit;
			expect(result.code).toBe(1);
			expect(events(result)).toMatchObject([
				{ event: "failed", code: "model_config_unavailable", counts: { calls: 0 } },
			]);
			expect(await readdir(test.launch.state_dir)).toEqual([]);
		} finally {
			await test.cleanup();
		}
	});

	it("accepts only bounded JSON-file/stdin CLI inputs and redacts malformed input", async () => {
		const test = await fixture();
		try {
			for (const args of [["--api-key", "synthetic-headless-key"], ["--stdin"], ["--launch", test.launchFile]]) {
				await writeFile(test.launchFile, "synthetic-headless-key {");
				const run = test.start("text", args, false);
				run.child.stdin.end("synthetic-headless-key {");
				const result = await run.exit;
				expect(result.code).toBe(1);
				expect(events(result)[0]).toMatchObject({ event: "failed", code: "invalid_launch" });
			}
			const run = test.start("text", ["--stdin"], false);
			run.child.stdin.on("error", () => {});
			run.child.stdin.end("x".repeat(MAX_LAUNCH_BYTES + 1));
			const result = await run.exit;
			expect(result.code).toBe(1);
			expect(events(result)[0]).toMatchObject({ event: "failed", code: "invalid_launch" });
		} finally {
			await test.cleanup();
		}
	}, 60_000);
});

describe.skipIf(!image)("Cortex headless faux-provider isolated launches", () => {
	it.each([
		{ mode: "transport-text", reasoning: true, failed: false },
		{ mode: "transport-text-no-reasoning", reasoning: false, failed: false },
		{ mode: "transport-reject", reasoning: true, failed: true },
	])(
		"uses cold built-in Responses, pinned auth and small context with $mode",
		async ({ mode, reasoning, failed }) => {
			const test = await fixture();
			try {
				Object.assign(test.model, {
					provider: "openai",
					model: "cx/gpt-6-astra",
					baseUrl: "http://127.0.0.1:9/v1",
					allowLoopbackHttp: true,
					reasoning,
					contextWindow: 16384,
					maxTokens: 2048,
					cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				});
				test.launch.budget = { ...limits, maxCalls: 4, maxReservedTokens: 100_000, timeoutMs: 120_000 };
				test.launch.deadline_ms = Date.now() + test.launch.budget.timeoutMs;
				await writeFile(test.launch.model_config_file, JSON.stringify(test.model));
				await test.save();
				const result = await test.start(mode, ["--launch", test.launchFile], transport).exit;
				expect(events(result)).toMatchObject([
					{ event: "started", restored: false },
					{
						...(failed ? { event: "failed", code: "model_failed" } : { event: "stopped", reason: "completed" }),
						counts: { calls: 1, reservedTokens: 18432, reservedMicroUsd: 0, children: 0 },
					},
				]);
				expect(result.code).toBe(failed ? 1 : 0);
				const requests = (await readFile(join(test.dir, "requests.jsonl"), "utf8"))
					.trim()
					.split("\n")
					.map((line) => JSON.parse(line) as Record<string, unknown>);
				expect(requests).toEqual([
					{
						model: "cx/gpt-6-astra",
						...(reasoning
							? { reasoning: { effort: "medium", summary: "auto" }, include: ["reasoning.encrypted_content"] }
							: {}),
						max_output_tokens: 2048,
						store: false,
						roles: [reasoning ? "developer" : "system", "user"],
						toolNames: ["ipython"],
					},
				]);
				expect((await test.checkpoint()).revoked).toBe(true);
				await test.noKernels();
			} finally {
				await test.cleanup();
			}
		},
		60_000,
	);

	it.each(["budget.json", "tree.json", "headless.json"])("refuses a restart missing %s", async (file) => {
		const test = await fixture();
		try {
			expect((await test.start().exit).code).toBe(0);
			await rm(join(test.launch.state_dir, file));
			test.launch.resume = true;
			await test.save();
			const result = await test.start().exit;
			expect(result.code).toBe(1);
			expect(events(result)).toMatchObject([{ event: "failed", code: "reattachment_failed" }]);
			expect(await readdir(test.launch.state_dir)).not.toContain(file);
			await test.noKernels();
		} finally {
			await test.cleanup();
		}
	});

	it.each(["experiment", "atlas"] as const)(
		"launches the complete %s runtime and recursive child through private controller IPC",
		async (role) => {
			const test = await fixture(role);
			try {
				const run = test.start("recursive", ["--stdin"]);
				run.child.stdin.end(JSON.stringify(test.launch));
				const result = await run.exit;
				expect(events(result)).toMatchObject([
					{ event: "started", restored: false },
					{ event: "stopped", reason: "completed", counts: { children: 1 } },
				]);
				expect(result.code).toBe(0);
				expect(test.calls).toEqual([
					{ schema_version: 1, scope: test.launch.scope, operation: "read_evidence", arguments: {} },
				]);
				const checkpoint = await test.checkpoint();
				expect(checkpoint.deadline).toBe(test.launch.deadline_ms);
				expect(checkpoint.calls).toBeGreaterThanOrEqual(4);
				expect(checkpoint.revoked).toBe(true);
				expect(checkpoint.children).toBe(1);
				await test.noKernels();
				// resume=false must reject even a complete, existing run.
				const replay = await test.start().exit;
				expect(replay.code).toBe(1);
				expect(events(replay).at(-1)?.event).toBe("failed");
				expect((await test.checkpoint()).deadline).toBe(checkpoint.deadline);
				expect((await test.checkpoint()).calls).toBe(checkpoint.calls);
				test.launch.resume = true;
				await test.save();
				const revoked = await test.start().exit;
				expect(revoked.code).toBe(1);
				expect(events(revoked).at(-1)?.event).toBe("failed");
				expect((await test.checkpoint()).deadline).toBe(checkpoint.deadline);
				expect((await test.checkpoint()).calls).toBe(checkpoint.calls);
			} finally {
				await test.cleanup();
			}
		},
		60_000,
	);

	it("reports model failures without provider diagnostics, output, environment fallback or success", async () => {
		const test = await fixture();
		try {
			const result = await test.start("failure").exit;
			expect(result.code).toBe(1);
			expect(events(result)).toMatchObject([
				{ event: "started" },
				{ event: "failed", code: "model_failed", counts: { calls: 1 } },
			]);
			expect((await test.checkpoint()).revoked).toBe(true);
			await test.noKernels();
		} finally {
			await test.cleanup();
		}
	});

	it.each(["SIGTERM", "SIGINT"] as const)(
		"revokes the durable tree and removes descendants on %s",
		async (signal) => {
			const test = await fixture();
			const run = test.start("wait");
			try {
				await expect.poll(() => test.calls.length, { timeout: 20_000 }).toBe(1);
				run.child.kill(signal);
				const result = await run.exit;
				expect(result.code).toBe(signal === "SIGTERM" ? 143 : 130);
				expect(events(result).at(-1)).toMatchObject({ event: "stopped", reason: "cancelled" });
				expect((await test.checkpoint()).revoked).toBe(true);
				await test.noKernels();
				const before = await test.checkpoint();
				test.launch.resume = true;
				await test.save();
				expect((await test.start("resume").exit).code).toBe(1);
				expect(await test.checkpoint()).toEqual(before);
			} finally {
				if (run.child.exitCode === null && run.child.signalCode === null) run.child.kill("SIGTERM");
				await run.exit;
				await test.cleanup();
			}
		},
		60_000,
	);

	it("enforces the original deadline and joins descendant cleanup before reporting failure", async () => {
		const test = await fixture();
		try {
			test.launch.deadline_ms = Date.now() + 3000;
			await test.save();
			const result = await test.start("wait").exit;
			expect(result.code).toBe(1);
			expect(events(result).at(-1)).toMatchObject({ event: "failed", code: "deadline_exceeded" });
			expect(await test.checkpoint()).toMatchObject({ revoked: true, deadline: test.launch.deadline_ms });
			for (const file of await readdir(test.launch.sandbox_dir)) {
				if (!file.endsWith(".json")) continue;
				const config = JSON.parse(await readFile(join(test.launch.sandbox_dir, file), "utf8")) as {
					deadline_ms: number;
					seconds: number;
				};
				expect(config.deadline_ms).toBe(test.launch.deadline_ms);
				expect(config.seconds).toBeLessThanOrEqual(3);
			}
			await test.noKernels();
		} finally {
			await test.cleanup();
		}
	}, 20_000);

	it("fences concurrent workers and reattaches after SIGKILL with original budget and deadline", async () => {
		const test = await fixture();
		const run = test.start("wait");
		try {
			await expect.poll(() => test.calls.length, { timeout: 20_000 }).toBe(1);
			const before = await test.checkpoint();
			expect(before.deadline).toBe(test.launch.deadline_ms);
			test.launch.resume = true;
			await test.save();
			const concurrent = await test.start().exit;
			expect(concurrent.code).toBe(1);
			expect(events(concurrent).at(-1)).toMatchObject({ event: "failed", code: "reattachment_failed" });
			expect((await test.checkpoint()).revoked).toBe(false);
			run.child.kill("SIGKILL");
			await run.exit;
			const original = structuredClone(test.launch);
			for (const change of [
				{ prompt: "Changed prompt must not authorize a new budget" },
				{ runtime_id: randomUUID() },
				{ deadline_ms: original.deadline_ms + 1000 },
				{ deadline_ms: original.deadline_ms - 1000 },
				{ scope: { ...original.scope, id: "different-scope" } },
			]) {
				Object.assign(test.launch, original, change);
				await test.save();
				const changed = await test.start().exit;
				expect(changed.code).toBe(1);
				expect(events(changed).at(-1)).toMatchObject({ event: "failed", code: "reattachment_failed" });
				expect(await test.checkpoint()).toEqual(before);
			}
			Object.assign(test.launch, original);
			await test.save();
			await test.rebind();
			// The new attempt gets a new private socket, never a new run budget.
			const restored = await test.start("resume-ipc").exit;
			expect(restored.code).toBe(0);
			expect(events(restored)).toMatchObject([
				{ event: "started", restored: true, counts: { calls: before.calls, children: before.children } },
				{ event: "stopped", reason: "completed" },
			]);
			const after = await test.checkpoint();
			expect(after.deadline).toBe(before.deadline);
			expect(after.calls).toBe(before.calls + 2);
			expect(after.children).toBe(before.children);
			expect(test.calls).toEqual([
				{ schema_version: 1, scope: original.scope, operation: "read_evidence", arguments: {} },
				{ schema_version: 1, scope: original.scope, operation: "read_evidence", arguments: {} },
			]);
			await test.noKernels();
		} finally {
			if (run.child.exitCode === null && run.child.signalCode === null) run.child.kill("SIGTERM");
			await run.exit;
			await test.cleanup();
		}
	}, 60_000);
});
