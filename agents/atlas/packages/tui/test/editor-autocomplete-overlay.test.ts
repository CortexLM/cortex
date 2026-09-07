import assert from "node:assert";
import { describe, it } from "node:test";
import type { AutocompleteProvider } from "../src/autocomplete.js";
import { Editor } from "../src/components/editor.js";
import { TUI } from "../src/tui.js";
import { defaultEditorTheme } from "./test-themes.js";
import { VirtualTerminal } from "./virtual-terminal.js";

const autocompleteProvider: AutocompleteProvider = {
	async getSuggestions() {
		return {
			prefix: "/",
			items: [
				{ value: "/help", label: "help" },
				{ value: "/hotkeys", label: "hotkeys" },
			],
		};
	},
	applyCompletion(lines, cursorLine, cursorCol, item, prefix) {
		const nextLines = [...lines];
		const line = nextLines[cursorLine] ?? "";
		const before = line.slice(0, cursorCol - prefix.length);
		nextLines[cursorLine] = before + item.value + line.slice(cursorCol);
		return { lines: nextLines, cursorLine, cursorCol: before.length + item.value.length };
	},
};

async function createOpenAutocomplete(): Promise<Editor> {
	const tui = new TUI(new VirtualTerminal(40, 12));
	const editor = new Editor(tui, defaultEditorTheme);
	tui.setFocus(editor);
	editor.setAutocompleteProvider(autocompleteProvider);
	editor.handleInput("/");
	await Promise.resolve();
	await new Promise((resolve) => setImmediate(resolve));
	assert.equal(editor.isShowingAutocomplete(), true);
	return editor;
}

describe("editor autocomplete overlay", () => {
	it("anchors the overlay to the active input row", async () => {
		const editor = await createOpenAutocomplete();
		const lines = editor.render(40);

		assert.equal(lines[0]?.includes("\x1b_pi:autocomplete:"), false);
		assert.equal(lines[1]?.includes("\x1b_pi:autocomplete:"), true);
	});

	it("closes synchronously when typing leaves the completion context", async () => {
		const editor = await createOpenAutocomplete();

		editor.handleInput("/");

		assert.equal(editor.getText(), "//");
		assert.equal(editor.isShowingAutocomplete(), false);
	});

	it("keeps the overlay visible when fullscreen clips the editor top row", async () => {
		const terminal = new VirtualTerminal(40, 5);
		const tui = new TUI(terminal);
		const editor = new Editor(tui, defaultEditorTheme);
		tui.setFocus(editor);
		editor.setAutocompleteProvider(autocompleteProvider);
		tui.start();
		tui.enterFullscreen({ scroll: [], dock: editor, mouse: false });

		editor.handleInput("/");
		await Promise.resolve();
		await new Promise((resolve) => setImmediate(resolve));
		tui.requestRender(true);
		await new Promise<void>((resolve) => process.nextTick(resolve));
		await terminal.waitForRender();

		const viewport = terminal.getViewport();
		assert.ok(viewport[0]?.includes("help"));
		assert.ok(viewport[1]?.includes("hotkeys"));
		assert.ok(viewport[3]?.includes("/"));
		tui.stop();
	});

	it("keeps one suggestion visible when the editor is at the terminal top edge", async () => {
		const terminal = new VirtualTerminal(40, 3);
		const tui = new TUI(terminal);
		const editor = new Editor(tui, defaultEditorTheme);
		tui.addChild(editor);
		tui.setFocus(editor);
		editor.setAutocompleteProvider(autocompleteProvider);
		tui.start();

		editor.handleInput("/");
		await Promise.resolve();
		await new Promise((resolve) => setImmediate(resolve));
		tui.requestRender(true);
		await terminal.waitForRender();

		const viewport = terminal.getViewport();
		assert.ok(viewport[0]?.includes("hotkeys"));
		assert.ok(viewport[1]?.includes("/"));
		tui.stop();
	});
});
