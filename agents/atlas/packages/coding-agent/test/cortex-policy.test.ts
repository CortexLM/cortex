import { describe, expect, it } from "vitest";
import {
	authorizeOperation,
	type BudgetCheckpoint,
	type CortexScope,
	cortexPrompt,
	TreeBudget,
} from "../src/cortex/policy.js";

const atlas: CortexScope = { role: "atlas", id: "round_1", commitment: "a".repeat(64) };
const limits = {
	maxDepth: 2,
	maxChildren: 3,
	maxConcurrentCalls: 2,
	maxCalls: 4,
	maxReservedTokens: 100,
	maxReservedMicroUsd: 100,
	timeoutMs: 1000,
};

describe("Cortex authority outside the model", () => {
	it("keeps Atlas out of compute, spending and mutable experiment tools", () => {
		for (const operation of ["execute", "kernel", "quote", "approve", "rent", "delete", "sign"]) {
			expect(() => authorizeOperation(atlas, operation)).toThrow();
		}
		expect(() => authorizeOperation(atlas, "history")).not.toThrow();
		expect(() => authorizeOperation(atlas, "submit_decision")).not.toThrow();
		expect(() => authorizeOperation({ ...atlas, role: "experiment" }, "submit_decision")).toThrow();
		expect(cortexPrompt(atlas)).toContain("Atlas");
	});

	it("shares reservations between children, releases concurrency only once", () => {
		const budget = new TreeBudget(limits, () => 0);
		const release = budget.reserveCall(40);
		budget.reserveCall(40);
		expect(() => budget.reserveCall(1)).toThrow();
		release();
		release();
		budget.reserveCall(20);
		expect(budget.snapshot().active).toBe(2);
		expect(budget.snapshot().reservedTokens).toBe(100);
		expect(() => budget.reserveCall(1)).toThrow();
	});

	it("bounds recursion, elapsed time and revoked capabilities", () => {
		let now = 0;
		const budget = new TreeBudget(limits, () => now);
		budget.admitChild(1);
		expect(() => budget.admitChild(3)).toThrow();
		now = 1000;
		expect(() => budget.reserveCall(1)).toThrow();
		const revoked = new TreeBudget(limits);
		revoked.revoke();
		expect(() => revoked.admitChild(1)).toThrow();
	});

	it("resumes reservations and deadlines without renewing the allocation", () => {
		let saved: BudgetCheckpoint | undefined;
		let now = 0;
		const journal = {
			binding: "a".repeat(64),
			assertActive: () => {},
			release: () => {},
			load: () => saved,
			save: (state: BudgetCheckpoint) => {
				saved = structuredClone(state);
			},
		};
		const first = new TreeBudget(limits, () => now, journal);
		first.reserveCall(60, 90)();
		first.admitChild(1);
		now = 500;
		const resumed = new TreeBudget(limits, () => now, journal);
		expect(resumed.remainingMs()).toBe(500);
		expect(resumed.snapshot().reservedTokens).toBe(60);
		expect(resumed.snapshot().children).toBe(1);
		expect(() => resumed.reserveCall(1, 11)).toThrow();
		expect(() => resumed.admitChild(-1)).toThrow();
		resumed.revoke();
		expect(() => new TreeBudget(limits, () => now, journal).assertActive()).toThrow();
	});

	it("refuses work when the durable budget cannot be updated", () => {
		let fail = false;
		const budget = new TreeBudget(limits, () => 0, {
			binding: "a".repeat(64),
			assertActive: () => {},
			release: () => {},
			load: () => undefined,
			save: () => {
				if (fail) throw new Error("disk full");
			},
		});
		fail = true;
		expect(() => budget.reserveCall(1)).toThrow(/persist/);
		expect(() => budget.assertActive()).toThrow(/revoked/);
		expect(budget.snapshot().active).toBe(0);
	});

	it("copies immutable limits instead of trusting mutable caller configuration", () => {
		const mutable = { ...limits };
		const budget = new TreeBudget(mutable, () => 0);
		mutable.maxCalls = 1000;
		expect(budget.limits.maxCalls).toBe(limits.maxCalls);
		expect(() => {
			budget.limits.maxCalls = 1000;
		}).toThrow();
	});
});
