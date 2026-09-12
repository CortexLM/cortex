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


class RecordingEnvironment:
    """Harbor-shaped environment: ``exec(command, cwd=None, env=None, timeout_sec=None)``."""

    def __init__(self) -> None:
        self.calls: list[dict] = []
        self.name = "recording"

    async def exec(self, command, cwd=None, env=None, timeout_sec=None):
        self.calls.append({"command": command, "cwd": cwd, "env": env, "timeout_sec": timeout_sec})
        return {"stdout": "", "return_code": 0}

    async def other(self) -> str:
        return "delegated"


class ExecTimeoutTests(unittest.TestCase):
    """The default exec wall clock is topic data (exec_timeout_s), never a
    number compiled here; a miner's explicit timeout is theirs."""

    def test_unset_topic_knob_wraps_nothing(self) -> None:
        os.environ.pop(proof_python_agent.EXEC_TIMEOUT_ENV, None)
        self.assertIsNone(proof_python_agent.exec_timeout_default())
        env = RecordingEnvironment()
        self.assertIs(proof_python_agent.wrap_environment(env, None), env)
        self.assertIsNone(proof_python_agent.wrap_environment(None, 900))

    def test_topic_default_fills_only_an_unset_timeout(self) -> None:
        import asyncio

        inner = RecordingEnvironment()
        env = proof_python_agent.wrap_environment(inner, 900)
        self.assertIsInstance(env, proof_python_agent.ExecTimeoutEnvironment)
        self.assertEqual(env.proof_exec_timeout_default_s, 900)
        asyncio.run(env.exec("ls"))
        asyncio.run(env.exec("sleep 1", timeout_sec=120))
        asyncio.run(env.exec("pwd", "/work", None, None))
        asyncio.run(env.exec("pwd", "/work", None, 30))
        asyncio.run(env.exec("make", timeout_sec=None))
        self.assertEqual([c["timeout_sec"] for c in inner.calls], [900, 120, 900, 30, 900])
        self.assertEqual(inner.calls[2]["cwd"], "/work")
        # Everything else delegates; wrapping twice is idempotent.
        self.assertEqual(env.name, "recording")
        self.assertEqual(asyncio.run(env.other()), "delegated")
        self.assertIs(proof_python_agent.wrap_environment(env, 900), env)
        env.name = "renamed"
        self.assertEqual(inner.name, "renamed")

    def test_env_var_is_read_and_shape_checked(self) -> None:
        os.environ[proof_python_agent.EXEC_TIMEOUT_ENV] = " 1800 "
        try:
            self.assertEqual(proof_python_agent.exec_timeout_default(), 1800)
        finally:
            os.environ.pop(proof_python_agent.EXEC_TIMEOUT_ENV, None)
        for bad in ("0", "-5", "soon", "1.5"):
            with self.assertRaises(SystemExit):
                proof_python_agent.exec_timeout_default({proof_python_agent.EXEC_TIMEOUT_ENV: bad})
        self.assertIsNone(proof_python_agent.exec_timeout_default({}))

    def test_agent_run_hands_the_miner_the_wrapped_environment(self) -> None:
        import asyncio

        art = HERE / "fixtures" / "python_agent"
        os.environ["PROOF_ARTIFACT_DIR"] = str(art)
        os.environ["PROOF_MINER_AGENT_IMPORT"] = "agent.agent:Agent"
        os.environ["PROOF_MINER_AGENT_ROOT"] = str(art)
        os.environ[proof_python_agent.EXEC_TIMEOUT_ENV] = "600"
        try:
            agent = proof_python_agent.ProofPythonAgent()
            seen: list = []

            class Miner:
                async def run(self, instruction, environment=None, context=None):
                    seen.append(environment)
                    await environment.exec("true")
                    return "ok"

            agent._miner = Miner()
            inner = RecordingEnvironment()
            self.assertEqual(asyncio.run(agent.run("hi", inner, None)), "ok")
            self.assertIsInstance(seen[0], proof_python_agent.ExecTimeoutEnvironment)
            self.assertEqual(inner.calls[0]["timeout_sec"], 600)
        finally:
            for k in (
                "PROOF_ARTIFACT_DIR",
                "PROOF_MINER_AGENT_IMPORT",
                "PROOF_MINER_AGENT_ROOT",
                proof_python_agent.EXEC_TIMEOUT_ENV,
            ):
                os.environ.pop(k, None)

    def test_no_compiled_timeout_number(self) -> None:
        src = (HERE.parent / "harness" / "proof_python_agent.py").read_text(encoding="utf-8")
        self.assertNotIn("timeout_sec=120", src)
        self.assertNotIn("= 120", src)


if __name__ == "__main__":
    unittest.main()
