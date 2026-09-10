#!/usr/bin/env python3
"""resolve_agent.py: AST import path, refuse built-in names."""

from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import resolve_agent  # noqa: E402


class ResolveAgentTests(unittest.TestCase):
    def test_custom_python_agent_class(self) -> None:
        d = HERE / "fixtures" / "python_agent" / "agent"
        info = resolve_agent.inspect_agent(d)
        self.assertEqual(info["import_path"], "agent.agent:Agent")
        self.assertEqual(info["kind"], "python")

    def test_top_level_agent_dir(self) -> None:
        d = HERE / "fixtures" / "agent"
        path, pythonpath = resolve_agent.discover(d)
        self.assertEqual(path, "agent.agent:MinerAgent")
        self.assertEqual(Path(pythonpath), d.parent.resolve())

    def test_recipe_agent_dir(self) -> None:
        d = HERE / "fixtures" / "recipe" / "agent"
        path, pythonpath = resolve_agent.discover(d)
        self.assertEqual(path, "agent.agent:RecipeAgent")
        self.assertEqual(Path(pythonpath), (HERE / "fixtures" / "recipe").resolve())

    def test_import_path_file_wins(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            agent = Path(tmp) / "agent"
            agent.mkdir()
            (agent / "agent.py").write_text(
                "from harbor.agents.base import BaseAgent\n"
                "class A(BaseAgent):\n    pass\n"
                "class B(BaseAgent):\n    pass\n",
                encoding="utf-8",
            )
            with self.assertRaises(SystemExit):
                resolve_agent.discover(agent)
            (agent / "import_path").write_text("agent.agent:B\n", encoding="utf-8")
            path, _ = resolve_agent.discover(agent)
            self.assertEqual(path, "agent.agent:B")

    def test_builtin_name_in_import_path_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            agent = Path(tmp) / "agent"
            agent.mkdir()
            (agent / "import_path").write_text("terminus-2\n", encoding="utf-8")
            with self.assertRaises(SystemExit) as ctx:
                resolve_agent.discover(agent)
            self.assertEqual(ctx.exception.code, 2)

    def test_empty_dir_is_not_an_agent(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp) / "agent"
            d.mkdir()
            self.assertFalse(resolve_agent.is_agent_dir(d))

    def test_recipe_run_sh_alone_is_not_an_agent_dir(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            recipe = Path(tmp) / "recipe"
            recipe.mkdir()
            (recipe / "run.sh").write_text("#!/bin/sh\necho classic\n", encoding="utf-8")
            self.assertFalse(resolve_agent.is_agent_dir(recipe))

    def test_import_path_outside_artefact_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            artefact = root / "artifact"
            agent = artefact / "agent"
            external = root / "external"
            agent.mkdir(parents=True)
            external.mkdir()
            (external / "outside_agent.py").write_text(
                "from harbor.agents.base import BaseAgent\n"
                "class ExternalAgent(BaseAgent):\n    pass\n",
                encoding="utf-8",
            )
            (agent / "import_path").write_text("outside_agent:ExternalAgent\n", encoding="utf-8")
            old_pp = os.environ.get("PYTHONPATH")
            os.environ["PYTHONPATH"] = str(external)
            os.environ["PROOF_ARTIFACT_DIR"] = str(artefact)
            try:
                with self.assertRaises(SystemExit) as ctx:
                    resolve_agent.discover(agent)
                self.assertEqual(ctx.exception.code, 2)
                self.assertFalse(resolve_agent.is_agent_dir(agent))
            finally:
                os.environ.pop("PROOF_ARTIFACT_DIR", None)
                if old_pp is None:
                    os.environ.pop("PYTHONPATH", None)
                else:
                    os.environ["PYTHONPATH"] = old_pp

    def test_stdlib_import_path_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            agent = Path(tmp) / "agent"
            agent.mkdir()
            (agent / "import_path").write_text("json:JSONDecoder\n", encoding="utf-8")
            with self.assertRaises(SystemExit) as ctx:
                resolve_agent.discover(agent)
            self.assertEqual(ctx.exception.code, 2)

    def test_import_path_inside_artefact_still_resolves(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            artefact = Path(tmp) / "artifact"
            agent = artefact / "agent"
            agent.mkdir(parents=True)
            (agent / "agent.py").write_text(
                "from harbor.agents.base import BaseAgent\n"
                "class InsideAgent(BaseAgent):\n    pass\n",
                encoding="utf-8",
            )
            (agent / "import_path").write_text("agent.agent:InsideAgent\n", encoding="utf-8")
            os.environ["PROOF_ARTIFACT_DIR"] = str(artefact)
            try:
                path, pythonpath = resolve_agent.discover(agent)
            finally:
                os.environ.pop("PROOF_ARTIFACT_DIR", None)
            self.assertEqual(path, "agent.agent:InsideAgent")
            self.assertEqual(Path(pythonpath), artefact.resolve())


if __name__ == "__main__":
    unittest.main()
