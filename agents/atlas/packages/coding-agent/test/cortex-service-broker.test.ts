import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, type RequestListener } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { CortexScope } from "../src/cortex/policy.js";
import { CortexServiceBroker } from "../src/cortex/service-broker.js";

const scope: CortexScope = { role: "experiment", id: "fixture", commitment: "a".repeat(64) };

async function serverTest(handler: RequestListener, run: (path: string) => Promise<void>): Promise<void> {
	const dir = await mkdtemp(join(tmpdir(), "cortex-ipc-"));
	const path = join(dir, "controller.sock");
	const server = createServer(handler);
	server.listen(path);
	await once(server, "listening");
	try {
		await run(path);
	} finally {
		server.closeAllConnections();
		await new Promise<void>((resolve) => server.close(() => resolve()));
		await rm(dir, { recursive: true, force: true });
	}
}

describe("Cortex private controller transport", () => {
	it("sends only bound scope and operation to the private socket", async () => {
		await serverTest(
			(request, response) => {
				const chunks: Buffer[] = [];
				request.on("data", (chunk: Buffer) => chunks.push(chunk));
				request.on("end", () => {
					expect(request.url).toBe("/call");
					expect(request.method).toBe("POST");
					expect(JSON.parse(Buffer.concat(chunks).toString())).toEqual({
						schema_version: 1,
						scope,
						operation: "report",
						arguments: { text: "Untrusted finding" },
					});
					response.end(JSON.stringify({ schema_version: 1, result: { authoritative: false } }));
				});
			},
			async (path) => {
				const broker = new CortexServiceBroker(path, scope);
				expect(
					await broker.call(scope, "report", { text: "Untrusted finding" }, new AbortController().signal),
				).toEqual({ authoritative: false });
			},
		);
	});

	it("rejects widened scopes and oversized requests without contacting the controller", async () => {
		let calls = 0;
		await serverTest(
			(_, response) => {
				calls++;
				response.end();
			},
			async (path) => {
				const broker = new CortexServiceBroker(path, scope);
				const signal = new AbortController().signal;
				await expect(broker.call({ ...scope, id: "other" }, "quote", {}, signal)).rejects.toThrow(/scope/);
				await expect(broker.call(scope, "submit_decision", {}, signal)).rejects.toThrow(/capability/);
				await expect(broker.call(scope, "report", { text: "a".repeat(70_000) }, signal)).rejects.toThrow(/limit/);
				expect(calls).toBe(0);
			},
		);
	});

	it.each([
		{ schema_version: 2, result: {} },
		{ schema_version: 1, result: [] },
		{ schema_version: 1, result: {}, extra: "private" },
		"not JSON",
		"a".repeat(130_000),
	])("redacts malformed and oversized controller replies", async (body) => {
		await serverTest(
			(_, response) => response.end(typeof body === "string" ? body : JSON.stringify(body)),
			async (path) => {
				await expect(
					new CortexServiceBroker(path, scope).call(scope, "quote", {}, new AbortController().signal),
				).rejects.toThrow("Cortex controller operation failed or was cancelled");
			},
		);
	});

	it("does not follow redirects or echo provider diagnostics", async () => {
		await serverTest(
			(_, response) => {
				response.writeHead(302, { location: "https://invalid.example/never-follow" });
				response.end("private provider diagnostic");
			},
			async (path) => {
				await expect(
					new CortexServiceBroker(path, scope).call(scope, "quote", {}, new AbortController().signal),
				).rejects.toThrow("Cortex controller operation failed or was cancelled");
			},
		);
	});

	it("bounds an unresponsive controller and never retries automatically", async () => {
		let calls = 0;
		await serverTest(
			() => {
				calls++;
			},
			async (path) => {
				await expect(
					new CortexServiceBroker(path, scope, 100).call(scope, "quote", {}, new AbortController().signal),
				).rejects.toThrow(/cancelled/);
				expect(calls).toBe(1);
			},
		);
	});

	it("cancels an in-flight request", async () => {
		const cancellation = new AbortController();
		await serverTest(
			() => cancellation.abort(),
			async (path) => {
				await expect(
					new CortexServiceBroker(path, scope).call(scope, "quote", {}, cancellation.signal),
				).rejects.toThrow(/cancelled/);
			},
		);
	});
});
