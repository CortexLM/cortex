import { setKeybindings, visibleWidth } from "@earendil-works/pi-tui";
import stripAnsi from "strip-ansi";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { KeybindingsManager } from "../src/core/keybindings.js";
import { PrimeOnboardingSplashComponent } from "../src/modes/interactive/components/prime-onboarding-splash.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";
import { PRIME_BUTTERFLY_LOGO } from "../src/themes/prime-logo.js";

describe("PrimeOnboardingSplashComponent", () => {
	beforeAll(() => {
		initTheme("dark");
	});

	beforeEach(() => {
		setKeybindings(new KeybindingsManager());
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	it("renders a minimal first-run login action", () => {
		const component = new PrimeOnboardingSplashComponent(
			() => {},
			() => {},
			{ getRows: () => 36 },
		);
		const lines = component.render(100);
		const output = stripAnsi(lines.join("\n"));

		expect(lines).toHaveLength(36);
		expect(output).toContain("Welcome to PRIME Agent");
		expect(output).toContain("Press Enter to login with Prime Intellect");
		expect(output).toContain("·");
		expect(output).not.toContain("prime agent");
		expect(output).not.toContain("Research and infrastructure assistant for high-context work.");
		expect(output).not.toContain("• Inspect logs, evals, training runs, and environments.");
		expect(output).not.toContain("• Keep context alive in Python state and artifacts.");
		expect(output).not.toContain("• Delegate focused work through recursive RLM calls.");
		expect(output).not.toContain("long-context coding tasks");
		expect(output).not.toContain("Login with Prime Intellect");
		expect(output).not.toContain("████▀▀▀██▄");
		expect(output).not.toContain("Get Started");
		expect(output).not.toContain("One account for models, inference, and coding sessions.");
		expect(output).not.toContain("Choose your model and start building.");
		expect(output).not.toContain("╭───╮");
		expect(output).not.toContain("PRIME INTELLECT");
		expect(output).not.toContain("cancel");
		expect(output).not.toContain("A coding agent connected to Prime Intellect.");
		expect(output).not.toContain("Continue with Prime Intellect");
		expect(output).not.toContain("Use one account for managed inference, model access, and usage.");
		expect(output).not.toContain("/* BUILD */");
		expect(output).not.toContain("/* EVALUATE */");
		expect(output).not.toContain("/* TRAIN */");
		expect(output).not.toContain("/* DEPLOY */");
		expect(output).not.toContain("required for first-time setup");
		expect(output).not.toContain("Start with your Prime Intellect account.");
		expect(output).not.toContain("Log in with Prime Intellect");
		expect(output).not.toContain("→");
		expect(output).not.toContain("Use a subscription");
		expect(output).not.toContain("Use an API key");
		expect(output).toContain(PRIME_BUTTERFLY_LOGO.split("\n")[0].trim());
		for (const line of lines) {
			expect(visibleWidth(line)).toBeLessThanOrEqual(100);
		}
	});

	it("starts Prime login on confirm", () => {
		let selected = false;
		const component = new PrimeOnboardingSplashComponent(
			() => {
				selected = true;
			},
			() => {},
		);

		component.handleInput("\r");

		expect(selected).toBe(true);
	});

	it("renders a model selection action when auth is already available", () => {
		const component = new PrimeOnboardingSplashComponent(
			() => {},
			() => {},
			{ getRows: () => 36, continueActionLabel: "choose a model" },
		);
		const output = stripAnsi(component.render(100).join("\n"));

		expect(output).toContain("Press Enter to choose a model");
		expect(output).not.toContain("Press Enter to login with Prime Intellect");
	});

	it("shows progress and ignores input while onboarding advances", () => {
		const onSelect = vi.fn();
		const onCancel = vi.fn();
		const component = new PrimeOnboardingSplashComponent(onSelect, onCancel, { getRows: () => 36 });

		component.showProgress("Preparing models...");
		component.handleInput("\r");
		component.handleInput("\x1b");

		const output = stripAnsi(component.render(100).join("\n"));
		expect(output).toContain("Preparing models...");
		expect(output).not.toContain("Press Enter");
		expect(onSelect).not.toHaveBeenCalled();
		expect(onCancel).not.toHaveBeenCalled();
	});

	it("animates the splash at an interactive cadence", () => {
		vi.useFakeTimers();
		let renderRequests = 0;
		const component = new PrimeOnboardingSplashComponent(
			() => {},
			() => {},
			{
				getRows: () => 36,
				requestRender: () => {
					renderRequests++;
				},
				animationIntervalMs: 20,
			},
		);

		const firstRender = stripAnsi(component.render(100).join("\n"));
		vi.advanceTimersByTime(60);
		const secondRender = stripAnsi(component.render(100).join("\n"));
		component.dispose();

		expect(renderRequests).toBe(3);
		expect(secondRender).not.toBe(firstRender);
		expect(secondRender).toContain("Welcome to PRIME Agent");
		expect(secondRender).toContain("Press Enter to login with Prime Intellect");
	});

	it("centers stacked content in narrow terminals", () => {
		const component = new PrimeOnboardingSplashComponent(
			() => {},
			() => {},
			{ getRows: () => 40 },
		);
		const rendered = component.render(60).map((line) => stripAnsi(line));
		const logoLine = rendered.find((line) => line.includes(PRIME_BUTTERFLY_LOGO.split("\n")[0].trim()));
		const brandLine = rendered.find((line) => line.includes("Welcome to PRIME Agent"));
		const hintLine = rendered.find((line) => line.includes("Press Enter to login with Prime Intellect"));

		expect(logoLine?.search(/\S/)).toBeGreaterThan(0);
		expect(brandLine?.search(/\S/)).toBeGreaterThan(0);
		expect(hintLine?.search(/\S/)).toBeGreaterThan(0);
	});
});
