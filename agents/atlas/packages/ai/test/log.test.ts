import { afterEach, describe, expect, test, vi } from "vitest";
import { getLogger, type LogEntry, setLogSink, stringifyLogEntry } from "../src/log.js";

afterEach(() => {
	setLogSink(undefined);
	vi.restoreAllMocks();
});

describe("structured logger", () => {
	test("routes entries with component, level, and fields to the sink", () => {
		const entries: LogEntry[] = [];
		setLogSink((entry) => entries.push(entry));

		const log = getLogger("test.component");
		log.info("hello", { a: 1 });
		log.error("boom", { requestId: "req_123" });

		expect(entries).toHaveLength(2);
		expect(entries[0]).toMatchObject({ level: "info", component: "test.component", msg: "hello", a: 1 });
		expect(entries[1]).toMatchObject({ level: "error", msg: "boom", requestId: "req_123" });
		expect(new Date(entries[0].ts).getTime()).not.toBeNaN();
	});

	test("without a sink, warn/error fall back to console.error and debug/info are dropped", () => {
		const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
		const log = getLogger("test");
		log.debug("quiet");
		log.info("quiet");
		log.warn("loud");
		log.error("loud");
		expect(consoleError).toHaveBeenCalledTimes(2);
	});

	test("caller fields cannot overwrite reserved entry keys", () => {
		const entries: LogEntry[] = [];
		setLogSink((entry) => entries.push(entry));
		getLogger("test").error("boom", { level: "info", msg: "spoofed", detail: "kept" });
		expect(entries[0]).toMatchObject({ level: "error", msg: "boom", detail: "kept" });
	});

	test("stringifyLogEntry survives circular refs and BigInt fields", () => {
		const circular: Record<string, unknown> = {};
		circular.self = circular;
		const entry: LogEntry = { ts: "t", level: "error", component: "c", msg: "m", circular, big: 1n };
		const line = stringifyLogEntry(entry);
		expect(JSON.parse(line)).toMatchObject({ msg: "m", circular: { self: "[Circular]" }, big: "1" });
	});

	test("a throwing sink never propagates and warn/error fall back to console.error", () => {
		const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
		setLogSink(() => {
			throw new Error("sink is broken");
		});
		const log = getLogger("test");
		expect(() => log.error("boom")).not.toThrow();
		expect(() => log.debug("quiet")).not.toThrow();
		expect(consoleError).toHaveBeenCalledTimes(1);
	});
});
