import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { type FauxResponseFactory, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai";
import { cortexFixture } from "../cortex-fixture.js";
import { getMessageText } from "../suite/harness.js";

const dir = process.argv[2];
if (!dir?.startsWith("/")) throw new Error("Missing crash test directory");
let resuming = false;
const test = await cortexFixture(
	"crash",
	{
		call: async () => {
			const children = (await readFile(`/proc/${process.pid}/task/${process.pid}/children`, "utf8"))
				.trim()
				.split(/\s+/)
				.filter(Boolean)
				.map(Number);
			const supervisors: number[] = [];
			for (const pid of children) {
				const cmdline = await readFile(`/proc/${pid}/cmdline`, "utf8").catch(() => "");
				if (cmdline.includes("/sandbox/kernel.py")) supervisors.push(pid);
			}
			await writeFile(
				join(dir, "ready.json"),
				JSON.stringify({
					pid: process.pid,
					supervisors,
					harnessDir: test.harness.tempDir,
				}),
			);
			return {};
		},
	},
	{ dir },
);
let current = test.make();
const response: FauxResponseFactory = (context) => {
	const child = context.messages.some((message) => getMessageText(message).includes("[task from parent]\n\nCHILD"));
	const results = context.messages.filter((message) => message.role === "toolResult").length;
	if (results === (resuming ? 1 : 0)) {
		return fauxAssistantMessage(
			fauxToolCall("ipython", {
				code: child
					? resuming
						? "from rlm import host_request\nawait host_request('cortex.call', {'operation': 'read_evidence', 'arguments': {}})\nimport time\ntime.sleep(60)"
						: "from rlm import host_request\nx = 73\nawait host_request('cortex.reply', {'text': 'Stored child state'})"
					: resuming
						? "from rlm import host_request\nawait host_request('cortex.child', {'id': handle.rlm_child_id, 'text': 'Continue'})"
						: "handle = await rlm('CHILD')",
			}),
			{ stopReason: "toolUse" },
		);
	}
	return fauxAssistantMessage("Done");
};
test.harness.setResponses(Array.from({ length: 20 }, () => response));
await current.runtime.start();
await current.runtime.prompt("STORE");
await current.runtime.pause();
current = test.make();
await current.runtime.start();
resuming = true;
// The test kills this controller, so no graceful shutdown or lease release may run.
void current.runtime.prompt("RUN").catch(() => {});
setInterval(() => {}, 1000);
