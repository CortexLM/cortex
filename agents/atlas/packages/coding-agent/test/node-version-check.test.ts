import { describe, expect, test } from "vitest";
import { assertNodeVersion, MIN_NODE_VERSION } from "../src/cli/node-version-check.js";

function run(version: string) {
	const logs: string[] = [];
	let exitCode: number | null = null;
	const ok = assertNodeVersion({
		version,
		log: (m) => logs.push(m),
		exit: (code) => {
			exitCode = code;
		},
	});
	return { ok, logs, exitCode };
}

describe("assertNodeVersion", () => {
	test("passes on the minimum supported version", () => {
		const { ok, logs, exitCode } = run(MIN_NODE_VERSION);
		expect(ok).toBe(true);
		expect(exitCode).toBeNull();
		expect(logs).toHaveLength(0);
	});

	test("passes on a newer minor", () => {
		const { ok, exitCode } = run("22.9.0");
		expect(ok).toBe(true);
		expect(exitCode).toBeNull();
	});

	test("passes on a newer patch", () => {
		const { ok, exitCode } = run("22.8.1");
		expect(ok).toBe(true);
		expect(exitCode).toBeNull();
	});

	test("passes on a newer major", () => {
		const { ok, exitCode } = run("25.9.0");
		expect(ok).toBe(true);
		expect(exitCode).toBeNull();
	});

	test("rejects a Node 22 release below the minimum", () => {
		const { ok, logs, exitCode } = run("22.7.0");
		expect(ok).toBe(false);
		expect(exitCode).toBe(1);
		expect(logs.join("\n")).toContain(`Node ${MIN_NODE_VERSION}`);
	});

	test("rejects an outdated major with guidance and exit 1", () => {
		const { ok, logs, exitCode } = run("20.18.1");
		expect(ok).toBe(false);
		expect(exitCode).toBe(1);
		const text = logs.join("\n");
		expect(text).toContain(`Node ${MIN_NODE_VERSION}`);
		expect(text).toContain("20.18.1");
		expect(text).toContain("github.com/PrimeIntellect-ai/prime-agent/releases/latest");
	});

	test("accepts the v prefix used by process.version", () => {
		const { ok, exitCode } = run(`v${MIN_NODE_VERSION}`);
		expect(ok).toBe(true);
		expect(exitCode).toBeNull();
	});

	test("accepts build metadata at the minimum version", () => {
		const { ok, exitCode } = run(`${MIN_NODE_VERSION}+build.1`);
		expect(ok).toBe(true);
		expect(exitCode).toBeNull();
	});

	test("rejects a prerelease of the minimum version", () => {
		const { ok, exitCode } = run(`${MIN_NODE_VERSION}-rc.1`);
		expect(ok).toBe(false);
		expect(exitCode).toBe(1);
	});

	test("lets an unparseable version through rather than blocking", () => {
		const { ok, exitCode } = run("not-a-version");
		expect(ok).toBe(true);
		expect(exitCode).toBeNull();
	});
});
