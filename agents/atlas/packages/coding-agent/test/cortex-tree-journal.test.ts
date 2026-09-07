import { mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { type CortexNode, TreeJournal } from "../src/cortex/tree-journal.js";

const binding = "a".repeat(64);
const directories: string[] = [];
afterEach(async () => {
	for (const dir of directories.splice(0)) await rm(dir, { recursive: true, force: true });
});

async function fixture() {
	const dir = await mkdtemp(join(tmpdir(), "cortex-tree-journal-"));
	directories.push(dir);
	const path = join(dir, "tree.json");
	const root: CortexNode = { sessionId: "root", depth: 0, file: join(dir, "root.jsonl"), status: "running" };
	const child: CortexNode = {
		sessionId: "child",
		parentId: "root",
		childId: "sub-12345678",
		depth: 1,
		file: join(dir, "child.jsonl"),
		status: "running",
	};
	return { path, root, child };
}

describe("Cortex tree journal", () => {
	it("preserves interrupted state and refuses identity overwrite or deleted-child revival", async () => {
		const { path, root, child } = await fixture();
		const first = new TreeJournal(path, binding, 2);
		first.put(root);
		first.put(child);
		expect(() => first.put({ ...root, file: child.file })).toThrow(/immutable/);
		const restored = new TreeJournal(path, binding, 2);
		expect(restored.entries().map((node) => node.status)).toEqual(["interrupted", "interrupted"]);
		restored.status(child.sessionId, "deleting");
		expect(() => restored.status(child.sessionId, "running")).toThrow(/revive/);
		restored.status(child.sessionId, "deleted");
		expect(() => restored.status(child.sessionId, "running")).toThrow(/revive/);
		expect(() => restored.status(root.sessionId, "deleted")).toThrow(/root/);
	});

	it("validates identity, binding, ancestry, containment and size without repairing corrupt journals", async () => {
		const { path, root, child } = await fixture();
		const malformed = [
			[],
			[child, root],
			[root, root],
			[root, { ...child, file: root.file }],
			[root, { ...child, depth: 2 }],
			[root, { ...child, parentId: "missing" }],
			[root, { ...child, file: "/outside.jsonl" }],
			[root, child, { ...child, sessionId: "other", file: `${child.file}.other` }],
		];
		for (const nodes of malformed) {
			const text = JSON.stringify({ version: 1, binding, nodes });
			await writeFile(path, text);
			expect(() => new TreeJournal(path, binding, 4)).toThrow();
			expect(await readFile(path, "utf8")).toBe(text);
		}
		await writeFile(path, JSON.stringify({ version: 1, binding, nodes: [root, child] }));
		expect(() => new TreeJournal(path, "b".repeat(64), 4)).toThrow();
		expect(() => new TreeJournal(path, binding, 1)).toThrow();
		const link = `${path}.link`;
		await symlink(path, link);
		expect(() => new TreeJournal(link, binding, 4)).toThrow(/read/);
	});

	it("atomically reserves whole subtrees and only finalizes parents after their descendants", async () => {
		const { path, root, child } = await fixture();
		const tree = new TreeJournal(path, binding, 3);
		const grandchild: CortexNode = {
			...child,
			sessionId: "grandchild",
			parentId: child.sessionId,
			childId: "sub-87654321",
			depth: 2,
			file: `${child.file}.grandchild`,
		};
		tree.put(root);
		tree.put(child);
		tree.put(grandchild);
		expect(() => tree.status(child.sessionId, "deleting")).toThrow(/edge/);
		tree.beginDeletion(child.sessionId);
		expect(new TreeJournal(path, binding, 3).entries().map((node) => node.status)).toEqual([
			"interrupted",
			"deleting",
			"deleting",
		]);
		expect(() => tree.status(child.sessionId, "deleted")).toThrow(/edge/);
		expect(() => tree.status(grandchild.sessionId, "running")).toThrow(/revive/);
		tree.status(grandchild.sessionId, "deleted");
		tree.status(child.sessionId, "deleted");
		expect(tree.subtree(child.sessionId).every((node) => node.status === "deleted")).toBe(true);
	});
});
