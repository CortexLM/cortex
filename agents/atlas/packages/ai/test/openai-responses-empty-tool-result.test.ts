import { describe, expect, it } from "vitest";
import { getModel } from "../src/models.js";
import { convertResponsesMessages } from "../src/providers/openai-responses-shared.js";
import type { AssistantMessage, Context, Model, ToolResultMessage, Usage } from "../src/types.js";

const emptyUsage: Usage = {
	input: 0,
	output: 0,
	cacheRead: 0,
	cacheWrite: 0,
	totalTokens: 0,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
};

function buildContext(toolResult: ToolResultMessage, now: number): Context {
	const assistantMessage: AssistantMessage = {
		role: "assistant",
		content: [{ type: "toolCall", id: "tool-1", name: "bash", arguments: { cmd: "true" } }],
		api: "openai-responses",
		provider: "openai",
		model: "gpt-4o-mini",
		usage: emptyUsage,
		stopReason: "toolUse",
		timestamp: now,
	};
	return {
		messages: [{ role: "user", content: "Run it", timestamp: now - 2 }, assistantMessage, toolResult],
	};
}

describe("openai-responses convertResponsesMessages", () => {
	const { compat: _compat, ...baseModel } = getModel("openai", "gpt-4o-mini");
	const model: Model<"openai-responses"> = {
		...baseModel,
		api: "openai-responses",
		input: ["text", "image"],
	};

	it("does not emit the image placeholder for empty-text tool results with no image", () => {
		const now = Date.now();
		const toolResult: ToolResultMessage = {
			role: "toolResult",
			toolCallId: "tool-1",
			toolName: "bash",
			content: [{ type: "text", text: "" }],
			isError: false,
			timestamp: now + 1,
		};

		const messages = convertResponsesMessages(model, buildContext(toolResult, now), new Set(["openai"]));
		const output = messages.find((m) => m.type === "function_call_output");
		expect(output?.output).toBe("");
		expect(output?.output).not.toBe("(see attached image)");
	});

	it("still attaches images for tool results that contain an image", () => {
		const now = Date.now();
		const toolResult: ToolResultMessage = {
			role: "toolResult",
			toolCallId: "tool-1",
			toolName: "read",
			content: [{ type: "image", data: "ZmFrZQ==", mimeType: "image/png" }],
			isError: false,
			timestamp: now + 1,
		};

		const messages = convertResponsesMessages(model, buildContext(toolResult, now), new Set(["openai"]));
		const output = messages.find((m) => m.type === "function_call_output");
		expect(Array.isArray(output?.output)).toBe(true);
		const parts = output?.output as Array<{ type?: string }>;
		expect(parts.some((p) => p.type === "input_image")).toBe(true);
	});
});
