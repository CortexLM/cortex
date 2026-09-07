import { mkdir, realpath } from "node:fs/promises";
import { join, relative } from "node:path";
import type { Api, Model } from "@earendil-works/pi-ai";
import type { AgentSession, AgentSessionEvent } from "../core/agent-session.js";
import type { AuthStorage } from "../core/auth-storage.js";
import { createExtensionRuntime } from "../core/extensions/index.js";
import type { HostRequestHandlers } from "../core/kernel/index.js";
import type { ModelRegistry } from "../core/model-registry.js";
import type { ResourceLoader } from "../core/resource-loader.js";
import {
	type CreateRlmSubagentRuntimeOptions,
	createRlmDeleteSubagentHostHandler,
	createRlmFindModelsHostHandler,
	createRlmListSubagentsHostHandler,
	createRlmRunHostHandler,
	type SubagentRuntimeHost,
} from "../core/rlm-runtime.js";
import { createAgentSession } from "../core/sdk.js";
import { SessionManager } from "../core/session-manager.js";
import { SettingsManager } from "../core/settings-manager.js";
import { IpythonKernelProvisioner } from "../core/tools/ipython.js";
import { wrapToolDefinition } from "../core/tools/tool-definition-wrapper.js";
import { cortexRuntimeBinding } from "./binding.js";
import { createCortexInference } from "./inference.js";
import { assertScope, authorizeOperation, type CortexScope, cortexPrompt, type TreeBudget } from "./policy.js";
import type { CortexSandbox } from "./sandbox.js";
import { flushCortexSession, openCortexSession, persistCortexSession } from "./session-file.js";
import { cortexIpythonTool } from "./tool.js";
import { TreeJournal } from "./tree-journal.js";

type CortexParent = Pick<
	CreateRlmSubagentRuntimeOptions,
	"id" | "parentSession" | "rlmParentNodeId" | "spawnedByRequestId"
>;

export interface CortexBroker {
	call(
		scope: CortexScope,
		operation: string,
		args: Record<string, unknown>,
		signal: AbortSignal,
	): Promise<Record<string, unknown>>;
}

export interface CortexRuntimeOptions {
	scope: CortexScope;
	stateDir: string;
	workspace: string;
	sandbox: CortexSandbox;
	model: Model<Api>;
	modelRegistry: ModelRegistry;
	authStorage: AuthStorage;
	budget: TreeBudget;
	broker: CortexBroker;
	/** Append to a controller-owned evidence sink, not a kernel-writable transcript. */
	record: (sessionId: string, event: AgentSessionEvent) => void;
}

export class CortexRuntime {
	private readonly abortController = new AbortController();
	private readonly sessions = new Set<AgentSession>();
	private readonly kernels = new Set<IpythonKernelProvisioner>();
	private readonly creating = new Set<Promise<AgentSession>>();
	private root?: AgentSession;
	private timer?: ReturnType<typeof setTimeout>;
	private readonly inference: ReturnType<typeof createCortexInference>;
	private closing?: Promise<void>;
	private started = false;
	private preserve = false;
	private failure?: Error;
	private tree?: TreeJournal;
	private restoring = false;
	private readonly byId = new Map<string, AgentSession>();
	private readonly cleanups = new Map<string, () => Promise<void>>();
	private readonly deletions = new Map<string, Promise<void>>();

	constructor(private readonly options: CortexRuntimeOptions) {
		assertScope(options.scope);
		if (!options.stateDir.startsWith("/") || !options.workspace.startsWith("/")) {
			throw new Error("Cortex runtime needs explicit absolute paths");
		}
		options.budget.assertBinding(cortexRuntimeBinding(options.scope, options.model, options.sandbox.limits));
		this.inference = createCortexInference(
			options.model,
			options.modelRegistry,
			options.budget,
			this.abortController.signal,
		);
	}

