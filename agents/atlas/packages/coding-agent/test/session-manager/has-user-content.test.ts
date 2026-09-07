import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { SessionManager } from "../../src/core/session-manager.js";
import { userMsg } from "../utilities.js";

describe("SessionManager.hasUserContent", () => {
	function withSession(run: (session: SessionManager) => void): void {
		const tempDir = mkdtempSync(join(tmpdir(), "has-user-content-"));
		try {
			run(SessionManager.create(join(tempDir, "project"), join(tempDir, "sessions")));
		} finally {
			rmSync(tempDir, { recursive: true, force: true });
		}
	}

	it("is false for a fresh session with only the default configuration entries", () => {
		withSession((session) => {
			// What createAgentSession writes for every new session.
			session.appendModelChange("anthropic", "claude-opus-4-8");
			session.appendThinkingLevelChange("off");
			session.appendServiceTierChange("default");
			expect(session.hasUserContent()).toBe(false);
		});
	});

	it("is true once the user changes the model after creation", () => {
		withSession((session) => {
			session.appendModelChange("anthropic", "claude-opus-4-8");
			session.appendThinkingLevelChange("off");
			session.appendModelChange("openai", "gpt-5");
			expect(session.hasUserContent()).toBe(true);
		});
	});

	it("is true once the user changes the thinking level after creation", () => {
		withSession((session) => {
			session.appendModelChange("anthropic", "claude-opus-4-8");
			session.appendThinkingLevelChange("off");
			session.appendThinkingLevelChange("high");
			expect(session.hasUserContent()).toBe(true);
		});
	});

	it("is true once the user enables Fast mode after creation", () => {
		withSession((session) => {
			session.appendModelChange("openai-codex", "gpt-5.5");
			session.appendThinkingLevelChange("medium");
			session.appendServiceTierChange("default");
			session.appendServiceTierChange("priority");
			expect(session.hasUserContent()).toBe(true);
		});
	});

	it("is false for a fresh session created with no model available (thinking entry only)", () => {
		withSession((session) => {
			// createAgentSession skips the model_change when no model is configured,
			// leaving a lone leading thinking_level_change as the creation default.
			session.appendThinkingLevelChange("off");
			expect(session.hasUserContent()).toBe(false);
		});
	});

	it("is true once the user changes the thinking level on a no-model session", () => {
		withSession((session) => {
			session.appendThinkingLevelChange("off");
			session.appendThinkingLevelChange("high");
			expect(session.hasUserContent()).toBe(true);
		});
	});

	it("is true for a session with a message", () => {
		withSession((session) => {
			session.appendMessage(userMsg("hello"));
			expect(session.hasUserContent()).toBe(true);
		});
	});

	it("is true once the session is named", () => {
		withSession((session) => {
			session.appendModelChange("anthropic", "claude-opus-4-8");
			session.appendThinkingLevelChange("off");
			session.appendSessionInfo("my draft");
			expect(session.hasUserContent()).toBe(true);
		});
	});
});
