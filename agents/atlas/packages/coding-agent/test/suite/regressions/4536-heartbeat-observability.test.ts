import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { AgentCronJobStore } from "../../../src/core/cron-jobs.js";
import { createHarness, type Harness } from "../harness.js";

describe("ENG-4536 heartbeat observability", () => {
	const harnesses: Harness[] = [];

	afterEach(() => {
		while (harnesses.length > 0) harnesses.pop()?.cleanup();
	});

	it("keeps user and agent heartbeat ownership separate while allowing user management", async () => {
		const harness = await createHarness({ persistSession: true });
		harnesses.push(harness);
		const store = AgentCronJobStore.forSessionArtifacts();
		for (const sessionId of ["session-a", "session-b"]) {
			store.registerSessionArtifact(sessionId, join(harness.tempDir, "artifacts", sessionId));
		}
		const base = {
			sessionFile: join(harness.tempDir, "session.jsonl"),
			cwd: harness.tempDir,
			scheduleText: "every 5m",
		};
		const userHeartbeat = store.createHeartbeat({
			...base,
			activeSessionId: "active-a",
			sessionId: "session-a",
			prompt: "user heartbeat",
		});
		const agentHeartbeat = store.createRlmHeartbeat({
			...base,
			activeSessionId: "active-a",
			sessionId: "session-a",
			prompt: "agent heartbeat",
		});
		const otherAgentHeartbeat = store.createRlmHeartbeat({
			...base,
			activeSessionId: "active-b",
			sessionId: "session-b",
			prompt: "other agent heartbeat",
		});

		expect(store.getHeartbeat("active-a")?.id).toBe(userHeartbeat.id);
		expect(store.listRlmHeartbeats("active-a").map((job) => job.id)).toEqual([agentHeartbeat.id]);
		expect(store.updateRlmHeartbeat("active-a", userHeartbeat.id, { status: "pause" })).toBeUndefined();
		expect(store.deleteRlmHeartbeat("active-a", userHeartbeat.id)).toBeUndefined();
		expect(store.updateRlmHeartbeat("active-a", otherAgentHeartbeat.id, { status: "pause" })).toBeUndefined();

		expect(store.manageHeartbeat("active-a", userHeartbeat.id, "pause")?.status).toBe("paused");
		expect(store.manageHeartbeat("active-a", agentHeartbeat.id, "pause")?.status).toBe("paused");
		expect(store.manageHeartbeat("active-b", otherAgentHeartbeat.id, "stop")?.status).toBe("cancelled");
		expect(store.manageHeartbeat("active-a", otherAgentHeartbeat.id, "stop")).toBeUndefined();
	});

	it("emits invalidation for lifecycle changes and keeps multiple agent heartbeats", async () => {
		const harness = await createHarness({ persistSession: true });
		harnesses.push(harness);
		const store = AgentCronJobStore.forSessionArtifacts();
		store.registerSessionArtifact("session-a", join(harness.tempDir, "artifacts", "session-a"));
		let changes = 0;
		store.onHeartbeatChange(() => changes++);
		const input = {
			activeSessionId: "active-a",
			sessionId: "session-a",
			sessionFile: join(harness.tempDir, "session.jsonl"),
			cwd: harness.tempDir,
			scheduleText: "every 10m",
		};
		const user = store.createHeartbeat({ ...input, prompt: "user" });
		store.createRlmHeartbeat({ ...input, prompt: "agent one" });
		store.createRlmHeartbeat({ ...input, prompt: "agent two" });

		expect(store.getHeartbeat("active-a")?.id).toBe(user.id);
		expect(store.listRlmHeartbeats("active-a")).toHaveLength(2);
		store.manageHeartbeat("active-a", user.id, "pause");
		store.manageHeartbeat("active-a", user.id, "resume");
		store.manageHeartbeat("active-a", user.id, "stop");
		expect(changes).toBe(6);

		const cron = store.create({
			...input,
			prompt: "ordinary cron",
			now: new Date("2026-01-01T00:00:00.000Z"),
		});
		const dueAt = new Date(cron.nextRunAt!);
		const dispatch = store.claimDue(dueAt, dueAt)[0];
		expect(dispatch).toBeDefined();
		store.recordDispatchResult(dispatch!.id, { now: dueAt, outcome: "ran" });
		expect(changes).toBe(6);

		const tickingHeartbeat = store.createRlmHeartbeat({
			...input,
			prompt: "ticking heartbeat",
			now: new Date("2026-02-01T00:00:00.000Z"),
		});
		expect(changes).toBe(7);
		const heartbeatDueAt = new Date(tickingHeartbeat.nextRunAt!);
		const heartbeatDispatch = store
			.claimDue(heartbeatDueAt, heartbeatDueAt)
			.find((candidate) => candidate.job.id === tickingHeartbeat.id);
		expect(heartbeatDispatch).toBeDefined();
		store.recordDispatchResult(heartbeatDispatch!.id, { now: heartbeatDueAt, outcome: "ran" });
		expect(changes).toBe(7);
	});
});
