import { beforeAll, describe, expect, it } from "vitest";
import { IPythonCellComponent } from "../src/modes/interactive/components/ipython-cell.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";
import {
	setWorkingPulseFrame,
	WORKING_ICON_FRAMES,
	workingIconFrame,
} from "../src/modes/interactive/theme/working-icon.js";

function stripAnsi(text: string): string {
	return text.replace(/\x1b\[[0-9;]*m/g, "");
}

describe("workingIconFrame", () => {
	it("cycles through the frames and wraps in both directions", () => {
		for (let i = 0; i < WORKING_ICON_FRAMES.length; i++) {
			expect(workingIconFrame(i)).toBe(WORKING_ICON_FRAMES[i]);
		}
		expect(workingIconFrame(WORKING_ICON_FRAMES.length)).toBe(WORKING_ICON_FRAMES[0]);
		expect(workingIconFrame(-1)).toBe(WORKING_ICON_FRAMES[WORKING_ICON_FRAMES.length - 1]);
	});
});

describe("IPythonCellComponent running marker", () => {
	beforeAll(() => {
		initTheme("dark");
	});

	it("animates the marker while running and stays static once done", () => {
		const running = new IPythonCellComponent({ code: "print(1)", executionStarted: true, isPartial: true });
		setWorkingPulseFrame(0);
		const frame0 = stripAnsi(running.render(80).join("\n"));
		setWorkingPulseFrame(1);
		const frame1 = stripAnsi(running.render(80).join("\n"));
		expect(frame0).toContain(workingIconFrame(0));
		expect(frame1).toContain(workingIconFrame(1));
		expect(frame0).not.toBe(frame1);

		const done = new IPythonCellComponent({
			code: "print(1)",
			executionStarted: true,
			isPartial: false,
			details: { status: "ok", result: "1" },
		});
		setWorkingPulseFrame(0);
		const doneFrame0 = stripAnsi(done.render(80).join("\n"));
		setWorkingPulseFrame(1);
		const doneFrame1 = stripAnsi(done.render(80).join("\n"));
		expect(doneFrame0).toBe(doneFrame1);
		expect(doneFrame0).toContain("✓");
	});
});
