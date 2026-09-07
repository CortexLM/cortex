import assert from "node:assert/strict";
import { appendFile } from "node:fs/promises";

interface RequestBody {
	model: string;
	reasoning?: { effort?: string; summary?: string };
	include?: string[];
	max_output_tokens?: number;
	store?: boolean;
	input: { role?: string }[];
	tools?: { name: string }[];
}

// Preload only a transport double: leave the built-in API registry and lazy SDK import intact.
globalThis.fetch = async (url, init) => {
	assert.equal(String(url), "http://127.0.0.1:9/v1/responses");
	assert.equal(new Headers(init?.headers).get("authorization"), "Bearer synthetic-headless-key");
	assert.equal(new Headers(init?.headers).get("x-stainless-retry-count"), "0");
	assert.ok(init && typeof init.body === "string");
	const body = JSON.parse(init.body) as RequestBody;
	const capture = process.env.CORTEX_HEADLESS_TEST_REQUESTS;
	assert.ok(capture);
	await appendFile(
		capture,
		`${JSON.stringify({
			model: body.model,
			reasoning: body.reasoning,
			include: body.include,
			max_output_tokens: body.max_output_tokens,
			store: body.store,
			roles: body.input.map((item) => item.role),
			toolNames: body.tools?.map((tool) => tool.name) ?? [],
		})}\n`,
		{ mode: 0o600 },
	);
	if (process.env.CORTEX_HEADLESS_TEST_MODE === "transport-reject") {
		return new Response(
			JSON.stringify({
				error: {
					type: "invalid_request_error",
					message: "PRIVATE ROOT PROMPT Authorization: synthetic-headless-key",
				},
			}),
			{ status: 400, headers: { "Content-Type": "application/json" } },
		);
	}
	const item = {
		type: "message",
		id: "msg_synthetic",
		role: "assistant",
		status: "completed",
		content: [{ type: "output_text", text: "Synthetic transport completed.", annotations: [] }],
	};
	const events = [
		{ type: "response.created", response: { id: "resp_synthetic" } },
		{
			type: "response.output_item.added",
			output_index: 0,
			item: { ...item, content: [], status: "in_progress" },
		},
		{ type: "response.output_item.done", output_index: 0, item },
		{
			type: "response.completed",
			response: {
				id: "resp_synthetic",
				status: "completed",
				output: [item],
				usage: { input_tokens: 10, output_tokens: 5, total_tokens: 15 },
			},
		},
	];
	return new Response(
		`${events.map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join("")}data: [DONE]\n\n`,
		{ status: 200, headers: { "Content-Type": "text/event-stream" } },
	);
};
