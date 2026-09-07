import assert from "node:assert";
import { afterEach, describe, it } from "node:test";
import {
	bestAnsiColor,
	blendColor,
	clearDefaultTerminalColors,
	detectBackgroundFromColorFgBg,
	getTerminalBackgroundKind,
	isLightColor,
	parseOscColorResponse,
	rgbTo256,
	rgbToHex,
	setDefaultTerminalColors,
} from "../src/terminal-colors.js";

describe("terminal color utilities", () => {
	afterEach(() => {
		clearDefaultTerminalColors();
	});

	it("parses OSC 10/11 RGB responses with ST or BEL terminators", () => {
		assert.deepStrictEqual(parseOscColorResponse("\x1b]10;rgb:ffff/8000/0000\x1b\\"), {
			kind: "foreground",
			rgb: { r: 255, g: 128, b: 0 },
		});
		assert.deepStrictEqual(parseOscColorResponse("\x1b]10;rgb:fff/800/000\x1b\\"), {
			kind: "foreground",
			rgb: { r: 255, g: 128, b: 0 },
		});
		assert.deepStrictEqual(parseOscColorResponse("\x1b]11;rgb:00/5f/87\x07"), {
			kind: "background",
			rgb: { r: 0, g: 95, b: 135 },
		});
	});

	it("ignores invalid OSC color responses", () => {
		assert.strictEqual(parseOscColorResponse("\x1b]12;rgb:ffff/ffff/ffff\x1b\\"), undefined);
		assert.strictEqual(parseOscColorResponse("\x1b]11;not-a-color\x1b\\"), undefined);
	});

	it("classifies lightness and blends colors", () => {
		assert.strictEqual(isLightColor({ r: 255, g: 255, b: 255 }), true);
		assert.strictEqual(isLightColor({ r: 0, g: 0, b: 0 }), false);
		assert.deepStrictEqual(blendColor({ r: 255, g: 255, b: 255 }, { r: 0, g: 0, b: 0 }, 0.12), {
			r: 31,
			g: 31,
			b: 31,
		});
		assert.deepStrictEqual(blendColor({ r: 0, g: 0, b: 0 }, { r: 255, g: 255, b: 255 }, 0.04), {
			r: 245,
			g: 245,
			b: 245,
		});
	});

	it("maps RGB colors to truecolor and 256-color values", () => {
		assert.strictEqual(rgbToHex({ r: 0, g: 95, b: 135 }), "#005f87");
		assert.strictEqual(rgbTo256({ r: 0, g: 95, b: 135 }), 24);
		assert.strictEqual(bestAnsiColor({ r: 0, g: 95, b: 135 }, "truecolor"), "#005f87");
		assert.strictEqual(bestAnsiColor({ r: 0, g: 95, b: 135 }, "256color"), 24);
		assert.strictEqual(bestAnsiColor({ r: 0, g: 95, b: 135 }, "ansi16"), "");
	});

	it("uses probed background before COLORFGBG fallback", () => {
		const previous = process.env.COLORFGBG;
		process.env.COLORFGBG = "0;15";
		try {
			assert.strictEqual(detectBackgroundFromColorFgBg(), "light");
			assert.strictEqual(getTerminalBackgroundKind(), "light");
			setDefaultTerminalColors({
				foreground: { r: 255, g: 255, b: 255 },
				background: { r: 0, g: 0, b: 0 },
			});
			assert.strictEqual(getTerminalBackgroundKind(), "dark");
		} finally {
			if (previous === undefined) {
				delete process.env.COLORFGBG;
			} else {
				process.env.COLORFGBG = previous;
			}
		}
	});
});
