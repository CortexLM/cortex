#!/usr/bin/env python3
"""Harbor ``-a`` wrapper for a miner custom-Python agent.

Harbor's ``-a`` needs ``module.path:ClassName``. Miners are not required to
subclass Harbor ``BaseAgent``. This module is the primary evaluate target:
it imports the miner class named by ``PROOF_MINER_AGENT_IMPORT`` from the
staged artefact only and delegates ``run``. Built-in names such as
``terminus-2`` are never loaded here.
"""

from __future__ import annotations

import asyncio
import importlib
import inspect
import os
import sys
from pathlib import Path
from typing import Any

try:
    from harbor.agents.base import BaseAgent
except ImportError:  # pragma: no cover - unit tests without Harbor
    class BaseAgent:  # type: ignore[no-redef]
        def __init__(self, *args: Any, **kwargs: Any) -> None:
            self._init_args = args
            self._init_kwargs = kwargs


def _fail(msg: str) -> None:
    print(f"proof_python_agent: {msg}", file=sys.stderr)
    raise SystemExit(2)


def _containment_root() -> Path:
    raw = os.environ.get("PROOF_ARTIFACT_DIR", "").strip()
    if raw:
        art = Path(raw)
        if art.is_dir():
            return art.resolve()
    extra = os.environ.get("PROOF_MINER_AGENT_ROOT", "").strip()
    if extra:
        return Path(extra).resolve()
    _fail("PROOF_ARTIFACT_DIR is required to load a miner Python agent")
    raise AssertionError


def _is_inside(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except (OSError, ValueError):
        return False


def load_miner_class(import_path: str, search_root: Path, bound: Path) -> type:
    module_name, sep, class_name = import_path.partition(":")
    if not sep or not module_name or not class_name:
        _fail(f"{import_path!r} is not module.path:ClassName")
    if not class_name.isidentifier():
        _fail(f"{import_path!r} class name is not a Python identifier")
    if ".." in module_name or "/" in module_name or "\\" in module_name:
        _fail(f"{import_path!r} is not a Python import path")
    sys.path.insert(0, str(search_root))
    try:
        module = importlib.import_module(module_name)
    except Exception as e:  # noqa: BLE001 — miner import errors must fail closed
        _fail(f"cannot import {module_name} from {search_root}: {e}")
    origin = getattr(module, "__file__", None)
    if not isinstance(origin, str) or not origin:
        _fail(f"{module_name} has no file origin under the artefact")
    if not _is_inside(Path(origin), bound):
        _fail(f"{import_path} origin {origin} is outside staged artefact {bound}")
    cls = getattr(module, class_name, None)
    if cls is None or not inspect.isclass(cls):
        _fail(f"{import_path} is not a class in {module_name}")
    return cls


def _construct(cls: type, *args: Any, **kwargs: Any) -> Any:
    try:
        return cls(*args, **kwargs)
    except TypeError:
        try:
            return cls()
        except TypeError as e:
            _fail(f"cannot construct {cls.__name__}: {e}")
    raise AssertionError


def _call_run(miner: Any, instruction: str, environment: Any, context: Any) -> Any:
    run = getattr(miner, "run", None)
    if run is None or not callable(run):
        _fail(f"{type(miner).__name__} has no callable run()")
    attempts = (
        lambda: run(instruction, environment, context),
        lambda: run(instruction, environment=environment, context=context),
        lambda: run(instruction, environment),
        lambda: run(instruction),
    )
    last_err: TypeError | None = None
    for attempt in attempts:
        try:
            return attempt()
        except TypeError as e:
            last_err = e
            continue
    _fail(f"{type(miner).__name__}.run is not callable with Harbor arguments: {last_err}")
    raise AssertionError


class ProofPythonAgent(BaseAgent):
    """Harbor agent that delegates to the miner's custom Python class."""

    @staticmethod
    def name() -> str:
        return os.environ.get("PROOF_MINER_AGENT_IMPORT", "proof-python")

    def version(self) -> str | None:
        return "1"

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        import_path = os.environ.get("PROOF_MINER_AGENT_IMPORT", "").strip()
        if not import_path:
            _fail("PROOF_MINER_AGENT_IMPORT is unset")
        bound = _containment_root()
        search = Path(os.environ.get("PROOF_MINER_AGENT_ROOT", "") or str(bound))
        cls = load_miner_class(import_path, search, bound)
        self._miner = _construct(cls, *args, **kwargs)

    async def run(self, instruction: str, environment: Any = None, context: Any = None) -> Any:
        result = _call_run(self._miner, instruction, environment, context)
        if inspect.isawaitable(result):
            return await result
        if asyncio.isfuture(result):
            return await result
        return result
