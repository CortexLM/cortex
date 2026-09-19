"""Synthetic private evaluator material, built in memory by boundary tests."""

import base64
import hashlib
import io
import tarfile

from cortex.proof.material import HarvestMaterial
from cortex.proof.models import Topic
from cortex.protocol.crypto import public_key
from cortex.vm.setup import SetupExport, SetupManifest

from .conftest import OWNER


def harvest_export() -> SetupExport:
    files = {
        "run.py": b"print('measured fixture')\n",
        "inspect.py": b"print('inspect fixture')\n",
        "private.txt": b"synthetic held-out evaluation sample\n",
    }
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        for name, data in files.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))
    return SetupExport(
        manifest=SetupManifest(
            content_hashes=[hashlib.sha256(files["private.txt"]).hexdigest()],
            flops_budget=1_000,
            wall_budget_s=30,
        ),
        pack_b64=base64.b64encode(output.getvalue()).decode(),
    )


class FixtureMaterialSource:
    def load(self, topic: Topic) -> HarvestMaterial:
        return HarvestMaterial.from_export(harvest_export(), topic, public_key(OWNER))
