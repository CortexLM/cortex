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


if __name__ == "__main__":
    unittest.main()
