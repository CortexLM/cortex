import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, describe, expect, test } from "vitest";
import { APP_NAME } from "../src/config.js";
import { formatResumeHint } from "../src/modes/interactive/resume-hint.js";

const SESSION_ID = "0196c2e4-7f01-7abc-8def-0123456789ab";

const sessionDir = mkdtempSync(join(tmpdir(), "resume-hint-test-"));
const existingSessionFile = join(sessionDir, `${SESSION_ID}.jsonl`);
writeFileSync(existingSessionFile, `{"type":"session","id":"${SESSION_ID}"}\n`);

afterAll(() => {
	rmSync(sessionDir, { recursive: true, force: true });
});

describe("formatResumeHint", () => {
	test("returns hint with --resume and the session id for a persisted session", () => {
		const hint = formatResumeHint({
			sessionId: SESSION_ID,
			sessionFile: existingSessionFile,
			userMessages: 3,
		});
		expect(hint).toBeDefined();
		expect(hint).toContain(`${APP_NAME} --resume ${SESSION_ID}`);
	});

	test("returns undefined for an in-memory session", () => {
		const hint = formatResumeHint({
			sessionId: SESSION_ID,
			sessionFile: undefined,
			userMessages: 3,
		});
		expect(hint).toBeUndefined();
	});

	test("returns undefined when the session has no user messages", () => {
		const hint = formatResumeHint({
			sessionId: SESSION_ID,
			sessionFile: existingSessionFile,
			userMessages: 0,
		});
		expect(hint).toBeUndefined();
	});

	test("returns undefined when the session file was never flushed to disk", () => {
		const hint = formatResumeHint({
			sessionId: SESSION_ID,
			sessionFile: join(sessionDir, "never-written.jsonl"),
			userMessages: 1,
		});
		expect(hint).toBeUndefined();
	});

	test("returns undefined when stats are unavailable", () => {
		expect(formatResumeHint(undefined)).toBeUndefined();
	});
});
