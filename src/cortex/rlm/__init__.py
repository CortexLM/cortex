"""Agentic research, bounded recursive inference, and verified shared knowledge."""

from .engine import RlmEngine
from .errors import BudgetExceeded, InvalidResponse, ProviderError, RlmError, ToolRejected
from .guest import AgentRequest, GuestRlmService
from .journal import RunJournal
from .knowledge import KnowledgeApproval, KnowledgeStore, Observation, RuleRevision
from .models import (
    AgentLimits,
    AgentRun,
    AgentTask,
    Endpoint,
    EvaluationVerdict,
    Metric,
    ProducedArtifact,
    ResearchSummary,
    Rule,
    RuleCheck,
    SetupProposal,
    VmAction,
    VmContext,
    VmExecutor,
    VmResult,
)
from .provider import OpenRouterClient
from .proxy import InferenceBroker, InferenceRequest, ProxyModelProvider

__all__ = [
    "AgentLimits",
    "AgentRequest",
    "AgentRun",
    "AgentTask",
    "BudgetExceeded",
    "Endpoint",
    "EvaluationVerdict",
    "GuestRlmService",
    "InvalidResponse",
    "InferenceBroker",
    "InferenceRequest",
    "KnowledgeApproval",
    "KnowledgeStore",
    "Metric",
    "ProducedArtifact",
    "Observation",
    "OpenRouterClient",
    "ProviderError",
    "ProxyModelProvider",
    "ResearchSummary",
    "RlmEngine",
    "RlmError",
    "Rule",
    "RuleCheck",
    "RuleRevision",
    "RunJournal",
    "SetupProposal",
    "ToolRejected",
    "VmAction",
    "VmContext",
    "VmExecutor",
    "VmResult",
]
