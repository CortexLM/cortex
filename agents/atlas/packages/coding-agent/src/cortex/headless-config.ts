import { constants, type Stats } from "node:fs";
import { lstat, mkdir, open, realpath } from "node:fs/promises";
import { dirname, isAbsolute, relative, resolve } from "node:path";
import type { Api, Model } from "@earendil-works/pi-ai";
import { assertScope, type CortexLimits, type CortexScope } from "./policy.js";
import type { SandboxLimits } from "./sandbox.js";

export const MAX_LAUNCH_BYTES = 256 * 1024;

export interface CortexLaunchConfig {
	schema_version: 1;
	runtime_id: string;
	deadline_ms: number;
	resume: boolean;
	scope: CortexScope;
	controller_socket: string;
	state_dir: string;
	workspace: string;
	sandbox_dir: string;
	model_config_file: string;
	kernel: SandboxLimits;
	budget: CortexLimits;
	prompt: string;
}

export interface CortexModelConfig {
	schema_version: 1;
	provider: string;
	model: string;
	api: "openai-responses" | "openai-completions" | "anthropic-messages";
	baseUrl: string;
	/** Explicit opt-in for literal 127.0.0.1 or [::1] HTTP endpoints only. */
	allowLoopbackHttp?: true;
	apiKeyFile: string;
	reasoning: boolean;
	contextWindow: number;
	maxTokens: number;
	/** USD per million tokens; reservations use the highest of these explicit rates. */
	cost: { input: number; output: number; cacheRead: number; cacheWrite: number };
}

export type HeadlessFailureCode =
	| "invalid_launch"
	| "unsafe_paths"
	| "model_config_unavailable"
	| "reattachment_failed"
	| "runtime_failed"
	| "model_failed"
	| "deadline_exceeded"
	| "cleanup_failed";

export class HeadlessFailure extends Error {
	constructor(readonly code: HeadlessFailureCode) {
		super(code);
	}
}

function object(value: unknown, keys: string[], optional: string[] = []): Record<string, unknown> {
	if (
		!value ||
		typeof value !== "object" ||
		Array.isArray(value) ||
		Object.keys(value).some((key) => !keys.includes(key) && !optional.includes(key)) ||
		keys.some((key) => !Object.hasOwn(value, key))
	)
		throw new Error("Invalid configuration object");
	return value as Record<string, unknown>;
}

function text(value: unknown, maxBytes: number): string {
	if (typeof value !== "string" || !value.trim() || Buffer.byteLength(value) > maxBytes)
		throw new Error("Invalid configuration string");
	return value;
}

function integer(value: unknown, ceiling = Number.MAX_SAFE_INTEGER): number {
	if (typeof value !== "number" || !Number.isSafeInteger(value) || value <= 0 || value > ceiling)
		throw new Error("Invalid configuration limit");
	return value;
}

function path(value: unknown): string {
	const result = text(value, 4096);
	if (!isAbsolute(result) || resolve(result) !== result || result === "/" || /[\u0000-\u001f\u007f,]/.test(result))
		throw new Error("Invalid absolute path");
	return result;
}

function contains(parent: string, child: string): boolean {
	const suffix = relative(parent, child);
	return suffix === "" || (!isAbsolute(suffix) && suffix !== ".." && !suffix.startsWith("../"));
}

function outside(paths: string[], privatePath: string): void {
	if (paths.some((directory) => contains(directory, privatePath) || contains(privatePath, directory)))
		throw new Error("Private paths overlap runtime directories");
}

