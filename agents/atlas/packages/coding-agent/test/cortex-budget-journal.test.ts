import { mkdtemp, readFile, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { FileBudgetJournal } from "../src/cortex/budget-journal.js";
import { TreeBudget } from "../src/cortex/policy.js";

const limits = {
	maxDepth: 2,
	maxChildren: 3,
	maxConcurrentCalls: 2,
	maxCalls: 10,
	maxReservedTokens: 100,
	maxReservedMicroUsd: 100,
	timeoutMs: 10_000,
};
const binding = "a".repeat(64);

describe("Cortex file budget ownership", () => {
	it("excludes concurrent controllers and preserves reservations after release", async () => {
		const dir = await mkdtemp(join(tmpdir(), "cortex-budget-"));
		const path = join(dir, "budget.json");
		const journal = new FileBudgetJournal(path, binding);
		try {
			const first = new TreeBudget(limits, () => 1000, journal);
			const release = first.reserveCall(40, 60);
			expect(() => new FileBudgetJournal(path, binding)).toThrow(/already active/);
			expect(() => first.release()).toThrow(/still active/);
			release();
			first.release();
			expect(() => first.reserveCall(1)).toThrow(/lease/);
			const next = new FileBudgetJournal(path, binding);
			try {
				const resumed = new TreeBudget(limits, () => 2000, next);
				expect(resumed.remainingMs()).toBe(9000);
				expect(resumed.snapshot().calls).toBe(1);
				expect(resumed.snapshot().reservedTokens).toBe(40);
				expect(() => resumed.reserveCall(1, 41)).toThrow(/budget/);
				resumed.revoke();
			} finally {
				next.release();
			}
			const final = new FileBudgetJournal(path, binding);
			try {
				expect(() => new TreeBudget(limits, () => 3000, final).assertActive()).toThrow(/revoked/);
			} finally {
				final.release();
			}
			expect((await stat(path)).mode & 0o777).toBe(0o600);
		} finally {
			journal.release();
			await rm(dir, { recursive: true, force: true });
		}
	});

	it("rejects a different scope, corrupt data, changed limits and journal symlinks", async () => {
		const dir = await mkdtemp(join(tmpdir(), "cortex-budget-"));
		const path = join(dir, "budget.json");
		const journal = new FileBudgetJournal(path, binding);
		try {
			new TreeBudget(limits, () => 0, journal);
			const original = await readFile(path, "utf8");
			expect(() => new TreeBudget({ ...limits, maxCalls: 11 }, () => 0, journal)).toThrow(/checkpoint/);
			journal.release();
			const different = new FileBudgetJournal(path, "b".repeat(64));
			try {
				expect(() => different.load()).toThrow(/different Cortex runtime/);
			} finally {
				different.release();
			}
			await writeFile(path, "{");
			const corrupt = new FileBudgetJournal(path, binding);
			try {
				expect(() => corrupt.load()).toThrow();
			} finally {
				corrupt.release();
			}
			await writeFile(path, original);
			const link = join(dir, "link.json");
			await symlink(path, link);
			const linked = new FileBudgetJournal(link, binding);
			try {
				expect(() => linked.load()).toThrow(/Cannot read/);
			} finally {
				linked.release();
			}
		} finally {
			journal.release();
			await rm(dir, { recursive: true, force: true });
		}
	});
});
