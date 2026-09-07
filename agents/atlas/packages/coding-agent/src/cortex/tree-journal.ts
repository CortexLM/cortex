import { realpathSync } from "node:fs";
import { dirname, isAbsolute, relative, resolve } from "node:path";
import { readPrivateJson, writePrivateJson } from "./private-json.js";

export interface CortexNode {
	sessionId: string;
	file: string;
	depth: number;
	parentId?: string;
	childId?: string;
	status: "running" | "completed" | "interrupted" | "deleting" | "deleted";
}

export class TreeJournal {
	private readonly nodes = new Map<string, CortexNode>();
	readonly restored: boolean;

	constructor(
		private readonly path: string,
		private readonly binding: string,
		private readonly maxNodes: number,
	) {
		if (
			!isAbsolute(path) ||
			realpathSync(dirname(path)) !== dirname(path) ||
			!/^[a-f0-9]{64}$/.test(binding) ||
			!Number.isSafeInteger(maxNodes) ||
			maxNodes < 1
		)
			throw new Error("Invalid Cortex tree configuration");
		const data = readPrivateJson(path, 1024 * 1024);
		this.restored = data !== undefined;
		if (!this.restored) return;
		if (
			typeof data !== "object" ||
			!data ||
			!("version" in data) ||
			data.version !== 1 ||
			!("binding" in data) ||
			data.binding !== binding ||
			!("nodes" in data) ||
			!Array.isArray(data.nodes) ||
			data.nodes.length === 0 ||
			data.nodes.length > maxNodes
		)
			throw new Error("Invalid Cortex tree journal");
		const values: unknown[] = data.nodes;
		this.validate(values);
		for (const value of values as CortexNode[]) {
			this.nodes.set(value.sessionId, {
				...value,
				status: value.status === "running" ? "interrupted" : value.status,
			});
		}
	}

	private validate(values: unknown[]): void {
		const nodes = new Map<string, CortexNode>();
		const files = new Set<string>();
		const childIds = new Set<string>();
		if (values.length === 0 || values.length > this.maxNodes) throw new Error("Invalid Cortex tree size");
		for (const value of values) {
			if (
				typeof value !== "object" ||
				!value ||
				!("sessionId" in value) ||
				typeof value.sessionId !== "string" ||
				!/^[a-zA-Z0-9-]{1,100}$/.test(value.sessionId) ||
				!("file" in value) ||
				typeof value.file !== "string" ||
				!isAbsolute(value.file) ||
				resolve(value.file) !== value.file ||
				files.has(value.file) ||
				!("depth" in value) ||
				typeof value.depth !== "number" ||
				!Number.isSafeInteger(value.depth) ||
				value.depth < 0 ||
				value.depth > 16 ||
				!("status" in value) ||
				typeof value.status !== "string" ||
				!["running", "completed", "interrupted", "deleting", "deleted"].includes(value.status) ||
				nodes.has(value.sessionId)
			)
				throw new Error("Invalid Cortex tree node");
			const node = value as CortexNode;
			if (value.depth === 0) {
				if (
					nodes.size !== 0 ||
					node.parentId !== undefined ||
					node.childId !== undefined ||
					node.status === "deleting" ||
					node.status === "deleted"
				) {
					throw new Error("Invalid Cortex root");
				}
			} else {
				const parent = node.parentId ? nodes.get(node.parentId) : undefined;
				if (
					!parent ||
					parent.depth + 1 !== value.depth ||
					typeof node.childId !== "string" ||
					!/^sub-[a-f0-9-]{8,36}$/.test(node.childId) ||
					childIds.has(node.childId) ||
					(parent.status === "deleted" && node.status !== "deleted") ||
					(parent.status === "deleting" && node.status !== "deleted" && node.status !== "deleting")
				) {
					throw new Error("Invalid Cortex parent edge");
				}
				childIds.add(node.childId);
			}
			const suffix = relative(dirname(this.path), value.file);
			if (!suffix || suffix.startsWith("..") || isAbsolute(suffix))
				throw new Error("Cortex session outside state directory");
			nodes.set(node.sessionId, node);
			files.add(node.file);
		}
	}

	entries(): CortexNode[] {
		return [...this.nodes.values()].map((node) => ({ ...node }));
	}

	put(node: CortexNode): void {
		if (this.nodes.has(node.sessionId)) throw new Error("Cortex session identity is immutable");
		this.replace([...this.entries(), node]);
	}

	subtree(sessionId: string): CortexNode[] {
		if (!this.nodes.has(sessionId)) throw new Error("Unknown Cortex session");
		const ids = new Set([sessionId]);
		return this.entries().filter((node) => {
			if (node.parentId && ids.has(node.parentId)) ids.add(node.sessionId);
			return ids.has(node.sessionId);
		});
	}

	beginDeletion(sessionId: string): CortexNode[] {
		const subtree = this.subtree(sessionId);
		if (subtree[0].depth === 0) throw new Error("Cannot delete the Cortex root");
		const ids = new Set(subtree.map((node) => node.sessionId));
		this.replace(
			this.entries().map((node) =>
				ids.has(node.sessionId) && node.status !== "deleted" ? { ...node, status: "deleting" } : node,
			),
		);
		return this.subtree(sessionId);
	}

	status(sessionId: string, status: CortexNode["status"]): void {
		const node = this.nodes.get(sessionId);
		if (!node) throw new Error("Unknown Cortex session");
		if ((node.status === "deleted" || node.status === "deleting") && status !== "deleted" && status !== node.status) {
			throw new Error("Cannot revive a deleted Cortex session");
		}
		this.replace(this.entries().map((entry) => (entry.sessionId === sessionId ? { ...entry, status } : entry)));
	}

	private replace(nodes: CortexNode[]): void {
		this.validate(nodes);
		writePrivateJson(this.path, { version: 1, binding: this.binding, nodes });
		this.nodes.clear();
		for (const node of nodes) this.nodes.set(node.sessionId, { ...node });
	}
}
