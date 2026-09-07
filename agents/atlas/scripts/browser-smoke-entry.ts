import { complete, getModel } from "@earendil-works/pi-ai";

const model = getModel("google", "gemini-2.5-flash");
console.log(model.id, typeof complete);
