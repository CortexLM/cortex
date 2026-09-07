import { request } from "node:http";
import { isAbsolute } from "node:path";
import { assertScope, authorizeOperation, type CortexScope } from "./policy.js";
import type { CortexBroker } from "./runtime.js";

const MAX_REQUEST_BYTES = 64 * 1024;
const MAX_RESPONSE_BYTES = 128 * 1024;

/** Host-only IPC. Never mount this socket or its directory into an agent kernel. */
export class CortexServiceBroker implements CortexBroker {
	private readonly scope: CortexScope;

	constructor(
		private readonly socketPath: string,
		scope: CortexScope,
		private readonly timeoutMs = 30_000,
	) {
		assertScope(scope);
		if (
			!isAbsolute(socketPath) ||
			socketPath.includes("\0") ||
			!Number.isSafeInteger(timeoutMs) ||
			timeoutMs <= 0 ||
			timeoutMs > 30_000
		) {
			throw new Error("Invalid Cortex controller transport");
		}
		this.scope = Object.freeze({ ...scope });
	}

	async call(
		scope: CortexScope,
		operation: string,
		args: Record<string, unknown>,
		signal: AbortSignal,
	): Promise<Record<string, unknown>> {
		authorizeOperation(scope, operation);
		if (
			scope.role !== this.scope.role ||
			scope.id !== this.scope.id ||
			scope.commitment !== this.scope.commitment ||
			!args ||
			Array.isArray(args) ||
			typeof args !== "object"
		) {
			throw new Error("Cortex call outside controller scope");
		}
		const body = JSON.stringify({ schema_version: 1, scope: this.scope, operation, arguments: args });
		if (Buffer.byteLength(body) > MAX_REQUEST_BYTES) throw new Error("Cortex request exceeds limit");
		const deadline = AbortSignal.any([signal, AbortSignal.timeout(this.timeoutMs)]);
		return new Promise((resolve, reject) => {
			const fail = () => reject(new Error("Cortex controller operation failed or was cancelled"));
			const outgoing = request(
				{
					socketPath: this.socketPath,
					path: "/call",
					method: "POST",
					agent: false,
					signal: deadline,
					headers: { "content-type": "application/json", "content-length": Buffer.byteLength(body) },
				},
				(response) => {
					if (response.statusCode !== 200) {
						response.destroy();
						fail();
						return;
					}
					const chunks: Buffer[] = [];
					let bytes = 0;
					response.on("data", (chunk: Buffer) => {
						bytes += chunk.length;
						if (bytes > MAX_RESPONSE_BYTES) {
							response.destroy();
							fail();
						} else {
							chunks.push(chunk);
						}
					});
					response.on("error", fail);
					response.on("aborted", fail);
					response.on("end", () => {
						try {
							const reply: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8"));
							if (
								!reply ||
								typeof reply !== "object" ||
								!("schema_version" in reply) ||
								reply.schema_version !== 1 ||
								!("result" in reply) ||
								!reply.result ||
								typeof reply.result !== "object" ||
								Array.isArray(reply.result) ||
								Object.keys(reply).length !== 2
							) {
								throw new Error("Invalid controller response");
							}
							resolve(reply.result as Record<string, unknown>);
						} catch {
							fail();
						}
					});
				},
			);
			outgoing.on("error", fail);
			outgoing.end(body);
		});
	}
}