export function parseCortexLaunch(value: unknown): CortexLaunchConfig {
	try {
		const data = object(value, [
			"schema_version",
			"runtime_id",
			"deadline_ms",
			"resume",
			"scope",
			"controller_socket",
			"state_dir",
			"workspace",
			"sandbox_dir",
			"model_config_file",
			"kernel",
			"budget",
			"prompt",
		]);
		if (data.schema_version !== 1) throw new Error("Unsupported launch schema");
		const runtimeId = text(data.runtime_id, 36);
		if (
			!/^[a-f0-9]{8}-[a-f0-9]{4}-[1-8][a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(runtimeId) ||
			typeof data.resume !== "boolean"
		)
			throw new Error("Invalid runtime identity or resume mode");
		const rawScope = object(data.scope, ["role", "id", "commitment"]);
		if (rawScope.role !== "experiment" && rawScope.role !== "atlas") throw new Error("Invalid role");
		const scope: CortexScope = {
			role: rawScope.role,
			id: text(rawScope.id, 100),
			commitment: text(rawScope.commitment, 64),
		};
		assertScope(scope);
		const rawKernel = object(data.kernel, ["image", "memoryMb", "workspaceMb", "cpus", "pids", "seconds"]);
		const image = text(rawKernel.image, 512);
		if (!/^(?:[a-zA-Z0-9./_:-]+@)?sha256:[a-f0-9]{64}$/.test(image)) throw new Error("Unpinned kernel");
		const kernel: SandboxLimits = {
			image,
			memoryMb: integer(rawKernel.memoryMb, 65536),
			workspaceMb: integer(rawKernel.workspaceMb, 4096),
			cpus: integer(rawKernel.cpus, 64),
			pids: integer(rawKernel.pids, 1024),
			seconds: integer(rawKernel.seconds, 86400),
		};
		const rawBudget = object(data.budget, [
			"maxDepth",
			"maxChildren",
			"maxConcurrentCalls",
			"maxCalls",
			"maxReservedTokens",
			"maxReservedMicroUsd",
			"timeoutMs",
		]);
		const budget: CortexLimits = {
			maxDepth: integer(rawBudget.maxDepth, 16),
			maxChildren: integer(rawBudget.maxChildren, Number.MAX_SAFE_INTEGER - 1),
			maxConcurrentCalls: integer(rawBudget.maxConcurrentCalls),
			maxCalls: integer(rawBudget.maxCalls),
			maxReservedTokens: integer(rawBudget.maxReservedTokens),
			maxReservedMicroUsd: integer(rawBudget.maxReservedMicroUsd),
			timeoutMs: integer(rawBudget.timeoutMs, 86_400_000),
		};
		const launch: CortexLaunchConfig = {
			schema_version: 1,
			runtime_id: runtimeId,
			deadline_ms: integer(data.deadline_ms),
			resume: data.resume,
			scope,
			controller_socket: path(data.controller_socket),
			state_dir: path(data.state_dir),
			workspace: path(data.workspace),
			sandbox_dir: path(data.sandbox_dir),
			model_config_file: path(data.model_config_file),
			kernel,
			budget,
			prompt: text(data.prompt, 128 * 1024),
		};
		const directories = [launch.state_dir, launch.workspace, launch.sandbox_dir];
		directories.forEach((directory, index) => {
			outside(directories.slice(index + 1), directory);
		});
		outside(directories, launch.controller_socket);
		outside(directories, launch.model_config_file);
		if (launch.controller_socket === launch.model_config_file) throw new Error("Overlapping private files");
		return launch;
	} catch {
		throw new HeadlessFailure("invalid_launch");
	}
}

export function parseCortexModel(value: unknown): CortexModelConfig {
	try {
		const data = object(
			value,
			[
				"schema_version",
				"provider",
				"model",
				"api",
				"baseUrl",
				"apiKeyFile",
				"reasoning",
				"contextWindow",
				"maxTokens",
				"cost",
			],
			["allowLoopbackHttp"],
		);
		if (
			data.schema_version !== 1 ||
			!["openai-responses", "openai-completions", "anthropic-messages"].includes(String(data.api)) ||
			typeof data.reasoning !== "boolean"
		)
			throw new Error("Unsupported model configuration");
		const provider = text(data.provider, 100);
		const model = text(data.model, 200);
		if (!/^[a-zA-Z0-9_-]+$/.test(provider) || !/^[a-zA-Z0-9._:/-]+$/.test(model))
			throw new Error("Invalid model identity");
		const baseUrl = text(data.baseUrl, 4096);
		const url = new URL(baseUrl);
		if (Object.hasOwn(data, "allowLoopbackHttp") && data.allowLoopbackHttp !== true)
			throw new Error("Invalid loopback HTTP opt-in");
		// Check the literal authority too: WHATWG URLs normalize integer/octal IPv4 and expanded IPv6.
		const loopbackHttp =
			data.allowLoopbackHttp === true &&
			url.protocol === "http:" &&
			["127.0.0.1", "[::1]"].includes(url.hostname) &&
			/^http:\/\/(?:127\.0\.0\.1|\[::1\])(?::[0-9]+)?(?:\/|$)/.test(baseUrl);
		if (
			(url.protocol !== "https:" && !loopbackHttp) ||
			!url.hostname ||
			url.username ||
			url.password ||
			url.search ||
			url.hash ||
			/[\s\\?#]/.test(baseUrl)
		)
			throw new Error("Model endpoint is not permitted");
		const rawCost = object(data.cost, ["input", "output", "cacheRead", "cacheWrite"]);
		const rate = (value: unknown): number => {
			if (typeof value !== "number" || !Number.isFinite(value) || value < 0) throw new Error("Invalid model price");
			return value;
		};
		const cost = {
			input: rate(rawCost.input),
			output: rate(rawCost.output),
			cacheRead: rate(rawCost.cacheRead),
			cacheWrite: rate(rawCost.cacheWrite),
		};
		const contextWindow = integer(data.contextWindow);
		const maxTokens = integer(data.maxTokens);
		const reservation = contextWindow + maxTokens;
		if (
			maxTokens > contextWindow ||
			!Number.isSafeInteger(reservation) ||
			!Number.isSafeInteger(Math.ceil(reservation * Math.max(...Object.values(cost))))
		)
			throw new Error("Unbounded model reservation");
		return {
			schema_version: 1,
			provider,
			model,
			api: data.api as CortexModelConfig["api"],
			baseUrl,
			...(data.allowLoopbackHttp === true ? { allowLoopbackHttp: true as const } : {}),
			apiKeyFile: path(data.apiKeyFile),
			reasoning: data.reasoning,
			contextWindow,
			maxTokens,
			cost,
		};
	} catch {
		throw new HeadlessFailure("model_config_unavailable");
	}
}

function privateOwner(stat: Stats): void {
	if (stat.uid !== process.geteuid?.() || (stat.mode & 0o077) !== 0)
		throw new Error("Cortex paths must be owner-private");
}

async function privateDirectory(directory: string): Promise<void> {
	if ((await realpath(directory)) !== directory) throw new Error("Noncanonical private directory");
	const stat = await lstat(directory);
	privateOwner(stat);
	if (!stat.isDirectory()) throw new Error("Missing private directory");
}

export async function readHeadlessPrivateFile(file: string, maxBytes: number): Promise<string> {
	path(file);
	await privateDirectory(dirname(file));
	if ((await realpath(file)) !== file) throw new Error("Noncanonical private file");
	const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
	try {
		const stat = await handle.stat();
		privateOwner(stat);
		if (!stat.isFile() || stat.nlink !== 1 || stat.size <= 0 || stat.size > maxBytes)
			throw new Error("Invalid private file");
		const buffer = Buffer.alloc(maxBytes + 1);
		let bytes = 0;
		for (;;) {
			const result = await handle.read(buffer, bytes, buffer.length - bytes, null);
			bytes += result.bytesRead;
			if (bytes > maxBytes) throw new Error("Private file exceeds limit");
			if (result.bytesRead === 0) return buffer.subarray(0, bytes).toString("utf8");
		}
	} finally {
		await handle.close();
	}
}

export async function prepareHeadlessPaths(launch: CortexLaunchConfig): Promise<void> {
	try {
		for (const directory of [launch.state_dir, launch.workspace, launch.sandbox_dir]) {
			await privateDirectory(dirname(directory));
			await mkdir(directory, { mode: 0o700 }).catch((error: NodeJS.ErrnoException) => {
				if (error.code !== "EEXIST") throw error;
			});
			await privateDirectory(directory);
		}
		await privateDirectory(dirname(launch.controller_socket));
		if ((await realpath(launch.controller_socket)) !== launch.controller_socket)
			throw new Error("Noncanonical controller socket");
		const socket = await lstat(launch.controller_socket);
		privateOwner(socket);
		if (!socket.isSocket()) throw new Error("Controller socket unavailable");
	} catch {
		throw new HeadlessFailure("unsafe_paths");
	}
}

export async function loadHeadlessModel(launch: CortexLaunchConfig): Promise<{
	config: CortexModelConfig;
	model: Model<Api>;
	apiKey: string;
}> {
	try {
		const config = parseCortexModel(JSON.parse(await readHeadlessPrivateFile(launch.model_config_file, 64 * 1024)));
		outside([launch.state_dir, launch.workspace, launch.sandbox_dir], config.apiKeyFile);
		if ([launch.controller_socket, launch.model_config_file].includes(config.apiKeyFile))
			throw new Error("Overlapping private files");
		const apiKey = (await readHeadlessPrivateFile(config.apiKeyFile, 16 * 1024)).trim();
		if (!/^[\x21-\x7e]+$/.test(apiKey)) throw new Error("Invalid API key file");
		return {
			config,
			apiKey,
			model: {
				provider: config.provider,
				id: config.model,
				name: config.model,
				api: config.api,
				baseUrl: config.baseUrl,
				reasoning: config.reasoning,
				input: ["text"],
				contextWindow: config.contextWindow,
				maxTokens: config.maxTokens,
				cost: config.cost,
			},
		};
	} catch {
		throw new HeadlessFailure("model_config_unavailable");
	}
}
