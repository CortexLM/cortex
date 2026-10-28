"""Pin this process to the CPU class the suite's manifests name, before torch is imported.

ATen, MKL and oneDNN read their variables at first use, and every manifest pins its environment
(ADR 0011, part 3): an in-process trainer or replay refuses a process that does not honor it.
Importing this module first (tests/conftest.py does) makes the test process honor it, whatever
the host or CI exported. Only conftest imports it: it has a side effect, so a test child that
must keep its own environment never does. Children are started by the launchers, which apply
the pin themselves.
"""

from __future__ import annotations

import os

from env_pin_fixtures import TEST_PIN
from hypertrain.protocol.env_pins import pinned_environ

_pinned = pinned_environ(os.environ, TEST_PIN)
for _name in set(os.environ) - set(_pinned):
    del os.environ[_name]
os.environ.update(_pinned)
