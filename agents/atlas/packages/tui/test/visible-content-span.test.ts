import assert from "node:assert";
import { describe, it } from "node:test";
import { visibleContentSpan } from "../src/utils.js";

describe("visibleContentSpan", () => {
	it("finds content between surrounding and interior whitespace", () => {
		assert.deepStrictEqual(visibleContentSpan("  alpha beta  ", 80), { from: 2, to: 12 });
	});

	it("ignores ANSI styling and styled padding", () => {
		const line = "\x1b[48;5;236m  \x1b[1malpha\x1b[22m  \x1b[49m";
		assert.deepStrictEqual(visibleContentSpan(line, 80), { from: 2, to: 7 });
		assert.strictEqual(visibleContentSpan("\x1b[48;5;236m      \x1b[49m", 80), null);
	});

	it("measures tabs, wide graphemes, and combining marks in terminal columns", () => {
		assert.deepStrictEqual(visibleContentSpan("\talpha\t", 80), { from: 3, to: 8 });
		assert.deepStrictEqual(visibleContentSpan(" 界🙂 ", 80), { from: 1, to: 5 });
		assert.deepStrictEqual(visibleContentSpan(" e\u0301 ", 80), { from: 1, to: 2 });
	});

	it("clips the content span to the requested width", () => {
		assert.deepStrictEqual(visibleContentSpan("  alpha", 4), { from: 2, to: 4 });
		assert.deepStrictEqual(visibleContentSpan("   界", 4), { from: 3, to: 4 });
		assert.strictEqual(visibleContentSpan("alpha", 0), null);
	});
});
