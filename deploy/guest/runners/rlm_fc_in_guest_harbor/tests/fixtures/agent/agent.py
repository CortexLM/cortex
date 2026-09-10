from harbor.agents.base import BaseAgent


class MinerAgent(BaseAgent):
    @staticmethod
    def name() -> str:
        return "miner-agent"

    def version(self) -> str | None:
        return "0.0.1"
