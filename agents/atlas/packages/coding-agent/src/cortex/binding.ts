import { createHash } from "node:crypto";
import type { Api, Model } from "@earendil-works/pi-ai";
import { assertScope, type CortexScope, cortexPrompt } from "./policy.js";
import type { SandboxLimits } from "./sandbox.js";

/** Secrets are deliberately excluded; executable/tool provenance is committed by the scope. */
export function cortexRuntimeBinding(scope: CortexScope, model: Model<Api>, sandbox: SandboxLimits): string {
	assertScope(scope);
	return createHash("sha256")
		.update(
			JSON.stringify({
				version: 1,
				scope: { role: scope.role, id: scope.id, commitment: scope.commitment },
				prompt: cortexPrompt(scope),
				model: {
					provider: model.provider,
					api: model.api,
					id: model.id,
					baseUrl: model.baseUrl,
					contextWindow: model.contextWindow,
					maxTokens: model.maxTokens,
					cost: {
						input: model.cost.input,
						output: model.cost.output,
						cacheRead: model.cost.cacheRead,
						cacheWrite: model.cost.cacheWrite,
					},
				},
				sandbox: {
					image: sandbox.image,
					memoryMb: sandbox.memoryMb,
					workspaceMb: sandbox.workspaceMb,
					cpus: sandbox.cpus,
					pids: sandbox.pids,
					seconds: sandbox.seconds,
				},
			}),
		)
		.digest("hex");
}
