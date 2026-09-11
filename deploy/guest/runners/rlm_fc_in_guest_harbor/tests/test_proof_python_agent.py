#!/usr/bin/env python3
"""proof_python_agent.py: load miner class from the artefact only."""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import proof_python_agent  # noqa: E402


class ProofPythonAgentTests(unittest.TestCase):
    def test_loads_custom_agent_from_artefact(self) -> None:
        art = HERE / "fixtures" / "python_agent"
        os.environ["PROOF_ARTIFACT_DIR"] = str(art)
        try:
            cls = proof_python_agent.load_miner_class(
                "agent.agent:Agent",
                art,
                art.resolve(),
            )
            self.assertEqual(cls.__name__, "Agent")
            miner = cls()
            self.assertEqual(miner.run("hi"), "ok")
        finally:
            os.environ.pop("PROOF_ARTIFACT_DIR", None)

    def test_call_run_body_typeerror_is_not_retried(self) -> None:
        class Miner:
            def __init__(self) -> None:
                self.calls = 0

            def run(self, instruction, environment=None, context=None):
                self.calls += 1
                raise TypeError("paid boom")

        miner = Miner()
        with self.assertRaises(TypeError) as ctx:
            proof_python_agent._call_run(miner, "hi", object(), object())
        self.assertEqual(miner.calls, 1)
        self.assertEqual(str(ctx.exception), "paid boom")

    def test_construct_body_typeerror_is_not_retried(self) -> None:
        class Miner:
            calls = 0

            def __init__(self, *args, **kwargs):
                type(self).calls += 1
                raise TypeError("ctor boom")

        with self.assertRaises(TypeError) as ctx:
            proof_python_agent._construct(Miner, "a", logs_dir="/tmp")
        self.assertEqual(Miner.calls, 1)
        self.assertEqual(str(ctx.exception), "ctor boom")

    def test_call_run_selects_instruction_only_before_invoke(self) -> None:
        class Miner:
            def run(self, instruction):
                return f"ok:{instruction}"

        self.assertEqual(
            proof_python_agent._call_run(Miner(), "hi", object(), object()),
            "ok:hi",
        )

    def test_call_run_incompatible_signature_does_not_invoke(self) -> None:
        class Miner:
            def __init__(self) -> None:
                self.calls = 0

            def run(self, *, only_keyword: str) -> None:
                self.calls += 1

        miner = Miner()
        with self.assertRaises(SystemExit):
            proof_python_agent._call_run(miner, "hi", None, None)
        self.assertEqual(miner.calls, 0)

    def test_construct_noarg_when_harbor_args_do_not_bind(self) -> None:
        class Miner:
            def __init__(self) -> None:
                self.ok = True

        miner = proof_python_agent._construct(Miner, "logs", logs_dir="/x")
        self.assertTrue(miner.ok)

    def test_refuses_origin_outside_artefact(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            art = root / "artifact"
            ext = root / "external"
            art.mkdir()
            ext.mkdir()
            (ext / "escape.py").write_text(
                "class Agent:\n    def run(self, instruction, **kwargs):\n        return 'no'\n",
                encoding="utf-8",
            )
            os.environ["PROOF_ARTIFACT_DIR"] = str(art)
            sys.path.insert(0, str(ext))
            try:
                with self.assertRaises(SystemExit):
                    proof_python_agent.load_miner_class(
                        "escape:Agent",
                        ext,
                        art.resolve(),
                    )
            finally:
                os.environ.pop("PROOF_ARTIFACT_DIR", None)
                if sys.path[0] == str(ext):
                    sys.path.pop(0)

    def test_setup_is_async_and_harbor_abc_concrete(self) -> None:
        """Pin 8704: Harbor ABC TypeError without abstract method setup."""
        import abc
        import asyncio
        import inspect
        from typing import Any

        self.assertTrue(
            inspect.iscoroutinefunction(proof_python_agent.ProofPythonAgent.setup),
            "Harbor BaseAgent.setup is async; a sync method is not enough",
        )

        class HarborBaseAgent(abc.ABC):
            @staticmethod
            @abc.abstractmethod
            def name() -> str:
                raise NotImplementedError

            @abc.abstractmethod
            def version(self) -> str | None:
                raise NotImplementedError

            @abc.abstractmethod
            async def setup(self, environment: Any) -> None:
                raise NotImplementedError

            @abc.abstractmethod
            async def run(
                self,
                instruction: str,
                environment: Any = None,
                context: Any = None,
            ) -> Any:
                raise NotImplementedError

        class MissingSetup(HarborBaseAgent):
            @staticmethod
            def name() -> str:
                return "x"

            def version(self) -> str | None:
                return "1"

            async def run(
                self,
                instruction: str,
                environment: Any = None,
                context: Any = None,
            ) -> Any:
                return None

        with self.assertRaises(TypeError) as ctx:
            MissingSetup()
        self.assertIn("setup", str(ctx.exception).lower())

        class WithSetup(HarborBaseAgent):
            name = staticmethod(proof_python_agent.ProofPythonAgent.name)
            version = proof_python_agent.ProofPythonAgent.version
            setup = proof_python_agent.ProofPythonAgent.setup
            run = proof_python_agent.ProofPythonAgent.run

            def __init__(self) -> None:
                self._miner = type("M", (), {})()

        WithSetup()

        art = HERE / "fixtures" / "python_agent"
        os.environ["PROOF_ARTIFACT_DIR"] = str(art)
        os.environ["PROOF_MINER_AGENT_IMPORT"] = "agent.agent:Agent"
        os.environ["PROOF_MINER_AGENT_ROOT"] = str(art)
        try:
            agent = proof_python_agent.ProofPythonAgent()
            asyncio.run(agent.setup(object()))
        finally:
            os.environ.pop("PROOF_ARTIFACT_DIR", None)
            os.environ.pop("PROOF_MINER_AGENT_IMPORT", None)
            os.environ.pop("PROOF_MINER_AGENT_ROOT", None)

    def test_setup_delegates_to_miner_when_present(self) -> None:
        import asyncio

        class Miner:
            def __init__(self) -> None:
                self.seen = None

            async def setup(self, environment):
                self.seen = environment

            def run(self, instruction):
                return instruction

        miner = Miner()
        env = object()
        asyncio.run(proof_python_agent._await_maybe(proof_python_agent._call_setup(miner, env)))
        self.assertIs(miner.seen, env)

    def test_setup_keyword_only_environment_is_called(self) -> None:
        class Miner:
            def __init__(self) -> None:
                self.seen = None

            def setup(self, *, environment):
                self.seen = environment

        miner = Miner()
        env = object()
        proof_python_agent._call_setup(miner, env)
        self.assertIs(miner.seen, env)

    def test_setup_incompatible_signature_fails_closed(self) -> None:
        class Miner:
            def __init__(self) -> None:
                self.calls = 0

            def setup(self, *, only_keyword: str) -> None:
                self.calls += 1

        miner = Miner()
        with self.assertRaises(SystemExit):
            proof_python_agent._call_setup(miner, object())
        self.assertEqual(miner.calls, 0)

    def test_setup_noop_when_miner_has_none(self) -> None:
        class Miner:
            pass

        self.assertIsNone(proof_python_agent._call_setup(Miner(), object()))

    def test_restores_openrouter_prefix_on_stripped_harbor_model_name(self) -> None:
        kwargs = {"model_name": "moonshotai/kimi-k3"}
        os.environ["PROOF_HARBOR_MODEL"] = "openrouter/moonshotai/kimi-k3"
        try:
            proof_python_agent._restore_model_kwargs(kwargs)
        finally:
            os.environ.pop("PROOF_HARBOR_MODEL", None)
        self.assertEqual(kwargs["model_name"], "openrouter/moonshotai/kimi-k3")


if __name__ == "__main__":
    unittest.main()
