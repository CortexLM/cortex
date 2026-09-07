import assert from "node:assert";
import { describe, it } from "node:test";
import { stripVTControlCharacters } from "node:util";
import { Editor } from "../src/components/editor.js";
import { TUI } from "../src/tui.js";
import { defaultEditorTheme } from "./test-themes.js";
import { VirtualTerminal } from "./virtual-terminal.js";

function createTestTUI(cols = 80, rows = 24): TUI {
	return new TUI(new VirtualTerminal(cols, rows));
}

class BashPromptEditor extends Editor {
	protected override getPromptPrefix(): string {
		return this.getBashPromptInfo(this.getLines()[0] ?? "")?.promptPrefix ?? "> ";
	}

	protected override getHiddenTextPrefixLength(lineIndex: number, line: string): number {
		if (lineIndex !== 0) {
			return 0;
		}
		return this.getBashPromptInfo(line)?.hiddenTextPrefixLength ?? 0;
	}

	private getBashPromptInfo(line: string): { promptPrefix: string; hiddenTextPrefixLength: number } | undefined {
		const trimmedLine = line.trimStart();
		const leadingWhitespaceLength = line.length - trimmedLine.length;
		if (trimmedLine.startsWith("!!")) {
			return {
				promptPrefix: "!! ",
				hiddenTextPrefixLength: leadingWhitespaceLength + (trimmedLine.startsWith("!! ") ? 3 : 2),
			};
		}
		if (trimmedLine.startsWith("!")) {
			return {
				promptPrefix: "! ",
				hiddenTextPrefixLength: leadingWhitespaceLength + (trimmedLine.startsWith("! ") ? 2 : 1),
			};
		}
		return undefined;
	}
}

function inputLine(editor: Editor, width = 24): string {
	return stripVTControlCharacters(editor.render(width)[1] ?? "");
}

describe("Editor prompt prefix", () => {
	it("renders prompt prefixes in the default foreground", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("hello");

		assert.match(editor.render(24)[1] ?? "", /^> /);
	});

	it("renders the default prompt prefix before regular input", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("hello");

		assert.match(inputLine(editor), /^> hello/);
	});

	it("insets the prompt prefix one column into the surface padding", () => {
		const editor = new BashPromptEditor(createTestTUI(), {
			...defaultEditorTheme,
			backgroundColor: (text) => text,
		});

		editor.setText("hello");

		assert.match(inputLine(editor), /^ > {2}hello/);
	});

	it("uses a blank prompt gutter on wrapped rows", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("abcdefghijklmnop");
		const contentLines = editor.render(10).slice(1, -1).map(stripVTControlCharacters);

		assert.match(contentLines[0] ?? "", /^> /);
		assert.match(contentLines[1] ?? "", /^ {2}/);
		assert.doesNotMatch(contentLines[1] ?? "", /^>/);
	});

	it("renders bash prefixes in the prompt gutter and hides them from input text", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("!ls");
		assert.match(inputLine(editor), /^! ls/);
		assert.doesNotMatch(inputLine(editor), /^! !ls/);

		editor.setText("! ls");
		assert.match(inputLine(editor), /^! ls/);
		assert.doesNotMatch(inputLine(editor), /^! {2}ls/);

		editor.setText("!!pwd");
		assert.match(inputLine(editor), /^!! pwd/);
		assert.doesNotMatch(inputLine(editor), /^!! !!pwd/);

		editor.setText("!! pwd");
		assert.match(inputLine(editor), /^!! pwd/);
		assert.doesNotMatch(inputLine(editor), /^!! {2}pwd/);

		editor.setText("  !! pwd");
		assert.match(inputLine(editor), /^!! pwd/);
	});

	it("treats the hidden prefix as the visual line start", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("!echo");
		editor.handleInput("\x01");
		editor.handleInput("x");

		assert.strictEqual(editor.getText(), "!xecho");
		assert.match(inputLine(editor), /^! xecho/);
	});

	it("keeps left navigation out of hidden bash prefixes", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("!echo");
		for (let i = 0; i < 10; i++) {
			editor.handleInput("\x1b[D");
		}
		editor.handleInput("x");

		assert.strictEqual(editor.getText(), "!xecho");
		assert.match(inputLine(editor), /^! xecho/);
	});

	it("keeps word-left navigation out of hidden bash prefixes", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("! foo bar");
		for (let i = 0; i < 4; i++) {
			editor.handleInput("\x1b[1;5D");
		}
		editor.handleInput("x");

		assert.strictEqual(editor.getText(), "! xfoo bar");
		assert.match(inputLine(editor), /^! xfoo bar/);
	});

	it("keeps backspace at the visible command boundary from deleting hidden prefixes", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("!echo");
		editor.handleInput("\x01");
		editor.handleInput("\x7f");

		assert.strictEqual(editor.getText(), "!echo");
		assert.match(inputLine(editor), /^! echo/);
	});

	it("allows backspace to clear an empty bash marker", () => {
		const editor = new BashPromptEditor(createTestTUI(), defaultEditorTheme);

		editor.setText("!");
		editor.handleInput("\x7f");

		assert.strictEqual(editor.getText(), "");
		assert.match(inputLine(editor), /^> /);
	});
});
