from harbor.agents.base import BaseAgent


class RecipeAgent(BaseAgent):
    @staticmethod
    def name() -> str:
        return "recipe-agent"

    def version(self) -> str | None:
        return "0.0.1"