	async start(sessionFile?: string): Promise<AgentSession> {
		if (this.started) throw new Error("Cortex root already started");
		this.assertActive();
		this.started = true;
		try {
			await mkdir(this.options.stateDir, { recursive: true, mode: 0o700 });
			await mkdir(this.options.workspace, { recursive: true, mode: 0o700 });
			this.tree = new TreeJournal(
				join(this.options.stateDir, "tree.json"),
				cortexRuntimeBinding(this.options.scope, this.options.model, this.options.sandbox.limits),
				this.options.budget.limits.maxChildren + 1,
			);
			if (this.tree.restored && !this.options.budget.restored)
				throw new Error("Cortex budget checkpoint is missing");
			const nodes = this.tree.entries();
			if (nodes.length) {
				if (sessionFile && nodes[0].file !== sessionFile) throw new Error("Cortex root checkpoint mismatch");
				sessionFile = nodes[0].file;
			} else if (sessionFile) {
				throw new Error("Cortex tree checkpoint is missing");
			}
			await this.options.sandbox.stop();
			for (const node of [...nodes].reverse()) {
				if (node.status === "deleting") this.tree.status(node.sessionId, "deleted");
			}
			if (sessionFile) {
				if (!this.options.budget.restored) throw new Error("Cannot resume without the original Cortex budget");
				const path = await realpath(sessionFile);
				const suffix = relative(join(this.options.stateDir, "sessions"), path);
				if (path !== sessionFile || suffix.startsWith("..") || suffix.startsWith("/")) {
					throw new Error("Session outside Cortex state directory");
				}
			}
			const managers = new Map<string, SessionManager>();
			for (const node of nodes) {
				if (node.status === "deleted" || node.status === "deleting") continue;
				const parent = nodes.find((entry) => entry.sessionId === node.parentId);
				managers.set(
					node.sessionId,
					openCortexSession(node.file, {
						sessionId: node.sessionId,
						depth: node.depth,
						parentSession: parent?.file,
					}),
				);
			}
			const manager = sessionFile
				? managers.get(nodes[0].sessionId)
				: SessionManager.create(this.options.workspace, join(this.options.stateDir, "sessions"));
			if (!manager) throw new Error("Cortex root checkpoint is missing");
			if (!sessionFile) manager.newSession({ id: manager.getSessionId(), rlmDepth: 0 });
			if (manager.getHeader()?.rlmDepth !== 0) throw new Error("Invalid Cortex root session");
			this.restoring = true;
			this.root = await this.create(manager, 0);
			for (const node of nodes.slice(1)) {
				if (node.status === "deleted" || node.status === "deleting") continue;
				const parent = node.parentId ? this.byId.get(node.parentId) : undefined;
				if (!parent || !node.childId || (await realpath(node.file)) !== node.file) {
					throw new Error("Invalid Cortex child checkpoint");
				}
				const childManager = managers.get(node.sessionId);
				if (!childManager) throw new Error("Cortex child checkpoint is missing");
				if (
					childManager.getSessionId() !== node.sessionId ||
					childManager.getHeader()?.rlmDepth !== node.depth ||
					childManager.getHeader()?.parentSession !== parent.sessionFile
				)
					throw new Error("Cortex child checkpoint mismatch");
				const child = await this.create(childManager, node.depth, {
					id: node.childId,
					parentSession: parent,
					rlmParentNodeId: node.childId,
				});
				if (
					!parent.registerRlmChildSession(node.childId, child, undefined, {
						status: node.status === "completed" ? "done" : "error",
						error:
							node.status === "completed"
								? undefined
								: "Interrupted before completion; explicit continuation required",
					})
				)
					throw new Error("Cannot restore Cortex child");
			}
			this.restoring = false;
			this.timer = setTimeout(() => {
				void this.stop().catch(() => {
					this.failure = new Error("Cortex deadline cleanup failed");
				});
			}, this.options.budget.remainingMs());
			return this.root;
		} catch (error) {
			await this.stop();
			throw error;
		}
	}

