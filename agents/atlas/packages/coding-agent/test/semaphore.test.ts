/**
 * Tests for the async Semaphore used to bound subagent kernel-boot concurrency.
 */

import { describe, expect, it } from "vitest";
import { Semaphore } from "../src/utils/semaphore.js";

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((r) => {
		resolve = r;
	});
	return { promise, resolve };
}

describe("Semaphore", () => {
	it("rejects non-positive permit counts", () => {
		expect(() => new Semaphore(0)).toThrow();
		expect(() => new Semaphore(-1)).toThrow();
		expect(() => new Semaphore(1.5)).toThrow();
	});

	it("never exceeds the permit count under heavy fan-out", async () => {
		const limit = 3;
		const total = 50;
		const sem = new Semaphore(limit);
		let active = 0;
		let peak = 0;
		let completed = 0;

		await Promise.all(
			Array.from({ length: total }, () =>
				sem.run(async () => {
					active += 1;
					peak = Math.max(peak, active);
					await new Promise((r) => setTimeout(r, 1));
					active -= 1;
					completed += 1;
				}),
			),
		);

		expect(peak).toBeLessThanOrEqual(limit);
		expect(completed).toBe(total);
	});

	it("releases the permit when the task throws", async () => {
		const sem = new Semaphore(1);
		await expect(sem.run(async () => Promise.reject(new Error("boom")))).rejects.toThrow("boom");
		// If the permit leaked, this second acquire would hang forever.
		await expect(sem.run(async () => "ok")).resolves.toBe("ok");
	});

	it("hands permits to waiters in FIFO order", async () => {
		const sem = new Semaphore(1);
		const order: number[] = [];
		const gate = deferred<void>();

		// Holder occupies the single permit until we release `gate`.
		const holder = sem.run(async () => {
			await gate.promise;
		});

		// Queue three waiters; they should run in the order they queued.
		const waiters = [0, 1, 2].map((i) =>
			sem.run(async () => {
				order.push(i);
			}),
		);

		expect(sem.queueLength).toBe(3);
		gate.resolve();
		await Promise.all([holder, ...waiters]);
		expect(order).toEqual([0, 1, 2]);
	});

	it("a queued waiter aborts out of the queue without consuming a permit", async () => {
		const sem = new Semaphore(1);
		const gate = deferred<void>();
		const holder = sem.run(async () => {
			await gate.promise;
		});

		const ac = new AbortController();
		const queued = sem.run(async () => "ran", ac.signal);
		expect(sem.queueLength).toBe(1);

		ac.abort();
		await expect(queued).rejects.toBeTruthy();
		expect(sem.queueLength).toBe(0);

		// The aborted waiter must not have stolen the permit: once the holder
		// releases, a fresh acquire runs immediately.
		gate.resolve();
		await holder;
		await expect(sem.run(async () => "ok")).resolves.toBe("ok");
	});

	it("rejects immediately if the signal is already aborted", async () => {
		const sem = new Semaphore(1);
		const ac = new AbortController();
		ac.abort();
		await expect(sem.run(async () => "ran", ac.signal)).rejects.toBeTruthy();
		await expect(sem.run(async () => "ok")).resolves.toBe("ok");
	});
});
