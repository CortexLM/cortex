import { mkdir, mkdtemp, readFile, rm, stat, symlink, truncate, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { CURRENT_SESSION_VERSION, SessionManager } from "../src/core/session-manager.js";
import { flushCortexSession, openCortexSession, persistCortexSession } from "../src/cortex/session-file.js";

const directories: string[] = [];
const expected = { sessionId: "test-root", depth: 0 };
const header = {
	type: "session",
	id: expected.sessionId,
	version: CURRENT_SESSION_VERSION,
	timestamp: new Date(0).toISOString(),
	cwd: "/work",
	rlmDepth: 0,
};
const entry = { type: "custom", customType: "test", id: "first", parentId: null, timestamp: header.timestamp };
const jsonl = (...values: unknown[]) => `${values.map((value) => JSON.stringify(value)).join("\n")}\n`;

async function fixture(content: string) {
	const dir = await mkdtemp(join(tmpdir(), "cortex-session-file-"));
	directories.push(dir);
	const file = join(dir, "session.jsonl");
	await writeFile(file, content);
	return file;
}

afterEach(async () => {
	for (const dir of directories.splice(0)) await rm(dir, { recursive: true, force: true });
});

describe("Cortex strict session checkpoints", () => {
	it("opens a valid current-version tree without changing its transcript", async () => {
		const text = jsonl(header, entry, { ...entry, id: "second", parentId: "first" });
		const file = await fixture(text);
		expect(openCortexSession(file, expected).getEntries()).toHaveLength(2);
		expect(await readFile(file, "utf8")).toBe(text);
	});

	it.each([
		["empty", ""],
		["malformed", "{\n"],
		["truncated", jsonl(header, entry).slice(0, -1)],
		["blank entry", `${jsonl(header)}\n`],
		["wrong id", jsonl({ ...header, id: "other" })],
		["old version", jsonl({ ...header, version: 1 })],
		["wrong depth", jsonl({ ...header, rlmDepth: 1 })],
		["wrong parent", jsonl({ ...header, parentSession: "/other" })],
		["duplicate id", jsonl(header, entry, entry)],
		["self edge", jsonl(header, { ...entry, parentId: "first" })],
		["missing predecessor", jsonl(header, { ...entry, parentId: "absent" })],
		["duplicate header", jsonl(header, header)],
		["invalid payload", jsonl(header, { ...entry, type: "message", message: null })],
		["unknown type", jsonl(header, { ...entry, type: "unknown" })],
	])("rejects %s and preserves the rejected bytes", async (_name, text) => {
		const file = await fixture(text);
		expect(() => openCortexSession(file, expected)).toThrow();
		expect(await readFile(file, "utf8")).toBe(text);
	});

	it("rejects symlinks, nonregular files and oversized checkpoints", async () => {
		const file = await fixture(jsonl(header));
		const link = `${file}.link`;
		await symlink(file, link);
		expect(() => openCortexSession(link, expected)).toThrow(/path/);
		const dir = `${file}.dir`;
		await mkdir(dir);
		expect(() => openCortexSession(dir, expected)).toThrow(/file/);
		await truncate(file, 64 * 1024 * 1024 + 1);
		expect(() => openCortexSession(file, expected)).toThrow(/file/);
	});

	it("flushes pre-inference entries privately and signals failed persistence observers", async () => {
		const file = await fixture(jsonl(header));
		const manager = SessionManager.open(file);
		let failed = false;
		const unsubscribe = persistCortexSession(manager, () => {
			failed = true;
		});
		manager.appendMessage({ role: "user", content: "Persist before inference", timestamp: 1 });
		flushCortexSession(manager);
		expect(openCortexSession(file, expected).getEntries()).toHaveLength(1);
		expect((await stat(file)).mode & 0o777).toBe(0o600);
		await truncate(file, 64 * 1024 * 1024 + 1);
		manager.appendSessionInfo("Cannot acknowledge an oversized transcript");
		expect(failed).toBe(true);
		unsubscribe();
	});
});
