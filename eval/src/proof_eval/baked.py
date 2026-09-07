"""Proxy ids this image is declared to contain.

The live pin ships no HF bake. An empty list is the contract: score must
see local weights at `PROOF_PROXY_MODEL_DIR`, not a downloaded id.
"""

from __future__ import annotations

import json
from pathlib import Path

from .contract import BAKED_PROXIES_PATH, ContractError

_SHIPPED = Path(__file__).with_name("baked_proxies.json")


def baked_proxies() -> list[str]:
    for path in (Path(BAKED_PROXIES_PATH), _SHIPPED):
        if path.is_file():
            data = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(data, list) and all(isinstance(x, str) for x in data):
                return [x.strip() for x in data if x.strip()]
    return []


def require_baked(proxy: str) -> str:
    want = proxy.strip()
    if not want:
        raise ContractError("proxy_model is empty; this image has no HF bake")
    if want not in baked_proxies():
        raise ContractError(f"proxy_model {want!r} is not baked into this image")
    return want
