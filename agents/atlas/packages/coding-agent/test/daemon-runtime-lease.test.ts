import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
	acquireSessionLease,
	SESSION_LEASE_OWNER_ID_ENV,
	SESSION_LEASES_ENABLED_ENV,
} from "../src/core/session-lease.js";
import { SessionManager } from "../src/core/session-manager.js";
import type { ActiveSessionState } from "../src/modes/daemon/active-session-state.js";
import { AgentDaemon } from "../src/modes/daemon/daemon-mode.js";
import type { DaemonCommand } from "../src/modes/daemon/daemon-protocol.js";
import { userMsg } from "./utilities.js";

describe("daemon runtime session leases", () => {
	afterEach(() => {
		vi.unstubAllEnvs();
	});

	it("releases an acquired lease when the runtime open guard cancels", async () => {
		const root = mkdtempSync(join(tmpdir(), "prime-daemon-runtime-lease-"));
		try {
			vi.stubEnv(SESSION_LEASES_ENABLED_ENV, "1");
			vi.stubEnv(SESSION_LEASE_OWNER_ID_ENV, "daemon-test");
			const sessionDir = join(root, "sessions");
			const sessionManager = SessionManager.create(root, sessionDir);
			sessionManager.appendMessage(userMsg("persist session"));
			const sessionPath = sessionManager.getSessionFile()!;
			const daemon = new AgentDaemon(join(root, "daemon.sock"), {
				defaultSessionConfig: { agentDir: root, cwd: root, sessionDir },
				createRuntime: async () => {
					throw new Error("runtime factory should not be reached");
				},
			});
			const createRuntime = (
				daemon as unknown as {
					createRuntime(
						command: Extract<DaemonCommand, { type: "create" }>,
						guard: () => Promise<boolean>,
					): Promise<ActiveSessionState>;
				}
			).createRuntime.bind(daemon);

			await expect(createRuntime({ type: "create", sessionPath }, async () => false)).rejects.toThrow();
			const reopened = acquireSessionLease(sessionPath, root);
			expect(reopened).toBeDefined();
			reopened?.release();
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});
});
