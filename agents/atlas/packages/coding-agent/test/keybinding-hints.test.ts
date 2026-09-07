import { describe, expect, it } from "vitest";
import { formatKeyText } from "../src/modes/interactive/components/keybinding-hints.js";

describe("keybinding hint formatting", () => {
	it("uses macOS modifier names on darwin but keeps Ctrl literal", () => {
		expect(formatKeyText("ctrl+p", "darwin")).toBe("Ctrl+P");
		expect(formatKeyText("alt+enter", "darwin")).toBe("Option+Enter");
		expect(formatKeyText("shift+ctrl+p/alt+up", "darwin")).toBe("Shift+Ctrl+P/Option+↑");
	});

	it("keeps canonical modifier names on linux", () => {
		expect(formatKeyText("ctrl+p", "linux")).toBe("Ctrl+P");
		expect(formatKeyText("alt+enter", "linux")).toBe("Alt+Enter");
		expect(formatKeyText("shift+ctrl+p/alt+up", "linux")).toBe("Shift+Ctrl+P/Alt+↑");
	});

	it("keeps canonical modifier names on Windows", () => {
		expect(formatKeyText("ctrl+p", "win32")).toBe("Ctrl+P");
		expect(formatKeyText("alt+enter", "win32")).toBe("Alt+Enter");
		expect(formatKeyText("ctrl+backspace/pageUp", "win32")).toBe("Ctrl+Backspace/PageUp");
	});

	it("formats escape as Esc", () => {
		expect(formatKeyText("escape", "linux")).toBe("Esc");
		expect(formatKeyText("esc", "linux")).toBe("Esc");
		expect(formatKeyText("ctrl+escape", "linux")).toBe("Ctrl+Esc");
	});

	it("formats arrow keys as symbols", () => {
		expect(formatKeyText("up/down/left/right", "linux")).toBe("↑/↓/←/→");
	});
});