	async prompt(text: string): Promise<void> {
		this.assertActive();
		if (this.restoring) throw new Error("Cortex tree is restoring");
		if (!this.root) throw new Error("Cortex runtime not started");
		this.tree?.status(this.root.sessionId, "running");
		await this.root.promptAndWait(text, { expandPromptTemplates: false });
		await this.root.waitForRlmQuiescence();
		for (const session of this.sessions) flushCortexSession(session.sessionManager);
		if (this.failure) throw this.failure;
		if (!this.closing) this.tree?.status(this.root.sessionId, "completed");
	}

	stop(): Promise<void> {
		if (this.closing && this.preserve) {
			return Promise.reject(new Error("Paused Cortex runtime must be cancelled through its controller"));
		}
		this.closing ??= Promise.resolve()
			.then(() => this.close())
			.catch((error: unknown) => {
				this.closing = undefined;
				throw error;
			});
		return this.closing;
	}

	pause(): Promise<void> {
		if (!this.closing) {
			this.preserve = true;
			this.closing = Promise.resolve()
				.then(() => this.close())
				.catch((error: unknown) => {
					this.closing = undefined;
					this.preserve = false;
					throw error;
				});
		}
		return this.closing;
	}

	private assertActive(): void {
		if (this.failure) throw this.failure;
		if (this.abortController.signal.aborted || this.closing) throw new Error("Cortex runtime is closed");
		this.options.budget.assertActive();
	}

	private fail(message: string): void {
		this.failure = new Error(message);
		void this.stop().catch(() => {
			this.failure = new Error("Cortex failure requires reconciliation");
		});
	}

	private assertSessionActive(id: string): void {
		this.assertActive();
		if (this.restoring) throw new Error("Cortex tree is restoring");
		const node = this.tree?.entries().find((entry) => entry.sessionId === id);
		if (!node || node.status === "deleting" || node.status === "deleted")
			throw new Error("Cortex session is unavailable");
	}

	private deleteSubtree(id: string): Promise<void> {
		const existing = this.deletions.get(id);
		if (existing) return existing;
		if (!this.tree) throw new Error("Cortex tree is missing");
		const nodes = this.tree.beginDeletion(id);
		const deletion = Promise.resolve()
			.then(async () => {
				await Promise.allSettled([...this.creating]);
				for (const node of nodes.reverse()) {
					if (node.status === "deleted") continue;
					const child = this.byId.get(node.sessionId);
					await child?.abort();
					// SDK disposal callbacks are best-effort. Join our own cleanup before certifying absence.
					await this.cleanups.get(node.sessionId)?.();
					await child?.disposeAsync();
					this.tree?.status(node.sessionId, "deleted");
				}
			})
			.finally(() => this.deletions.delete(id));
		this.deletions.set(id, deletion);
		return deletion;
	}

	private async close(): Promise<void> {
		const failures: unknown[] = [];
		if (!this.preserve) {
			try {
				this.options.budget.revoke();
			} catch (error) {
				failures.push(error);
			}
		}
		this.abortController.abort();
		if (this.timer) clearTimeout(this.timer);
		await Promise.allSettled([...this.creating]);
		await Promise.allSettled([...this.deletions.values()]);
		const sessions = [...this.sessions];
		await Promise.allSettled(sessions.map((session) => session.abort()));
		for (const session of sessions) {
			try {
				flushCortexSession(session.sessionManager);
			} catch (error) {
				failures.push(error);
			}
		}
		if (this.preserve) {
			for (const kernel of this.kernels) {
				if (kernel.hasRunningKernel && !(await kernel.manager?.snapshotState())) {
					failures.push(new Error("Cannot checkpoint Cortex kernel"));
				}
			}
		}
		const cleanupResults = await Promise.allSettled([...this.cleanups.values()].map((cleanup) => cleanup()));
		for (const result of cleanupResults) if (result.status === "rejected") failures.push(result.reason);
		await Promise.allSettled([...this.kernels].map((kernel) => kernel.dispose({ snapshot: false })));
		await Promise.allSettled(sessions.map((session) => session.disposeAsync()));
		this.inference.dispose();
		await this.options.sandbox.stop().catch((error: unknown) => failures.push(error));
		if (failures.length) throw new Error("Cortex shutdown requires reconciliation");
		this.sessions.clear();
		this.kernels.clear();
		this.byId.clear();
		this.options.budget.release();
	}

