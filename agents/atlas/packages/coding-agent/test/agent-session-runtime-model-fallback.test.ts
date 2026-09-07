import type { Api, Model } from "@earendil-works/pi-ai";
import { describe, expect, it } from "vitest";
import type { AgentSession } from "../src/core/agent-session.js";
import { AgentSessionRuntime, type CreateAgentSessionRuntimeFactory } from "../src/core/agent-session-runtime.js";
import type { AgentSessionServices } from "../src/core/agent-session-services.js";
import { formatNoModelsAvailableMessage } from "../src/core/auth-guidance.js";

const model = { provider: "prime-inference", id: "gpt-5.5" } as Model<Api>;

function makeRuntime(options: { model?: Model<Api>; modelFallbackMessage?: string }): AgentSessionRuntime {
	const session = {
		model: options.model,
		setSubagentRuntimeHost: () => {},
	} as unknown as AgentSession;
	const createRuntime: CreateAgentSessionRuntimeFactory = () => {
		throw new Error("not used");
	};
	return new AgentSessionRuntime(session, {} as AgentSessionServices, createRuntime, [], options.modelFallbackMessage);
}

describe("AgentSessionRuntime.modelFallbackMessage", () => {
	it("reports the no-models warning while the session has no model", () => {
		const runtime = makeRuntime({ modelFallbackMessage: formatNoModelsAvailableMessage() });

		expect(runtime.modelFallbackMessage).toBe(formatNoModelsAvailableMessage());
	});

	it("drops the no-models warning once the session has a model", () => {
		const runtime = makeRuntime({ model, modelFallbackMessage: formatNoModelsAvailableMessage() });

		expect(runtime.modelFallbackMessage).toBeUndefined();
	});

	it("keeps one-time restore notices even when the session has a model", () => {
		const message = "Could not restore model anthropic/claude-old. Using prime-inference/gpt-5.5";
		const runtime = makeRuntime({ model, modelFallbackMessage: message });

		expect(runtime.modelFallbackMessage).toBe(message);
	});
});
