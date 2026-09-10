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

    def test_restores_final_user_after_pytest_layer(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / "cad-model" / "environment"
            env.mkdir(parents=True)
            df = env / "Dockerfile"
            df.write_text(
                "FROM python:3.12-slim\nUSER app:group\nWORKDIR /app\n",
                encoding="utf-8",
            )
            stats = ensure_verifier.ensure_tree(root)
            self.assertEqual(stats["dockerfiles_patched"], 1)
            text = df.read_text(encoding="utf-8")
            self.assertIn("USER root\n", text)
            self.assertGreater(text.rfind("USER app:group"), text.rfind("USER root"))
            self.assertTrue(text.rstrip().endswith("USER app:group"))

    def test_restores_last_stage_user_not_build_stage(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / "quick" / "environment"
            env.mkdir(parents=True)
            df = env / "Dockerfile"
            df.write_text(
                "FROM python:3.12-slim AS build\nUSER nobody\n"
                "FROM python:3.12-slim\nUSER app\nWORKDIR /app\n",
                encoding="utf-8",
            )
            ensure_verifier.ensure_tree(root)
            text = df.read_text(encoding="utf-8")
            self.assertTrue(text.rstrip().endswith("USER app"))
            self.assertIn("USER nobody", text)

    def test_no_original_user_does_not_invent_one(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / "quick" / "environment"
            env.mkdir(parents=True)
            df = env / "Dockerfile"
            df.write_text("FROM python:3.12-slim\nWORKDIR /app\n", encoding="utf-8")
            ensure_verifier.ensure_tree(root)
            users = [
                ln.strip()
                for ln in df.read_text(encoding="utf-8").splitlines()
                if ln.strip().upper().startswith("USER ")
            ]
            self.assertEqual(users, ["USER root"])

    def test_builder_stage_pytest_does_not_skip_final_stage(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            env = root / "cad-model" / "environment"
            env.mkdir(parents=True)
            df = env / "Dockerfile"
            df.write_text(
                "FROM python:3.12-slim AS build\n"
                "RUN pip install pytest\n"
                "FROM python:3.12-slim\n"
                "WORKDIR /app\n",
                encoding="utf-8",
            )
            stats = ensure_verifier.ensure_tree(root)
            self.assertEqual(stats["dockerfiles_patched"], 1)
            self.assertEqual(stats["dockerfiles_already"], 0)
            text = df.read_text(encoding="utf-8")
            final = ensure_verifier.final_stage_text(text)
            self.assertIn("pip install --no-cache-dir pytest", final)
            self.assertGreater(text.rfind("pip install --no-cache-dir pytest"), text.rfind("FROM python:3.12-slim"))


if __name__ == "__main__":
    unittest.main()
