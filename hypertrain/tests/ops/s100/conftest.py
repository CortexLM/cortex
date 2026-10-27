from __future__ import annotations

from pathlib import Path

import pytest

from .sandbox import Box


@pytest.fixture(scope="session")
def golden(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """The backup the script's own `backup` command writes, taken once in a throwaway sandbox."""
    box = Box(tmp_path_factory.mktemp("golden"))
    box.run("backup", "T1")
    return box.b()


@pytest.fixture
def box(tmp_path: Path) -> Box:
    return Box(tmp_path)


@pytest.fixture
def backup(box: Box, golden: Path) -> Path:
    return box.seed_backup(golden)
