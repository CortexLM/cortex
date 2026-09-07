import type { HeadlessEvent } from "./headless.js";

const write = process.stdout.write.bind(process.stdout);
const discard = (
	_chunk: Uint8Array | string,
	encoding?: BufferEncoding | ((error?: Error | null) => void),
	callback?: (error?: Error | null) => void,
): boolean => {
	const done = typeof encoding === "function" ? encoding : callback;
	done?.();
	return true;
};

// This module must be the CLI's first import: SDK/provider diagnostics are not a public channel.
process.stdout.write = discard;
process.stderr.write = discard;

export function emitHeadlessEvent(event: HeadlessEvent): void {
	write(`${JSON.stringify(event)}\n`);
}
