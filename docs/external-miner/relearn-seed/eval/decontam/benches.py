"""Official-bench decontamination.

v0 ships a small pinned n-gram blocklist of well-known public bench
stems. Real eval items that match are dropped. Expand this list in
relearn CI — never train or score on official items.
"""

from __future__ import annotations

# Stems only — not full official items.
BLOCKLIST = (
    "hellaswag",
    "mmlu",
    "gsm8k",
    "arc-challenge",
    "winogrande",
    "truthfulqa",
    "gpqa",
    "aime",
)


def is_contaminated(text: str) -> bool:
    lower = text.lower()
    return any(stem in lower for stem in BLOCKLIST)
