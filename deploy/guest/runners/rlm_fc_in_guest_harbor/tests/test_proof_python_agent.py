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
