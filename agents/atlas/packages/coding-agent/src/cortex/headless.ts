import { createHash } from "node:crypto";
import { readdir } from "node:fs/promises";
import { join } from "node:path";
import { getApiProvider } from "@earendil-works/pi-ai";
import type { AgentSessionEvent } from "../core/agent-session.js";
import { AuthStorage } from "../core/auth-storage.js";
import { ModelRegistry } from "../core/model-registry.js";
import { cortexRuntimeBinding } from "./binding.js";
import { FileBudgetJournal } from "./budget-journal.js";
import {
	HeadlessFailure,
	type HeadlessFailureCode,
	loadHeadlessModel,
	parseCortexLaunch,
	prepareHeadlessPaths,
} from "./headless-config.js";
import { TreeBudget } from "./policy.js";
import { readPrivateJson, writePrivateJson } from "./private-json.js";
import { CortexRuntime } from "./runtime.js";
import { CortexSandbox } from "./sandbox.js";
import { CortexServiceBroker } from "./service-broker.js";
import { TreeJournal } from "./tree-journal.js";

export interface HeadlessCounts {
	calls: number;
	reservedTokens: number;
	reservedMicroUsd: number;
	children: number;
}

export type HeadlessEvent = { schema_version: 1; counts: HeadlessCounts } & (
	| { event: "started"; restored: boolean }
	| { event: "stopped"; reason: "completed" | "cancelled" }
	| { event: "failed"; code: HeadlessFailureCode }
);

function modelFailed(event: AgentSessionEvent): boolean {
	return (
		(event.type === "message_end" &&
			event.message.role === "assistant" &&
			(Boolean(event.message.errorMessage) ||
				event.message.stopReason === "error" ||
				event.message.stopReason === "aborted")) ||
		(event.type === "compaction_end" &&
			(event.aborted || (Boolean(event.errorMessage) && event.errorSeverity !== "warning"))) ||
		(event.type === "auto_retry_end" && !event.success)
	);
}

