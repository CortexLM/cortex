import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { type HostRequestHandlers, ReplKernelManager } from "../src/core/kernel/index.js";
import { CortexSandbox } from "../src/cortex/sandbox.js";

const image = process.env.CORTEX_TEST_KERNEL_IMAGE;

async function fixture(id: string, hostHandlers?: HostRequestHandlers) {
	if (!image) throw new Error("Missing kernel image");
	const dir = await mkdtemp(join(tmpdir(), `cortex-${id}-`));
	const work = join(dir, "work");
	await mkdir(work);
	const sandbox = new CortexSandbox(join(dir, "sandbox"), {
		image,
		memoryMb: 512,
		workspaceMb: 64,
		cpus: 1,
		pids: 64,
		seconds: 30,
	});
	const kernel = new ReplKernelManager({
		python: await sandbox.prepare(work),
		inheritEnv: false,
		cwd: work,
		hostHandlers,
	});
	return {
		dir,
		work,
		sandbox,
		kernel,
		cleanup: async () => {
			await kernel.shutdown();
			await sandbox.stop();
			await rm(dir, { recursive: true, force: true });
		},
	};
}

describe.skipIf(!image)("Cortex sandbox adversarial probes", () => {
	it("keeps simultaneous tenants' files and namespaces separate", async () => {
		const left = await fixture("tenant-left");
		const right = await fixture("tenant-right");
		try {
			const results = await Promise.all(
				[left, right].map((test, index) =>
					test.kernel.execute(`
import os, pathlib
x = ${index}
pathlib.Path('/work/private').write_text(str(x))
assert not pathlib.Path(${JSON.stringify(index === 0 ? right.work : left.work)}).exists()
assert os.getuid() == 65532
print(x)
`),
				),
			);
			expect(results.map((result) => result.status)).toEqual(["ok", "ok"]);
			expect((await left.kernel.execute("print(x, open('/work/private').read())")).stdout.trim()).toBe("0 0");
			expect((await right.kernel.execute("print(x, open('/work/private').read())")).stdout.trim()).toBe("1 1");
			await left.cleanup();
			expect((await right.kernel.execute("print(x)")).stdout.trim()).toBe("1");
		} finally {
			await left.cleanup();
			await right.cleanup();
		}
	}, 60_000);

	it.each([
		["invalid JSON", "os.write(sys.modules['rlm.repl']._protocol_fd, b'not-json\\n')"],
		["oversized frame", "os.write(sys.modules['rlm.repl']._protocol_fd, b'x' * (17 * 1024 * 1024))"],
	])(
		"stops a kernel emitting %s",
		async (_name, code) => {
			const test = await fixture("protocol");
			try {
				await test.kernel.start();
				await expect(test.kernel.execute(`import os, sys\n${code}`)).rejects.toThrow(/shut down/);
				expect(test.kernel.isRunning).toBe(false);
			} finally {
				await test.cleanup();
			}
		},
		30_000,
	);

	it("bounds pending host requests and tears down a flooding kernel", async () => {
		let release: () => void = () => {};
		const gate = new Promise<void>((resolve) => {
			release = resolve;
		});
		let admitted = 0;
		const test = await fixture("flood", {
			slow: async () => {
				admitted++;
				await gate;
				return {};
			},
		});
		try {
			await expect(
				test.kernel.execute(`
import asyncio
from rlm import host_request
await asyncio.gather(*[host_request('slow', {}) for _ in range(128)])
`),
			).rejects.toThrow(/concurrency exhausted/);
			expect(admitted).toBe(64);
			expect(test.kernel.isRunning).toBe(false);
		} finally {
			release();
			await test.cleanup();
		}
	}, 30_000);
});
