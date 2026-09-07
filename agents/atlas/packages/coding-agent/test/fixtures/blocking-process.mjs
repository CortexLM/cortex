import { writeFileSync } from "node:fs";

const readyPath = process.argv[2];
if (!readyPath) {
	throw new Error("A readiness marker path is required");
}

writeFileSync(readyPath, `${process.pid}\n`);

const keepAlive = setInterval(() => {}, 2_147_483_647);
const stop = () => {
	clearInterval(keepAlive);
	process.exit(0);
};

process.on("SIGINT", stop);
process.on("SIGTERM", stop);
