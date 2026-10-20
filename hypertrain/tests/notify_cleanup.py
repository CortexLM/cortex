"""Session-end cleanup of the empty ht-notify-* directories that tests leave in the shared /tmp."""

from __future__ import annotations

import os
from pathlib import Path


def remove_new_empty_notify_dirs(root: Path, before: set[str]) -> None:
    """rmdir only: a non-empty directory may be a concurrent same-uid session's live store."""
    for path in root.glob("ht-notify-*"):
        if path.name not in before and path.lstat().st_uid == os.getuid():
            try:
                path.rmdir()
            except OSError:
                continue  # still in use, or a store a test never closed: leave it visible
