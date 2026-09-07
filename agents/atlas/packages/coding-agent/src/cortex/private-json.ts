import { randomUUID } from "node:crypto";
import { closeSync, constants, fstatSync, fsyncSync, openSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

export function readPrivateJson(path: string, maxBytes: number): unknown {
	let fd: number;
	try {
		fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
		throw new Error("Cannot read private Cortex journal");
	}
	try {
		const stat = fstatSync(fd);
		if (!stat.isFile() || stat.size === 0 || stat.size > maxBytes) throw new Error("Invalid Cortex journal size");
		return JSON.parse(readFileSync(fd, "utf8"));
	} finally {
		closeSync(fd);
	}
}

/** Only controller-owned canonical paths may be passed here. */
export function writePrivateJson(path: string, value: unknown): void {
	const partial = `${path}.${randomUUID()}.next`;
	const fd = openSync(partial, "wx", 0o600);
	try {
		writeFileSync(fd, JSON.stringify(value));
		fsyncSync(fd);
	} finally {
		closeSync(fd);
	}
	renameSync(partial, path);
	const directory = openSync(dirname(path), "r");
	try {
		fsyncSync(directory);
	} finally {
		closeSync(directory);
	}
}
