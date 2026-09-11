#!/usr/bin/env python3
"""Harbor / LiteLLM model id: ``PROOF_PARAM_MODEL`` else ``PROOF_MODEL_PIN``.

Canon ``constraints.model_pin`` is ``vendor/model`` only (``proof-canon``
rejects ``a/b/c``). The guest injects the Harbor/LiteLLM id as
``PROOF_PARAM_MODEL`` (e.g. ``openrouter/moonshotai/kimi-k3``). Harbor ``-m``
is ``MODEL="${PROOF_PARAM_MODEL:-$PROOF_MODEL_PIN}"``. An OpenRouter path
(``miner_byok`` / ``inference_key_env`` = ``OPENROUTER_API_KEY``) fails closed
unless that id already has the ``openrouter/`` provider prefix — the pin is
not rewritten.
"""

from __future__ import annotations

import os
import sys

OPENROUTER_KEY = "OPENROUTER_API_KEY"
OPENROUTER_PREFIX = "openrouter/"


def _fail(msg: str, code: int = 2) -> None:
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def _openrouter_path(miner_byok: str, inference_key_env: str) -> bool:
    for raw in (miner_byok, inference_key_env):
        for part in (raw or "").split(","):
            if part.strip() == OPENROUTER_KEY:
                return True
    return False


def resolve_harbor_model(
    model_pin: str = "",
    param_model: str = "",
    miner_byok: str = "",
    inference_key_env: str = "",
) -> str:
    # MODEL="${PROOF_PARAM_MODEL:-$PROOF_MODEL_PIN}"
    model = (param_model or "").strip() or (model_pin or "").strip()
    if not model:
        return ""
    if _openrouter_path(miner_byok, inference_key_env) and not model.startswith(
        OPENROUTER_PREFIX
    ):
        _fail(
            "OpenRouter Harbor -m needs a provider prefix "
            "(params.model=openrouter/...), got a vendor/model pin"
        )
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
