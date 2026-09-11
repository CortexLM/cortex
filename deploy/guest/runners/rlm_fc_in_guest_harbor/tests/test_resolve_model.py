#!/usr/bin/env python3
"""Harbor/LiteLLM model id: PARAM_MODEL else PIN; OpenRouter fail-closed."""

from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "harness"))
import resolve_model  # noqa: E402


class ResolveHarborModelTests(unittest.TestCase):
    def test_prefers_param_model_over_pin(self) -> None:
        got = resolve_model.resolve_harbor_model(
            model_pin="moonshotai/kimi-k3",
            param_model="openrouter/moonshotai/kimi-k3",
            miner_byok="OPENROUTER_API_KEY",
        )
        self.assertEqual(got, "openrouter/moonshotai/kimi-k3")

    def test_falls_back_to_pin_when_param_unset(self) -> None:
        got = resolve_model.resolve_harbor_model(
            model_pin="moonshotai/kimi-k3",
            miner_byok="SOME_OTHER_KEY",
        )
        self.assertEqual(got, "moonshotai/kimi-k3")

    def test_openrouter_without_provider_prefix_fails_closed(self) -> None:
        with self.assertRaises(SystemExit):
            resolve_model.resolve_harbor_model(
                model_pin="moonshotai/kimi-k3",
                miner_byok="OPENROUTER_API_KEY",
            )

    def test_openrouter_inference_key_env_without_prefix_fails_closed(self) -> None:
        with self.assertRaises(SystemExit):
            resolve_model.resolve_harbor_model(
                model_pin="moonshotai/kimi-k3",
                inference_key_env="OPENROUTER_API_KEY",
            )

    def test_openrouter_inference_key_env_checked_when_byok_is_other(self) -> None:
        with self.assertRaises(SystemExit):
            resolve_model.resolve_harbor_model(
                model_pin="moonshotai/kimi-k3",
                miner_byok="OTHER_KEY",
                inference_key_env="OPENROUTER_API_KEY",
            )

    def test_openrouter_csv_trims_whitespace_around_names(self) -> None:
        with self.assertRaises(SystemExit):
            resolve_model.resolve_harbor_model(
                model_pin="moonshotai/kimi-k3",
                miner_byok="OTHER_KEY, OPENROUTER_API_KEY",
            )

    def test_does_not_rewrite_pin_for_non_openrouter_keys(self) -> None:
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
