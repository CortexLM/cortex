import { spawn } from "node:child_process";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const packageRoot = fileURLToPath(new URL("..", import.meta.url));
const loader = fileURLToPath(new URL("../../../node_modules/tsx/dist/loader.mjs", import.meta.url));
const shared = new URL("../src/core/kernel/shared.ts", import.meta.url).href;

describe("Cortex kernel signal ownership", () => {
	it.each([false, true])(
		"preserves default behavior and permits explicit external ownership: %s",
		async (external) => {
			const script = `
import assert from "node:assert/strict";
import { installSignalHandlersOnce, manageKernelSignalsExternally } from ${JSON.stringify(shared)};
const before = ["SIGINT", "SIGTERM", "beforeExit", "exit"].map((signal) => process.listenerCount(signal));
${external ? "manageKernelSignalsExternally();" : ""}
installSignalHandlersOnce();
installSignalHandlersOnce();
const after = ["SIGINT", "SIGTERM", "beforeExit", "exit"].map((signal) => process.listenerCount(signal));
assert.deepEqual(after.map((count, index) => count - before[index]), ${external ? "[0, 0, 1, 1]" : "[1, 1, 1, 1]"});
assert.throws(() => manageKernelSignalsExternally(), /already installed/);
process.emit("SIGTERM");
setImmediate(() => process.stdout.write("external-owner-completes\\n"));
`;
			const child = spawn(process.execPath, ["--import", loader, "--input-type=module", "--eval", script], {
				cwd: packageRoot,
				env: { PATH: "/usr/bin:/bin", HOME: "/nonexistent", DO_NOT_TRACK: "1" },
				stdio: ["ignore", "pipe", "pipe"],
			});
			let stdout = "";
			let stderr = "";
			child.stdout.on("data", (chunk: Buffer) => {
				stdout += chunk.toString();
			});
			child.stderr.on("data", (chunk: Buffer) => {
				stderr += chunk.toString();
			});
			const [code, signal] = await once(child, "close");
			expect(stderr).toBe("");
			expect(signal).toBeNull();
			expect(code).toBe(external ? 0 : 143);
			expect(stdout).toBe(external ? "external-owner-completes\n" : "");
		},
	);
});
