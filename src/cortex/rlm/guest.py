"""Guest-side entry point; provisioning supplies keys, limits and VM adapters."""

from __future__ import annotations

from collections.abc import Callable
from typing import Protocol

from .engine import RlmEngine
from .errors import ToolRejected
from .journal import RunJournal
from .knowledge import KnowledgeAccess
from .models import AgentLimits, AgentRun, AgentTask, StrictModel, VmExecutor
from .provider import ModelProvider


class AgentRequest(StrictModel):
    task: AgentTask
    resume: bool = False


class GuestBinding(Protocol):
    @property
    def topic_id(self) -> str: ...

    @property
    def image_digest(self) -> str: ...

    @property
    def kind(self) -> str: ...


class GuestRlmService:
    """Run inside a topic guest using a boot-verified binding and host callback.

    No request field selects an API key, model, journal path or execution backend.
    The caller creates binding from the VM's kernel boot identity and supplies an
    executor that routes experiments to the host's authenticated sister VM API.
    """

    def __init__(
        self,
        *,
        binding: GuestBinding,
        provider: ModelProvider,
        executor_factory: Callable[[AgentTask], VmExecutor],
        journal: RunJournal,
        limits: AgentLimits,
        knowledge: KnowledgeAccess | None = None,
    ) -> None:
        self.binding = binding
        self.provider = provider
        self.executor_factory = executor_factory
        self.journal = journal
        self.limits = limits
        self.knowledge = knowledge

    async def run(self, request: AgentRequest) -> AgentRun:
        context = request.task.context
        if (
            self.binding.kind != "topic"
            or context.topic_id != self.binding.topic_id
            or context.image_digest != self.binding.image_digest
        ):
            raise ToolRejected("agent request does not bind this topic guest")
        engine = RlmEngine(
            self.provider,
            self.executor_factory(request.task),
            limits=self.limits,
            knowledge=self.knowledge,
            journal=self.journal,
        )
        return await engine.run(request.task, resume=request.resume)
