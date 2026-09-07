import { getModels } from "../src/models.js";
import type { Model } from "../src/types.js";

const KIMI_TEST_MODEL_PREFERENCE = ["kimi-k2-thinking", "kimi-for-coding", "k2p7", "k3", "kimi-for-coding-highspeed"];

export function getKimiCodingTestModel(options: { image?: boolean } = {}): Model<"anthropic-messages"> {
	const models = getModels("kimi-coding") as Model<"anthropic-messages">[];
	const eligible = options.image ? models.filter((model) => model.input.includes("image")) : models;
	for (const id of KIMI_TEST_MODEL_PREFERENCE) {
		const model = eligible.find((candidate) => candidate.id === id);
		if (model) return model;
	}
	const model = eligible[0];
	if (!model) {
		throw new Error(`No ${options.image ? "image-capable " : ""}Kimi Coding model is available`);
	}
	return model;
}
