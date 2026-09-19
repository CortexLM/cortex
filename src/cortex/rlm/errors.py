"""Errors deliberately contain no remote response, prompt, or credential data."""


class RlmError(Exception):
    pass


class ProviderError(RlmError):
    pass


class InvalidResponse(RlmError):
    pass


class BudgetExceeded(RlmError):
    pass


class ToolRejected(RlmError):
    pass