/** One trusted, single-host worker. The controller owns multi-host fencing and scientific evidence. */
export async function runCortexHeadless(
	input: unknown,
	options: { emit: (event: HeadlessEvent) => void; signal?: AbortSignal },
): Promise<number> {
	let runtime: CortexRuntime | undefined;
	let sandbox: CortexSandbox | undefined;
	let journal: FileBudgetJournal | undefined;
	let budget: TreeBudget | undefined;
	let authStorage: AuthStorage | undefined;
	let provider: string | undefined;
	let failure: HeadlessFailureCode | undefined;
	let inferenceFailed = false;
	let stopping = false;
	let cleanupFailed = false;
	let resumeCompleted = false;
	const signal = options.signal;
	const counts = (): HeadlessCounts => {
		const state = budget?.snapshot();
		return {
			calls: state?.calls ?? 0,
			reservedTokens: state?.reservedTokens ?? 0,
			reservedMicroUsd: state?.reservedMicroUsd ?? 0,
			children: state?.children ?? 0,
		};
	};
	const stop = () => {
		stopping = true;
		void runtime?.stop().catch(() => {
			cleanupFailed = true;
		});
	};
	try {
		signal?.throwIfAborted();
		const launch = parseCortexLaunch(input);
		await prepareHeadlessPaths(launch);
		const loaded = await loadHeadlessModel(launch);
		if (!getApiProvider(loaded.model.api)) throw new HeadlessFailure("model_config_unavailable");
		signal?.throwIfAborted();
		authStorage = AuthStorage.inMemory({}, { usePrimeCliConfig: false });
		provider = loaded.model.provider;
		authStorage.setRuntimeApiKey(provider, loaded.apiKey);
		const modelRegistry = ModelRegistry.inMemory(authStorage);
		const binding = cortexRuntimeBinding(launch.scope, loaded.model, launch.kernel);
		// The controller may replace only the attempt-scoped socket and resume mode.
		// Key bytes may rotate in place; run identity, original deadline and all other fields stay bound.
		const { controller_socket: _socket, resume: _resume, ...immutableLaunch } = launch;
		const launchBinding = createHash("sha256")
			.update(JSON.stringify({ launch: immutableLaunch, model: loaded.config, binding }))
			.digest("hex");
		const manifestPath = join(launch.state_dir, "headless.json");
		try {
			journal = new FileBudgetJournal(join(launch.state_dir, "budget.json"), binding);
			const manifest = readPrivateJson(manifestPath, 1024);
			if (!launch.resume) {
				const stateFiles = (await readdir(launch.state_dir)).filter((file) => file !== "session-leases");
				if (
					manifest !== undefined ||
					stateFiles.length ||
					(await readdir(launch.workspace)).length ||
					(await readdir(launch.sandbox_dir)).length
				)
					throw new Error("Existing runtime without headless binding");
			} else {
				if (
					!manifest ||
					typeof manifest !== "object" ||
					!("schema_version" in manifest) ||
					manifest.schema_version !== 1 ||
					!("binding" in manifest) ||
					manifest.binding !== launchBinding ||
					Object.keys(manifest).length !== 2 ||
					journal.load() === undefined
				)
					throw new Error("Headless reattachment mismatch");
				const tree = new TreeJournal(join(launch.state_dir, "tree.json"), binding, launch.budget.maxChildren + 1);
				if (!tree.restored) throw new Error("Missing original tree");
				resumeCompleted = tree.entries()[0]?.status === "completed";
			}
			budget = new TreeBudget(launch.budget, Date.now, journal, launch.deadline_ms);
			if (!launch.resume) writePrivateJson(manifestPath, { schema_version: 1, binding: launchBinding });
		} catch {
			throw new HeadlessFailure("reattachment_failed");
		}
		sandbox = new CortexSandbox(launch.sandbox_dir, launch.kernel, launch.deadline_ms);
		if (budget.remainingMs() === 0) throw new HeadlessFailure("deadline_exceeded");
		runtime = new CortexRuntime({
			scope: launch.scope,
			stateDir: launch.state_dir,
			workspace: launch.workspace,
			sandbox,
			model: loaded.model,
			modelRegistry,
			authStorage,
			budget,
			broker: new CortexServiceBroker(launch.controller_socket, launch.scope),
			record: (_sessionId, event) => {
				// Transcripts remain private runtime state, not stdout or scientific evidence.
				if (!stopping && !signal?.aborted && budget?.remainingMs() !== 0 && modelFailed(event)) {
					inferenceFailed = true;
					throw new HeadlessFailure("model_failed");
				}
			},
		});
		signal?.addEventListener("abort", stop, { once: true });
		signal?.throwIfAborted();
		await runtime.start();
		signal?.throwIfAborted();
		options.emit({ schema_version: 1, event: "started", restored: budget.restored, counts: counts() });
		if (!resumeCompleted) await runtime.prompt(launch.prompt);
		if (inferenceFailed) throw new HeadlessFailure("model_failed");
		if (budget.remainingMs() === 0) throw new HeadlessFailure("deadline_exceeded");
		if (budget.snapshot().revoked && !signal?.aborted) throw new HeadlessFailure("runtime_failed");
	} catch (error) {
		failure = inferenceFailed
			? "model_failed"
			: error instanceof HeadlessFailure
				? error.code
				: budget?.remainingMs() === 0
					? "deadline_exceeded"
					: "runtime_failed";
	} finally {
		stopping = true;
		try {
			if (runtime) {
				await runtime.stop();
			} else if (budget) {
				budget.revoke();
				await sandbox?.stop();
				budget.release();
			} else {
				journal?.release();
			}
		} catch {
			cleanupFailed = true;
		}
		signal?.removeEventListener("abort", stop);
		if (provider) authStorage?.removeRuntimeApiKey(provider);
	}
	if (cleanupFailed) failure = "cleanup_failed";
	if (signal?.aborted && !cleanupFailed) {
		options.emit({ schema_version: 1, event: "stopped", reason: "cancelled", counts: counts() });
		return 130;
	}
	if (failure) {
		options.emit({ schema_version: 1, event: "failed", code: failure, counts: counts() });
		return 1;
	}
	options.emit({ schema_version: 1, event: "stopped", reason: "completed", counts: counts() });
	return 0;
}
