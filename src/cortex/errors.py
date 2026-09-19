"""Public failures carry stable reasons, never upstream bodies or credentials."""


class ServiceError(Exception):
    def __init__(self, status: int, reason: str):
        super().__init__(reason)
        self.status = status
        self.reason = reason
