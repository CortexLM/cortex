import "./headless-stdio.js";
import { manageKernelSignalsExternally } from "../core/kernel/shared.js";
import { runCortexHeadless } from "./headless.js";
import { HeadlessFailure, MAX_LAUNCH_BYTES, readHeadlessPrivateFile } from "./headless-config.js";
import { emitHeadlessEvent } from "./headless-stdio.js";

const controller = new AbortController();
let signalExit = 130;
let fatal = false;
const cancel = (exitCode: number) => {
	signalExit = exitCode;
	controller.abort();
	process.stdin.destroy();
};
process.on("SIGTERM", () => cancel(143));
process.on("SIGINT", () => cancel(130));
process.on("uncaughtException", () => {
	fatal = true;
	cancel(1);
});
process.on("unhandledRejection", () => {
	fatal = true;
	cancel(1);
});
process.stdout.on("error", () => cancel(1));

async function launchInput(): Promise<unknown> {
	try {
		const args = process.argv.slice(2);
		if (args.length === 2 && args[0] === "--launch") {
			return JSON.parse(await readHeadlessPrivateFile(args[1], MAX_LAUNCH_BYTES));
		}
		if (args.length !== 1 || args[0] !== "--stdin") throw new Error("Invalid launch arguments");
		const timer = setTimeout(() => process.stdin.destroy(new HeadlessFailure("invalid_launch")), 30_000);
		try {
			const chunks: Buffer[] = [];
			let bytes = 0;
			for await (const chunk of process.stdin) {
				const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
				bytes += buffer.length;
				if (bytes > MAX_LAUNCH_BYTES) throw new Error("Launch input exceeds limit");
				chunks.push(buffer);
			}
			return JSON.parse(Buffer.concat(chunks).toString("utf8"));
		} finally {
			clearTimeout(timer);
			process.stdin.destroy();
		}
	} catch {
		throw new HeadlessFailure("invalid_launch");
	}
}

try {
	manageKernelSignalsExternally();
	const input = await launchInput();
	const result = await runCortexHeadless(input, {
		emit: (event) =>
			emitHeadlessEvent(
				fatal && event.event === "stopped"
					? { schema_version: 1, event: "failed", code: "runtime_failed", counts: event.counts }
					: event,
			),
		signal: controller.signal,
	});
	process.exitCode = fatal ? 1 : result === 130 ? signalExit : result;
} catch {
	emitHeadlessEvent({
		schema_version: 1,
		event: "failed",
		code: "invalid_launch",
		counts: { calls: 0, reservedTokens: 0, reservedMicroUsd: 0, children: 0 },
	});
	process.exitCode = controller.signal.aborted ? signalExit : 1;
}
