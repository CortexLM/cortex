import { realpathSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import { acquireSessionLease, type SessionLease } from "../core/session-lease.js";
import type { BudgetCheckpoint, BudgetJournal } from "./policy.js";
import { readPrivateJson, writePrivateJson } from "./private-json.js";

/** Single-host journal. Multi-host scheduling additionally requires a database fence. */
export class FileBudgetJournal implements BudgetJournal {
	private readonly lease: SessionLease;

	constructor(
		private readonly path: string,
		readonly binding: string,
	) {
		if (!isAbsolute(path) || realpathSync(dirname(path)) !== resolve(dirname(path))) {
			throw new Error("Budget journal needs a private, canonical directory");
		}
		if (!/^[a-f0-9]{64}$/.test(binding)) throw new Error("Invalid budget binding");
		const lease = acquireSessionLease(path, dirname(path), { PRIME_AGENT_INTERNAL_SESSION_LEASES: "1" });
		if (!lease) throw new Error("Cannot acquire budget lease");
		this.lease = lease;
	}

	assertActive(): void {
		this.lease.assertHeld();
	}

	release(): void {
		this.lease.release();
	}

	load(): unknown {
		this.assertActive();
		const envelope = readPrivateJson(this.path, 64 * 1024);
		if (envelope === undefined) return undefined;
		if (
			typeof envelope !== "object" ||
			envelope === null ||
			!("version" in envelope) ||
			envelope.version !== 1 ||
			!("binding" in envelope) ||
			envelope.binding !== this.binding ||
			!("state" in envelope)
		)
			throw new Error("Budget journal belongs to a different Cortex runtime");
		return envelope.state;
	}

	save(state: BudgetCheckpoint): void {
		this.assertActive();
		writePrivateJson(this.path, { version: 1, binding: this.binding, state });
	}
}
