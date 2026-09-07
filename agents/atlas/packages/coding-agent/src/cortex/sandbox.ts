import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdir, open, readdir, readFile, realpath, writeFile } from "node:fs/promises";
import { basename, isAbsolute, join, resolve } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const exec = promisify(execFile);
const supervisor = fileURLToPath(new URL("../../../../sandbox/kernel.py", import.meta.url));

export interface SandboxLimits {
	image: string;
	memoryMb: number;
	workspaceMb: number;
	cpus: number;
	pids: number;
	seconds: number;
}

function quoted(path: string): string {
	return `'${path.replaceAll("'", "'\\''")}'`;
}

export class CortexSandbox {
	private readonly names = new Set<string>();

	constructor(
		private readonly stateDir: string,
		readonly limits: SandboxLimits,
		private readonly deadlineMs?: number,
	) {
		if (!isAbsolute(stateDir)) throw new Error("Sandbox state path must be absolute");
		if (!/^(?:[a-zA-Z0-9./_:-]+@)?sha256:[a-f0-9]{64}$/.test(limits.image)) {
			throw new Error("Sandbox image must be pinned");
		}
		if (deadlineMs !== undefined && (!Number.isSafeInteger(deadlineMs) || deadlineMs <= 0))
			throw new Error("Invalid sandbox deadline");
		this.limits = Object.freeze({ ...limits });
	}

	async prepare(workspace: string, remainingMs = this.limits.seconds * 1000): Promise<string> {
		if (this.deadlineMs !== undefined) remainingMs = Math.min(remainingMs, this.deadlineMs - Date.now());
		if (!Number.isSafeInteger(remainingMs) || remainingMs <= 0) throw new Error("Sandbox capability expired");
		if (!isAbsolute(workspace) || (await realpath(workspace)) !== resolve(workspace)) {
			throw new Error("Sandbox workspace must not contain symlinks");
		}
		await mkdir(this.stateDir, { recursive: true, mode: 0o700 });
		const name = `cortex-kernel-${randomUUID()}`;
		const config = join(this.stateDir, `${name}.json`);
		const launcher = join(this.stateDir, `${name}.sh`);
		await writeFile(
			config,
			JSON.stringify({
				name,
				workspace,
				image: this.limits.image,
				memory_mb: this.limits.memoryMb,
				workspace_mb: this.limits.workspaceMb,
				cpus: this.limits.cpus,
				pids: this.limits.pids,
				seconds: Math.min(this.limits.seconds, Math.ceil(remainingMs / 1000)),
				...(this.deadlineMs !== undefined ? { deadline_ms: this.deadlineMs } : {}),
			}),
			{ mode: 0o600, flag: "wx" },
		);
		await writeFile(launcher, `#!/bin/sh\nexec /usr/bin/python3 -I ${quoted(supervisor)} ${quoted(config)} "$@"\n`, {
			mode: 0o700,
			flag: "wx",
		});
		for (const path of [config, launcher, this.stateDir]) {
			const file = await open(path, "r");
			try {
				await file.sync();
			} finally {
				await file.close();
			}
		}
		this.names.add(name);
		return launcher;
	}

	async stopLauncher(launcher: string): Promise<void> {
		const name = basename(launcher, ".sh");
		if (launcher !== join(this.stateDir, `${name}.sh`) || !this.names.has(name)) {
			throw new Error("Unknown Cortex kernel resource");
		}
		await this.remove(name);
	}

	async stop(): Promise<void> {
		// The controller's private inventory survives a crash. Never scan arbitrary provider names.
		const files = await readdir(this.stateDir).catch((error: NodeJS.ErrnoException) => {
			if (error.code === "ENOENT") return [];
			throw error;
		});
		for (const file of files) {
			if (!/^cortex-kernel-[a-f0-9-]{36}\.json$/.test(file)) continue;
			const name = file.slice(0, -5);
			const config: unknown = JSON.parse(await readFile(join(this.stateDir, file), "utf8"));
			if (typeof config !== "object" || !config || !("name" in config) || config.name !== name) {
				throw new Error("Invalid Cortex kernel inventory");
			}
			this.names.add(name);
		}
		const failures: unknown[] = [];
		for (const name of this.names) {
			await this.remove(name).catch((error: unknown) => failures.push(error));
		}
		if (failures.length) throw new Error(`Cannot confirm deletion of ${failures.length} Cortex kernel(s)`);
	}

	private async identity(name: string): Promise<string | undefined> {
		try {
			const result = await exec(
				"/usr/bin/docker",
				["inspect", '--format={{.Id}} {{index .Config.Labels "cortex.kernel"}}', name],
				{ timeout: 10_000, env: { PATH: "/usr/bin:/bin", HOME: "/nonexistent", LANG: "C.UTF-8" } },
			);
			const [id, label] = result.stdout.trim().split(" ");
			if (!/^[a-f0-9]{64}$/.test(id) || label !== name) throw new Error("Kernel identity mismatch");
			return id;
		} catch (error) {
			if (error instanceof Error && "stderr" in error && /no such (object|container)/i.test(String(error.stderr))) {
				return undefined;
			}
			throw new Error("Cannot verify Cortex kernel identity");
		}
	}

	private async remove(name: string): Promise<void> {
		const id = await this.identity(name);
		if (id) {
			await exec("/usr/bin/docker", ["rm", "--force", id], {
				timeout: 10_000,
				env: { PATH: "/usr/bin:/bin", HOME: "/nonexistent", LANG: "C.UTF-8" },
			}).catch(() => undefined);
			// A surviving supervisor can be removing the same ID during crash takeover.
			for (let attempt = 0; ; attempt++) {
				const remaining = await this.identity(name);
				if (!remaining) break;
				if (remaining !== id || attempt >= 50) throw new Error("Kernel still exists after deletion");
				await sleep(100);
			}
		}
		// Keep the private inventory addressable for idempotent cleanup retries.
	}
}
