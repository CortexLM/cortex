#!/usr/bin/env python3
"""Harbor/LiteLLM model id: never strip openrouter/."""

from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import resolve_model  # noqa: E402


class ResolveHarborModelTests(unittest.TestCase):
    def test_prefers_param_model_with_openrouter_prefix(self) -> None:
        got = resolve_model.resolve_harbor_model(
            model_pin="moonshotai/kimi-k3",
            param_model="openrouter/moonshotai/kimi-k3",
            miner_byok="OPENROUTER_API_KEY",
        )
        self.assertEqual(got, "openrouter/moonshotai/kimi-k3")

    def test_does_not_strip_already_prefixed_pin(self) -> None:
        got = resolve_model.resolve_harbor_model(
            model_pin="openrouter/moonshotai/kimi-k3",
            miner_byok="OPENROUTER_API_KEY",
        )
        self.assertEqual(got, "openrouter/moonshotai/kimi-k3")

    def test_prepends_openrouter_when_byok_and_pin_has_no_provider(self) -> None:
        got = resolve_model.resolve_harbor_model(
            model_pin="moonshotai/kimi-k3",
            miner_byok="OPENROUTER_API_KEY",
        )
        self.assertEqual(got, "openrouter/moonshotai/kimi-k3")

    def test_prepends_when_inference_key_env_is_openrouter(self) -> None:
        got = resolve_model.resolve_harbor_model(
            model_pin="moonshotai/kimi-k3",
            inference_key_env="OPENROUTER_API_KEY",
        )
        self.assertEqual(got, "openrouter/moonshotai/kimi-k3")

    def test_does_not_prepend_for_non_openrouter_keys(self) -> None:
        got = resolve_model.resolve_harbor_model(
            model_pin="moonshotai/kimi-k3",
            miner_byok="SOME_OTHER_KEY",
        )
        self.assertEqual(got, "moonshotai/kimi-k3")

    def test_from_env_reads_param_model(self) -> None:
        env = {
            "PROOF_MODEL_PIN": "moonshotai/kimi-k3",
            "PROOF_PARAM_MODEL": "openrouter/moonshotai/kimi-k3",
            "PROOF_PARAM_MINER_BYOK": "OPENROUTER_API_KEY",
        }
        self.assertEqual(
            resolve_model.from_env(env),
            "openrouter/moonshotai/kimi-k3",
        )

    def test_restore_model_name_puts_stripped_suffix_back(self) -> None:
        self.assertEqual(
            resolve_model.restore_model_name(
                "moonshotai/kimi-k3",
                "openrouter/moonshotai/kimi-k3",
            ),
            "openrouter/moonshotai/kimi-k3",
        )
        self.assertEqual(
            resolve_model.restore_model_name(
                "openrouter/moonshotai/kimi-k3",
                "openrouter/moonshotai/kimi-k3",
            ),
            "openrouter/moonshotai/kimi-k3",
        )
        self.assertEqual(
            resolve_model.restore_model_name(
                "other/vendor/model",
                "openrouter/moonshotai/kimi-k3",
            ),
            "other/vendor/model",
        )

    def test_cli_prints_resolved_id(self) -> None:
        os.environ["PROOF_MODEL_PIN"] = "moonshotai/kimi-k3"
        os.environ["PROOF_PARAM_MODEL"] = "openrouter/moonshotai/kimi-k3"
        os.environ["PROOF_PARAM_MINER_BYOK"] = "OPENROUTER_API_KEY"
        try:
            self.assertEqual(resolve_model.from_env(), "openrouter/moonshotai/kimi-k3")
        finally:
            os.environ.pop("PROOF_MODEL_PIN", None)
            os.environ.pop("PROOF_PARAM_MODEL", None)
            os.environ.pop("PROOF_PARAM_MINER_BYOK", None)


if __name__ == "__main__":
    unittest.main()
