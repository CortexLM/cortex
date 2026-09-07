import type { ToolDefinition } from "../core/extensions/index.js";
import { createIpythonToolDefinition, type IpythonKernelProvisioner } from "../core/tools/ipython.js";

/** The SDK erases tool schemas. Validate at that boundary instead of casting the typed tool. */
export function cortexIpythonTool(workspace: string, provisioner: IpythonKernelProvisioner): ToolDefinition {
	const tool = createIpythonToolDefinition(workspace, { provisioner });
	return {
		name: tool.name,
		label: tool.label,
		description: tool.description,
		promptSnippet: tool.promptSnippet,
		parameters: tool.parameters,
		executionMode: tool.executionMode,
		execute: (id, args, signal, update, context) => {
			if (
				typeof args !== "object" ||
				args === null ||
				!("code" in args) ||
				typeof args.code !== "string" ||
				args.code.length > 128_000
			) {
				throw new Error("Invalid Cortex Python cell");
			}
			return tool.execute(id, { code: args.code }, signal, update, context);
		},
	};
}
