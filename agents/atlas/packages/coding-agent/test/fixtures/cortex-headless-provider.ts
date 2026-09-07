import {
	type FauxResponseFactory,
	fauxAssistantMessage,
	fauxToolCall,
	registerFauxProvider,
} from "@earendil-works/pi-ai";
import { getMessageText } from "../suite/harness.js";

// Only tests preload this module. The production launcher has no faux-provider/config override.
const faux = registerFauxProvider({ api: "openai-responses", provider: "headless-fixture" });
const response: FauxResponseFactory = (context, options) => {
	if (options?.apiKey !== "synthetic-headless-key") throw new Error("Wrong fixture credential source");
	if (options.maxRetries !== 0) throw new Error("Unexpected provider retries");
	console.log("synthetic-headless-key diagnostic that must not reach stdout");
	process.stderr.write("synthetic-headless-key diagnostic that must not reach stderr\n");
	const mode = process.env.CORTEX_HEADLESS_TEST_MODE;
	if (mode === "failure") {
		return fauxAssistantMessage("synthetic-headless-key", {
			stopReason: "error",
			errorMessage: "Authorization: Bearer synthetic-headless-key",
		});
	}
	if (mode === "text" || mode === "resume") return fauxAssistantMessage("synthetic-headless-key untrusted output");
	if (mode === "resume-ipc") {
		const lastPrompt = context.messages.map((message) => message.role).lastIndexOf("user");
		if (context.messages.slice(lastPrompt + 1).some((message) => message.role === "toolResult"))
			return fauxAssistantMessage("Resumed through the current attempt socket");
		return fauxAssistantMessage(
			fauxToolCall("ipython", {
				code: "from rlm import host_request\nawait host_request('cortex.call', {'operation': 'read_evidence', 'arguments': {}})",
			}),
			{ stopReason: "toolUse" },
		);
	}
	const child = context.messages.some(
		(message) => message.role === "user" && getMessageText(message).includes("[task from parent]\n\nCHILD"),
	);
	const results = context.messages.filter((message) => message.role === "toolResult");
	if (results.length > 0) {
		if (results.some((message) => message.isError)) throw new Error("Fixture kernel failed");
		return fauxAssistantMessage("synthetic-headless-key untrusted completion");
	}
	if (!child) {
		return fauxAssistantMessage(
			fauxToolCall("ipython", {
				code: "handle = await rlm('CHILD', name='headless-child')\nprint('synthetic-headless-key private output')",
			}),
			{ stopReason: "toolUse" },
		);
	}
	return fauxAssistantMessage(
		fauxToolCall("ipython", {
			code: `
import os, pathlib, socket, subprocess
from rlm import host_request
assert os.getuid() == 65532
assert 'CORTEX_HEADLESS_TEST_MODE' not in os.environ
assert 'OPENAI_API_KEY' not in os.environ
assert not os.path.exists('/root/cortex')
assert not pathlib.Path('/var/run/docker.sock').exists()
try:
    socket.create_connection(('192.0.2.1', 80), timeout=0.2)
    raise AssertionError('network escaped')
except OSError:
    pass
await host_request('cortex.call', {'operation': 'read_evidence', 'arguments': {}})
${mode === "wait" ? "subprocess.Popen(['sleep', '60'])\nimport time\ntime.sleep(60)" : "await host_request('cortex.reply', {'text': 'untrusted child result'})"}
`,
		}),
		{ stopReason: "toolUse" },
	);
};
faux.setResponses(Array.from({ length: 30 }, () => response));
