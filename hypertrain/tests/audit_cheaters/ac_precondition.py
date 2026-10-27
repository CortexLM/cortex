"""A scenario step that must hold today but did not: the test is broken, the hole is not open.

PreconditionError is deliberately not an AssertionError: an `xfail(raises=AssertionError)` test
then reports FAILED instead of hiding a broken setup behind the expected failure of the defence
it reproduces. Every module of the suite that sets a scenario up for a strict xfail uses it.
"""

from __future__ import annotations


class PreconditionError(Exception):
    """A step the scenario relies on did not hold; see the module docstring."""


def precondition(condition: bool, message: str) -> None:
    if not condition:
        raise PreconditionError(message)
