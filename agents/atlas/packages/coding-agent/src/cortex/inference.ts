import { randomUUID } from "node:crypto";
import {
	type Api,
	type AssistantMessage,
	type Context,
	createAssistantMessageEventStream,
	getApiProvider,
	type Model,
	type SimpleStreamOptions,
	unregisterApiProviders,
} from "@earendil-works/pi-ai";
import { AuthStorage } from "../core/auth-storage.js";
import { ModelRegistry } from "../core/model-registry.js";
import type { TreeBudget } from "./policy.js";

/** A private provider alias catches *all* SDK calls, including compaction. */
export function createCortexInference(
	pinned: Model<Api>,
	sourceRegistry: ModelRegistry,
	budget: TreeBudget,
	signal: AbortSignal,
) {
	const upstream = getApiProvider(pinned.api);
	if (!upstream) throw new Error("Pinned Cortex inference provider unavailable");
	const provider = `cortex-${randomUUID()}`;
	const authStorage = AuthStorage.inMemory();
	const registry = ModelRegistry.inMemory(authStorage);
	authStorage.setRuntimeApiKey(provider, "controller-mediated");
	const stream = (_model: Model<Api>, context: Context, options?: SimpleStreamOptions) => {
		budget.assertActive();
		if (_model.provider !== provider || _model.id !== pinned.id) throw new Error("Unpinned Cortex model");
		const rates = Object.values(pinned.cost);
		if (rates.some((rate) => !Number.isFinite(rate) || rate < 0)) throw new Error("Unknown inference price");
		const tokens = pinned.contextWindow + pinned.maxTokens;
		const release = budget.reserveCall(tokens, Math.ceil(tokens * Math.max(...rates)));
		const output = createAssistantMessageEventStream();
		void (async () => {
			try {
				const auth = await sourceRegistry.getApiKeyAndHeaders(pinned);
				if (!auth.ok) throw new Error("Inference credentials unavailable");
				const linked = options?.signal ? AbortSignal.any([signal, options.signal]) : signal;
				if (linked.aborted) throw new Error("Inference cancelled");
				const events = upstream.streamSimple(pinned, context, {
					...options,
					apiKey: auth.apiKey,
					headers: auth.headers,
					maxTokens: Math.min(options?.maxTokens ?? pinned.maxTokens, pinned.maxTokens),
					maxRetries: 0,
					signal: linked,
				});
				for await (const event of events) {
					if (event.type === "error") throw new Error("Upstream inference failed");
					const message = "partial" in event ? event.partial : event.type === "done" ? event.message : undefined;
					if (message?.errorMessage || message?.stopReason === "error" || message?.stopReason === "aborted") {
						throw new Error("Upstream inference failed");
					}
					output.push(event);
				}
				const result = await events.result();
				if (result.errorMessage || result.stopReason === "error" || result.stopReason === "aborted") {
					throw new Error("Upstream inference failed");
				}
				output.end(result);
			} catch {
				// Provider errors can contain request headers. Never forward their raw text.
				const error: AssistantMessage = {
					role: "assistant",
					content: [],
					api: provider,
					provider,
					model: pinned.id,
					stopReason: "error",
					errorMessage: "Cortex inference failed or was cancelled",
					timestamp: Date.now(),
					usage: {
						input: 0,
						output: 0,
						cacheRead: 0,
						cacheWrite: 0,
						totalTokens: 0,
						cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
					},
				};
				output.push({ type: "error", reason: "error", error });
				output.end(error);
			} finally {
				release();
			}
		})();
		return output;
	};
	registry.registerProvider(provider, {
		baseUrl: pinned.baseUrl,
		api: provider,
		apiKey: "controller-mediated",
		streamSimple: stream,
		models: [{ ...pinned, api: provider, headers: undefined }],
	});
	const model = registry.find(provider, pinned.id);
	if (!model) throw new Error("Cannot register Cortex model");
	return {
		model,
		registry,
		authStorage,
		dispose: () => unregisterApiProviders(`provider:${provider}`),
	};
}
