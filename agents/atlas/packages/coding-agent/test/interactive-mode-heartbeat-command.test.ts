import type { AutocompleteItem } from "@earendil-works/pi-tui";
import { describe, expect, it } from "vitest";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";

type InteractiveModePrototype = {
	getHeartbeatArgumentCompletions(prefix: string): AutocompleteItem[] | null;
};

const interactiveModePrototype = InteractiveMode.prototype as unknown as InteractiveModePrototype;

describe("InteractiveMode /heartbeat", () => {
	describe("argument autocomplete", () => {
		it("lists the interval and delivery-mode starters for an empty prefix", () => {
			const items = interactiveModePrototype.getHeartbeatArgumentCompletions("");

			expect(items?.map((item) => item.label)).toEqual([
				"every <duration> <instruction>",
				"--steer <instruction>",
				"--follow-up <instruction>",
			]);
			const everyItem = items?.find((item) => item.label === "every <duration> <instruction>");
			expect(everyItem?.value).toBe("every ");
			expect(everyItem?.description).toBe(
				"Set an interval, then add an instruction: /heartbeat every 10s Scan the logs",
			);
			const followUpItem = items?.find((item) => item.label === "--follow-up <instruction>");
			expect(followUpItem?.value).toBe("--follow-up ");
		});

		it("filters delivery-mode flags by keyword", () => {
			const items = interactiveModePrototype.getHeartbeatArgumentCompletions("--fol");

			expect(items?.map((item) => item.label)).toEqual(["--follow-up <instruction>"]);
		});

		it("does not autocomplete lifecycle commands over free-form instructions", () => {
			const items = interactiveModePrototype.getHeartbeatArgumentCompletions("st");

			expect(items).toBeNull();
		});

		it("filters interval syntax by keyword", () => {
			const items = interactiveModePrototype.getHeartbeatArgumentCompletions("every");

			expect(items?.map((item) => item.label)).toEqual(["every <duration> <instruction>"]);
		});
	});
});