	private loader(): ResourceLoader {
		return {
			getExtensions: () => ({ extensions: [], errors: [], runtime: createExtensionRuntime() }),
			getSkills: () => ({ skills: [], diagnostics: [] }),
			getPrompts: () => ({ prompts: [], diagnostics: [] }),
			getThemes: () => ({ themes: [], diagnostics: [] }),
			getAgentsFiles: () => ({ agentsFiles: [] }),
			getSystemPrompt: () => cortexPrompt(this.options.scope),
			getAppendSystemPrompt: () => [],
			extendResources: () => {},
			reload: async () => {},
		};
	}

	private create(manager: SessionManager, depth: number, parent?: CortexParent): Promise<AgentSession> {
		this.assertActive();
		const pending = this.createSession(manager, depth, parent);
		this.creating.add(pending);
		void pending.then(
			() => this.creating.delete(pending),
			() => this.creating.delete(pending),
		);
		return pending;
	}

	private async createSession(manager: SessionManager, depth: number, parent?: CortexParent): Promise<AgentSession> {
		const { options } = this;
		const workspace = join(options.workspace, manager.getSessionId());
		await mkdir(workspace, { recursive: true, mode: 0o700 });
		const kernelLauncher = await options.sandbox.prepare(workspace, options.budget.remainingMs());
		let session: AgentSession;
		const handlers: HostRequestHandlers = {
			"rlm.run": createRlmRunHostHandler(async ({ prompt, kwargs, cellSourceCode }) => {
				this.assertActive();
				if (this.restoring) throw new Error("Cortex tree is restoring");
				return { ...(await session.runRlmChild(prompt, kwargs, cellSourceCode)) };
			}),
			"rlm.find_models": createRlmFindModelsHostHandler(() => ({
				models: [
					{
						provider: this.inference.model.provider,
						id: this.inference.model.id,
						name: this.inference.model.name,
						selector: `${this.inference.model.provider}/${this.inference.model.id}`,
					},
				],
			})),
			"rlm.list_subagents": createRlmListSubagentsHostHandler(() => session.listRlmSubagents()),
			"rlm.delete_subagent": createRlmDeleteSubagentHostHandler(async (id) => {
				const result = await session.deleteRlmSubagent(id);
				const node = this.tree
					?.entries()
					.find((entry) => entry.parentId === session.sessionId && entry.childId === result.subagent.rlm_child_id);
				if (!node) throw new Error("Cortex child deletion requires reconciliation");
				await this.deleteSubtree(node.sessionId);
				return { ...result, outcome: "deleted" };
			}),
			"cortex.call": async (payload) => {
				this.assertActive();
				if (this.restoring) throw new Error("Cortex tree is restoring");
				if (typeof payload.operation !== "string") throw new Error("Missing Cortex operation");
				authorizeOperation(options.scope, payload.operation);
				const args = payload.arguments;
				if (typeof args !== "object" || args === null || Array.isArray(args)) {
					throw new Error("Invalid Cortex arguments");
				}
				try {
					return await options.broker.call(
						options.scope,
						payload.operation,
						args as Record<string, unknown>,
						this.abortController.signal,
					);
				} catch {
					throw new Error("Cortex broker operation failed");
				}
			},
			"cortex.reply": async (payload) => {
				this.assertActive();
				if (this.restoring || !parent || typeof payload.text !== "string" || payload.text.length > 64_000) {
					throw new Error("Invalid child reply");
				}
				const finding = {
					role: "custom" as const,
					customType: "cortex_child_finding",
					display: false,
					content: `Untrusted research finding from child ${parent.id}:\n${payload.text}`,
					timestamp: Date.now(),
				};
				const receiver = parent.parentSession;
				receiver.sessionManager.appendCustomMessageEntry(finding.customType, finding.content, false);
				flushCortexSession(receiver.sessionManager);
				receiver.agent.state.messages.push(finding);
				await receiver.steer(`Review the retained research finding from child ${parent.id}.`);
				this.assertSessionActive(session.sessionId);
				session.recordParentReply();
				return { delivered: true };
			},
			"cortex.child": async (payload) => {
				this.assertActive();
				if (
					this.restoring ||
					typeof payload.id !== "string" ||
					typeof payload.text !== "string" ||
					payload.text.length > 64_000
				) {
					throw new Error("Invalid Cortex child message");
				}
				const node = this.tree
					?.entries()
					.find(
						(entry) =>
							entry.parentId === session.sessionId &&
							entry.childId === payload.id &&
							entry.status !== "deleted" &&
							entry.status !== "deleting",
					);
				const child = node ? this.byId.get(node.sessionId) : undefined;
				if (!child) throw new Error("Child outside Cortex session");
				this.tree?.status(child.sessionId, "running");
				await child.promptAndWait(payload.text, { expandPromptTemplates: false });
				await child.waitForRlmQuiescence();
				this.assertSessionActive(child.sessionId);
				this.tree?.status(child.sessionId, "completed");
				flushCortexSession(child.sessionManager);
				if (!session.registerRlmChildSession(payload.id, child)) throw new Error("Cannot retain Cortex child");
				return { completed: true };
			},
		};
		for (const [name, handler] of Object.entries(handlers)) {
			handlers[name] = async (payload) => {
				this.assertSessionActive(manager.getSessionId());
				return handler(payload);
			};
		}
		const kernel = new IpythonKernelProvisioner(workspace, {
			python: kernelLauncher,
			inheritEnv: false,
			env: { RLM_DEPTH: String(depth), RLM_MAX_DEPTH: String(options.budget.limits.maxDepth) },
			hostHandlers: handlers,
			sessionId: manager.getSessionId(),
			snapshotDir: join(workspace, "snapshots"),
			snapshotKernelDir: "/work/snapshots",
			onRestore: (result) => {
				void session.sendCustomMessage(
					{
						customType: "cortex_kernel_restore",
						content: `Kernel checkpoint restored: ${JSON.stringify(result)}`,
						display: false,
					},
					{ deliverAs: "nextTurn" },
				);
			},
		});
		this.kernels.add(kernel);
		const host: SubagentRuntimeHost = {
			createRlmSubagentRuntime: async (child) => {
				this.assertSessionActive(manager.getSessionId());
				if (child.rlmDepth !== depth + 1) throw new Error("Invalid Cortex child depth");
				options.budget.admitChild(depth + 1);
				if (child.model.provider !== this.inference.model.provider || child.model.id !== options.model.id) {
					throw new Error("Child model outside pinned Cortex provider");
				}
				const childManager = SessionManager.create(options.workspace, child.sessionDir);
				childManager.newSession({
					id: childManager.getSessionId(),
					parentSession: manager.getSessionFile(),
					rlmDepth: depth + 1,
				});
				const created = await this.create(childManager, child.rlmDepth, child);
				this.assertSessionActive(manager.getSessionId());
				created.setSessionName(child.sessionName);
				child.onSessionPublished?.(created);
				return { session: created };
			},
			deleteRlmSubagentRuntime: async (id, child) => {
				const node = this.tree
					?.entries()
					.find((entry) => entry.parentId === manager.getSessionId() && entry.childId === id);
				if (!node || (child && child.sessionId !== node.sessionId)) throw new Error("Child outside Cortex session");
				await this.deleteSubtree(node.sessionId);
			},
			completeRlmSubagentRuntime: (_id, child) => {
				const status = this.tree?.entries().find((node) => node.sessionId === child.sessionId)?.status;
				if (status === "deleting" || status === "deleted") return false;
				if (!this.restoring && !this.closing) this.tree?.status(child.sessionId, "completed");
				return true;
			},
		};
		const result = await createAgentSession({
			cwd: workspace,
			agentDir: options.stateDir,
			sessionManager: manager,
			authStorage: this.inference.authStorage,
			modelRegistry: this.inference.registry,
			model: this.inference.model,
			resourceLoader: this.loader(),
			settingsManager: SettingsManager.inMemory({
				autoRefine: { enabled: false },
				agentTraces: { enabled: false },
				telemetry: { enabled: false },
				retry: { enabled: false },
				compaction: { enabled: true },
			}),
			tools: ["ipython"],
			baseToolsOverride: { ipython: wrapToolDefinition(cortexIpythonTool(workspace, kernel)) },
			rlmDepth: depth,
			rlmMaxDepth: options.budget.limits.maxDepth,
			rlmParentNodeId: parent?.rlmParentNodeId,
			rlmParentAgent: parent?.parentSession.sessionName ?? parent?.parentSession.sessionId,
			semanticParentSessionId: parent?.parentSession.sessionId,
			semanticSpawnedByRequestId: parent?.spawnedByRequestId,
			rlmSessionDir: manager.getSessionArtifactDir(),
			subagentRuntimeHost: host,
			prewarmIpythonKernel: false,
			includeGoals: false,
			includeCompactSkill: false,
			includeHarnessState: false,
			allowSessionCommands: false,
		});
		session = result.session;
		this.sessions.add(session);
		this.byId.set(session.sessionId, session);
		let unsubscribePersistence: (() => void) | undefined;
		let cleanup: Promise<void> | undefined;
		const cleanupSession = () => {
			cleanup ??= (async () => {
				await kernel.dispose({ snapshot: false });
				await options.sandbox.stopLauncher(kernelLauncher);
				this.sessions.delete(session);
				this.kernels.delete(kernel);
				this.byId.delete(session.sessionId);
				this.cleanups.delete(session.sessionId);
				unsubscribePersistence?.();
			})().catch((error: unknown) => {
				cleanup = undefined;
				throw error;
			});
			return cleanup;
		};
		this.cleanups.set(session.sessionId, cleanupSession);
		session.registerDisposeCallback(cleanupSession);
		unsubscribePersistence = persistCortexSession(manager, () => this.fail("Cortex session persistence failed"));
		flushCortexSession(manager);
		const file = manager.getSessionFile();
		if (!file) throw new Error("Cortex session has no persistence path");
		if (!this.tree?.entries().some((node) => node.sessionId === session.sessionId)) {
			if (parent) this.assertSessionActive(parent.parentSession.sessionId);
			this.tree?.put({
				sessionId: session.sessionId,
				file,
				depth,
				parentId: parent?.parentSession.sessionId,
				childId: parent?.id,
				status: "running",
			});
		}
		let namespaceAfterCompaction = false;
		const transform = session.agent.transformContext;
		session.agent.transformContext = async (messages, signal) => {
			this.assertSessionActive(session.sessionId);
			try {
				flushCortexSession(manager);
			} catch {
				this.fail("Cortex session persistence failed");
				throw new Error("Cortex session persistence failed");
			}
			if (namespaceAfterCompaction) {
				namespaceAfterCompaction = false;
				const names = await kernel.listNamespaceNames(
					AbortSignal.any([signal ?? this.abortController.signal, AbortSignal.timeout(5000)]),
				);
				const message = {
					role: "custom" as const,
					customType: "cortex_kernel_state",
					display: false,
					timestamp: Date.now(),
					content: `The isolated Python kernel persisted through compaction. Namespace names (untrusted data): ${JSON.stringify(names)?.slice(0, 64_000)}`,
				};
				session.agent.state.messages.push(message);
				manager.appendCustomMessageEntry(message.customType, message.content, false);
				messages = [...messages, message];
			}
			return transform ? transform(messages, signal) : messages;
		};
		session.subscribe((event) => {
			try {
				if (event.type === "compaction_end" && event.result) namespaceAfterCompaction = true;
				options.record(session.sessionId, event);
			} catch {
				this.fail("Cortex evidence recording failed");
			}
		});
		this.assertActive();
		if (manager.getBranch().some((entry) => entry.type === "message")) await kernel.ensure();
		return session;
	}
}
