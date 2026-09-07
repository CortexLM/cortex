import { closeSync, constants, fchmodSync, fstatSync, fsyncSync, openSync, readFileSync, realpathSync } from "node:fs";
import { dirname, isAbsolute } from "node:path";
import { CURRENT_SESSION_VERSION, SessionManager } from "../core/session-manager.js";

export interface CortexSessionIdentity {
	sessionId: string;
	depth: number;
	parentSession?: string;
}

function record(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function validEntry(value: Record<string, unknown>): boolean {
	switch (value.type) {
		case "message":
			return (
				record(value.message) &&
				typeof value.message.role === "string" &&
				(typeof value.message.content === "string" || Array.isArray(value.message.content))
			);
		case "custom_message":
			return (
				typeof value.customType === "string" &&
				typeof value.display === "boolean" &&
				(typeof value.content === "string" || Array.isArray(value.content))
			);
		case "custom":
			return typeof value.customType === "string";
		case "model_change":
			return typeof value.provider === "string" && typeof value.modelId === "string";
		case "thinking_level_change":
			return typeof value.thinkingLevel === "string";
		case "service_tier_change":
			return typeof value.serviceTier === "string";
		case "compaction":
			return (
				typeof value.summary === "string" &&
				typeof value.firstKeptEntryId === "string" &&
				typeof value.tokensBefore === "number" &&
				Number.isFinite(value.tokensBefore)
			);
		case "branch_summary":
			return typeof value.summary === "string" && typeof value.fromId === "string";
		case "child_usage_attributed":
			return typeof value.targetId === "string" && record(value.childUsage) && record(value.aggregateUsage);
		case "label":
			return typeof value.targetId === "string" && (value.label === undefined || typeof value.label === "string");
		case "session_info":
			return value.name === undefined || typeof value.name === "string";
		case "session_state":
			return record(value.state) && ["active", "archived", "crash"].includes(String(value.state.status));
		case "agent_status":
			return record(value.status) && typeof value.status.summary === "string";
		case "git_state":
			return record(value.git);
		default:
			return false;
	}
}

/** Upstream repairs corrupt files by starting fresh; Cortex must instead refuse them. */
export function openCortexSession(file: string, expected: CortexSessionIdentity): SessionManager {
	if (!isAbsolute(file) || realpathSync(file) !== file) throw new Error("Invalid Cortex session path");
	const fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
	try {
		const stat = fstatSync(fd);
		if (!stat.isFile() || stat.size === 0 || stat.size > 64 * 1024 * 1024)
			throw new Error("Invalid Cortex session file");
		const text = readFileSync(fd, "utf8");
		if (!text.endsWith("\n")) throw new Error("Incomplete Cortex session checkpoint");
		const records: unknown[] = text
			.slice(0, -1)
			.split("\n")
			.map((line) => JSON.parse(line));
		const header = records[0];
		if (
			!record(header) ||
			header.type !== "session" ||
			header.id !== expected.sessionId ||
			header.version !== CURRENT_SESSION_VERSION ||
			header.rlmDepth !== expected.depth ||
			header.parentSession !== expected.parentSession ||
			typeof header.cwd !== "string" ||
			!isAbsolute(header.cwd) ||
			typeof header.timestamp !== "string" ||
			!Number.isFinite(Date.parse(header.timestamp))
		) {
			throw new Error("Invalid Cortex session header");
		}
		const ids = new Set<string>();
		for (const entry of records.slice(1)) {
			if (
				!record(entry) ||
				!validEntry(entry) ||
				typeof entry.id !== "string" ||
				entry.id.length === 0 ||
				ids.has(entry.id) ||
				typeof entry.timestamp !== "string" ||
				!Number.isFinite(Date.parse(entry.timestamp)) ||
				(entry.parentId !== null && (typeof entry.parentId !== "string" || !ids.has(entry.parentId)))
			) {
				throw new Error("Invalid Cortex session entry");
			}
			ids.add(entry.id);
		}
	} finally {
		closeSync(fd);
	}
	// The canonical directory is controller-only and the tree budget lease is held throughout opening.
	return SessionManager.open(file);
}

function syncSessionFile(file: string): void {
	const fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
	try {
		const stat = fstatSync(fd);
		if (!stat.isFile() || stat.size > 64 * 1024 * 1024) throw new Error("Invalid Cortex session file");
		fchmodSync(fd, 0o600);
		fsyncSync(fd);
	} finally {
		closeSync(fd);
	}
	const directory = openSync(dirname(file), constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
	try {
		fsyncSync(directory);
	} finally {
		closeSync(directory);
	}
}

export function flushCortexSession(manager: SessionManager): void {
	manager.flushNow();
	const file = manager.getSessionFile();
	if (!file) throw new Error("Cortex session has no persistence path");
	syncSessionFile(file);
}

/** Upstream observers swallow failures, so the owner must revoke work explicitly. */
export function persistCortexSession(manager: SessionManager, fail: () => void): () => void {
	const unsubscribe = manager.onPersist((file) => {
		try {
			syncSessionFile(file);
		} catch {
			fail();
		}
	});
	try {
		flushCortexSession(manager);
		return unsubscribe;
	} catch (error) {
		unsubscribe();
		throw error;
	}
}
