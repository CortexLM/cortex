import importlib.util

import pytest

from cortex.cli import parser


def test_keygen_command_and_module_are_gone():
    assert importlib.util.find_spec("cortex.keygen") is None
    with pytest.raises(SystemExit):
        parser().parse_args(["keygen", "--seed-out", "a", "--public-out", "b"])
