"""Deterministic island trainer core (leaf scheme ht-leaf-v1).

Importing this package pins determinism before any submodule imports torch.
"""

from hypertrain.trainer.determinism import setup_determinism

DETERMINISM = setup_determinism()
