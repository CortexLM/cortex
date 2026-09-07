import { type Component, type OverlayHandle, setKeybindings, type TUI } from "@earendil-works/pi-tui";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { KeybindingsManager } from "../../../src/core/keybindings.js";
import type { ModelRegistry } from "../../../src/core/model-registry.js";
import type { AgentConnectionModel } from "../../../src/modes/agent-connection/types.js";
import type { AuthenticationResult } from "../../../src/modes/interactive/auth-flows.js";
import type { ConfigurationMenuComponent } from "../../../src/modes/interactive/components/configuration-menu.js";
import { InteractiveMode } from "../../../src/modes/interactive/interactive-mode.js";
import { initTheme } from "../../../src/modes/interactive/theme/theme.js";
import { createHarness, type Harness } from "../harness.js";

interface OnboardingSplashHandle {
	showProgress(message: string): void;
	dismiss(): void;
}

interface InteractiveOnboardingHarness {
	runOnboardingFlow(showPrimeCliSplash?: boolean): Promise<void>;
	uiServices: {
		modelRegistry: ModelRegistry;
	};
	getModelCandidates(): Promise<AgentConnectionModel[]>;
	showOnboardingSplash(continueActionLabel?: string): Promise<OnboardingSplashHandle | undefined>;
	createAuthFlows(): {
		runPrimeInferenceLogin(): Promise<AuthenticationResult>;
	};
	prepareForModelSelectionAfterLogin(authResult: AuthenticationResult): Promise<boolean>;
	showConfigurationMenu(tab: "providers" | "models" | "mcp-connections"): Promise<void>;
}

interface ConfigurationHarness {
	showConfigurationMenu(tab: "providers" | "models" | "mcp-connections"): Promise<void>;
	ui: TUI;
	uiServices: {
		modelRegistry: ModelRegistry;
		settingsManager: Harness["settingsManager"];
	};
	getCachedModelCandidates(): AgentConnectionModel[];
	getScopedModelState(): [];
	getCurrentModel(): AgentConnectionModel;
	getModelSelectorRefreshPromise(): undefined;
	createAuthFlows(): {
		getLoginProviderOptions(): Array<{ id: string; name: string; authType: "api_key" }>;
		loginProvider(): Promise<AuthenticationResult>;
	};
	showFullPaneOverlay(component: Component, width: number): OverlayHandle;
	showError(message: string): void;
	showStatus(message: string): void;
}

function deferred<T>(): { promise: Promise<T>; resolve(value: T): void } {
	let resolvePromise: (value: T) => void = () => {};
	const promise = new Promise<T>((resolve) => {
		resolvePromise = resolve;
	});
	return { promise, resolve: resolvePromise };
}

describe("ENG-4658 onboarding transitions", () => {
	const harnesses: Harness[] = [];

	beforeEach(() => {
		initTheme("dark");
		setKeybindings(new KeybindingsManager());
	});

	afterEach(() => {
		for (const harness of harnesses.splice(0)) {
			harness.cleanup();
		}
	});

	test("keeps the splash mounted until first-launch model selection closes", async () => {
		const harness = await createHarness({ provider: "prime-inference", withConfiguredAuth: false });
		harnesses.push(harness);
		const order: string[] = [];
		const configuration = deferred<void>();
		const splash: OnboardingSplashHandle = {
			showProgress: (message) => order.push(`progress:${message}`),
			dismiss: () => order.push("dismiss"),
		};
		const fakeThis = Object.create(InteractiveMode.prototype) as InteractiveOnboardingHarness;
		fakeThis.uiServices = { modelRegistry: harness.session.modelRegistry };
		fakeThis.getModelCandidates = vi.fn(async () => []);
		fakeThis.showOnboardingSplash = vi.fn(async () => splash);
		fakeThis.createAuthFlows = vi.fn(() => ({
			runPrimeInferenceLogin: async (): Promise<AuthenticationResult> => {
				order.push("login");
				return {
					status: "success",
					providerId: "prime-inference",
					providerName: "Prime Inference",
					authType: "api_key",
					kind: "provider",
				};
			},
		}));
		fakeThis.prepareForModelSelectionAfterLogin = vi.fn(async () => {
			order.push("prepare");
			return true;
		});
		fakeThis.showConfigurationMenu = vi.fn((tab) => {
			order.push(`configuration:${tab}`);
			return configuration.promise;
		});

		const onboarding = fakeThis.runOnboardingFlow(false);
		await vi.waitFor(() => expect(fakeThis.showConfigurationMenu).toHaveBeenCalledWith("models"));

		expect(order).not.toContain("dismiss");
		configuration.resolve();
		await onboarding;

		expect(fakeThis.showOnboardingSplash).toHaveBeenCalledWith();
		expect(order).toEqual([
			"progress:Signing in to Prime Intellect...",
			"login",
			"progress:Preparing models...",
			"prepare",
			"configuration:models",
			"dismiss",
		]);
	});

	test("keeps the configuration overlay mounted while provider authentication is pending", async () => {
		const harness = await createHarness();
		harnesses.push(harness);
		const login = deferred<AuthenticationResult>();
		const setHidden = vi.fn();
		const hide = vi.fn();
		const focus = vi.fn();
		let menu: ConfigurationMenuComponent | undefined;
		const overlayHandle: OverlayHandle = {
			hide,
			setHidden,
			isHidden: () => false,
			focus,
			unfocus: vi.fn(),
			isFocused: () => true,
		};
		const model = harness.getModel() as AgentConnectionModel;
		const fakeThis = Object.create(InteractiveMode.prototype) as ConfigurationHarness;
		fakeThis.ui = {
			terminal: { rows: 24 },
			requestRender: vi.fn(),
		} as unknown as TUI;
		fakeThis.uiServices = {
			modelRegistry: harness.session.modelRegistry,
			settingsManager: harness.settingsManager,
		};
		fakeThis.getCachedModelCandidates = () => [model];
		fakeThis.getScopedModelState = () => [];
		fakeThis.getCurrentModel = () => model;
		fakeThis.getModelSelectorRefreshPromise = () => undefined;
		fakeThis.createAuthFlows = () => ({
			getLoginProviderOptions: () => [{ id: model.provider, name: model.provider, authType: "api_key" }],
			loginProvider: () => login.promise,
		});
		fakeThis.showFullPaneOverlay = (component) => {
			menu = component as ConfigurationMenuComponent;
			return overlayHandle;
		};
		fakeThis.showError = vi.fn();
		fakeThis.showStatus = vi.fn();

		const configuration = fakeThis.showConfigurationMenu("providers");
		expect(menu).toBeDefined();
		menu?.handleInput("\r");

		expect(setHidden).not.toHaveBeenCalled();
		expect(hide).not.toHaveBeenCalled();

		login.resolve({ status: "cancelled" });
		await vi.waitFor(() => expect(focus).toHaveBeenCalled());
		menu?.handleInput("\x1b");
		await expect(configuration).resolves.toBeUndefined();
		expect(hide).toHaveBeenCalledOnce();
	});
});
