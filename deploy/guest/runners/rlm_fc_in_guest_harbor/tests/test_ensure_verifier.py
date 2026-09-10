#!/usr/bin/env python3
"""ensure_verifier.py: pytest must exist in Harbor environment/verifier images."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import ensure_verifier  # noqa: E402


class EnsureVerifierTests(unittest.TestCase):
    def test_python_dockerfile_gets_pytest_layer(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            task = root / "cad-model"
            env = task / "environment"
            env.mkdir(parents=True)
            df = env / "Dockerfile"
            df.write_text("FROM python:3.12-slim\nWORKDIR /app\n", encoding="utf-8")
            stats = ensure_verifier.ensure_tree(root)
            self.assertEqual(stats["dockerfiles_patched"], 1)
            text = df.read_text(encoding="utf-8")
            self.assertIn("pip install --no-cache-dir pytest", text)
            self.assertIn("python3-pytest", text)

    def test_already_has_pytest_not_duplicated(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / "quick" / "environment"
            env.mkdir(parents=True)
            df = env / "Dockerfile"
            df.write_text(
                "FROM python:3.12-slim\nRUN pip install pytest\n",
                encoding="utf-8",
            )
            stats = ensure_verifier.ensure_tree(root)
            self.assertEqual(stats["dockerfiles_already"], 1)
            self.assertEqual(stats["dockerfiles_patched"], 0)
            self.assertEqual(df.read_text(encoding="utf-8").count("pytest"), 1)

    def test_scratch_image_skipped(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / "blob" / "environment"
            env.mkdir(parents=True)
            df = env / "Dockerfile"
            df.write_text("FROM scratch\nCOPY bin /bin\n", encoding="utf-8")
            stats = ensure_verifier.ensure_tree(root)
            self.assertEqual(stats["dockerfiles_skipped"], 1)
            self.assertNotIn("pip install", df.read_text(encoding="utf-8"))

    def test_env_dockerfile_without_python_hint_still_gets_pytest(self) -> None:
        """biped/cad Harbor env images are often CUDA/MuJoCo, not python:*."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name in ("biped-contact-dynamics", "cad-model"):
                env = root / name / "environment"
                env.mkdir(parents=True)
                df = env / "Dockerfile"
                df.write_text(
                    "FROM ghcr.io/example/mujoco-runtime:latest\nWORKDIR /app\n",
                    encoding="utf-8",
                )
            stats = ensure_verifier.ensure_tree(root)
            self.assertEqual(stats["dockerfiles_patched"], 2)
            for name in ("biped-contact-dynamics", "cad-model"):
                text = (root / name / "environment" / "Dockerfile").read_text(
                    encoding="utf-8"
                )
                self.assertIn("pip install --no-cache-dir pytest", text)

    def test_requirements_txt_appends_pytest(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / "biped" / "environment"
            env.mkdir(parents=True)
            req = env / "requirements.txt"
            req.write_text("numpy\n", encoding="utf-8")
            stats = ensure_verifier.ensure_tree(root)
            self.assertEqual(stats["requirements_patched"], 1)
            self.assertIn("pytest", req.read_text(encoding="utf-8").splitlines())


if __name__ == "__main__":
    unittest.main()
