const { spawnSync } = require("node:child_process");
const { existsSync } = require("node:fs");
const { join } = require("node:path");

const script = join(__dirname, "dist", "postinstall.js");
if (!existsSync(script)) {
	process.exit(0);
}

const result = spawnSync(process.execPath, [script], { stdio: "inherit" });
if (result.error) {
	console.error(`prime-agent: postinstall setup skipped: ${result.error.message}`);
}
process.exit(0);
