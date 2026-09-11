#!/usr/bin/env python3
"""Resolve the Harbor / LiteLLM model id. Never strip a provider prefix.

Retained tbench-x0025: job params had ``model=openrouter/moonshotai/kimi-k3``
but Harbor ``-m`` used ``PROOF_MODEL_PIN=moonshotai/kimi-k3``. LiteLLM then
treated ``moonshotai`` as the provider (``litellm.BadRequestError``) and every
trial scored 0.

Prefer ``constraints.params.model`` (LiteLLM id) when it is the pin plus a
provider prefix. When the topic pays OpenRouter (``OPENROUTER_API_KEY``) and
the id still has no ``openrouter/`` prefix, add that prefix for Harbor / the
agent / LiteLLM. The signed ``PROOF_MODEL_PIN`` is not rewritten.
"""

from __future__ import annotations

import os
import sys

OPENROUTER_KEY = "OPENROUTER_API_KEY"
OPENROUTER_PREFIX = "openrouter/"


def resolve_harbor_model(
    model_pin: str = "",
    param_model: str = "",
    miner_byok: str = "",
    inference_key_env: str = "",
) -> str:
    pin = (model_pin or "").strip()
    param = (param_model or "").strip()
    if param and pin:
        if param == pin or param.endswith("/" + pin):
            model = param
        elif pin.endswith("/" + param):
            model = pin
        else:
            model = param
    else:
        model = param or pin
    key = (miner_byok or "").strip() or (inference_key_env or "").strip()
    if key == OPENROUTER_KEY and model and not model.startswith(OPENROUTER_PREFIX):
        model = OPENROUTER_PREFIX + model
    return model


def from_env(env: dict[str, str] | None = None) -> str:
    source = os.environ if env is None else env
    return resolve_harbor_model(
        source.get("PROOF_MODEL_PIN", ""),
        source.get("PROOF_PARAM_MODEL", ""),
        source.get("PROOF_PARAM_MINER_BYOK", ""),
        source.get("PROOF_PARAM_INFERENCE_KEY_ENV", ""),
    )


def restore_model_name(model_name: str | None, full: str) -> str:
    """If Harbor passed a stripped suffix of ``full``, put the prefix back."""
    full = (full or "").strip()
    name = (model_name or "").strip() if isinstance(model_name, str) else ""
    if not full:
        return name
    if not name:
        return full
    if full == name or full.endswith("/" + name):
        return full
    return name


def main(argv: list[str] | None = None) -> int:
    del argv
    print(from_env(), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
