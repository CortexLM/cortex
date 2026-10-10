"""Explicit v2 client authority and manifest-N launcher integration."""

from __future__ import annotations

import json
from pathlib import Path

import httpx
import pytest

from hypertrain.miner.core import MinerConfig, MinerError, NetworkMiner
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import RunManifestV2


def test_client_requires_owner_authority_when_manifest_signed_by_other(tmp_path: Path):
    # Given: valid signature, wrong pinned owner.
    import importlib.util
    import sys

    spec = importlib.util.spec_from_file_location(
        "client_fixture", Path(__file__).parents[1] / "ledger/test_escrow_v2.py"
    )
    fixture = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = fixture
    spec.loader.exec_module(fixture)
    manifest = fixture.setup().manifest
    key = Keypair(b"a" * 32)
    keyfile = tmp_path / "miner.key"
    keyfile.write_bytes(b"a" * 32)
    keyfile.chmod(0o600)
    cfg = MinerConfig(
        "https://service.test",
        keyfile,
        tmp_path,
        "unused",
        manifest.training.reference_spec.image_digest,
        run_id=manifest.run_id(),
        owner_hotkey=fixture.COORD.ss58,
    )
    transport = httpx.MockTransport(
        lambda _: httpx.Response(
            200,
            json={
                "manifest_envelope": seal(key, "RunManifestV2", manifest.run_id(), manifest, 1000)
            },
        )
    )
    # When / Then
    with httpx.Client(transport=transport) as client, pytest.raises(MinerError, match="authority"):
        NetworkMiner(cfg, client)


def test_two_rank_manifest_launches_real_runtime(tmp_path: Path):
    # Given: real staged signed dataset, strict two-rank wrapper.
    import importlib.util
    import sys

    spec = importlib.util.spec_from_file_location(
        "rank_fixture", Path(__file__).parents[1] / "ledger/test_escrow_v2.py"
    )
    fixture = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = fixture
    spec.loader.exec_module(fixture)
    s = fixture.setup()
    body = s.manifest.body()
    body["training"]["reference_spec"]["layout"].update(n_gpus=2, dp_size=2, ep_size=1, zero1=True)
    wrapper = RunManifestV2.model_validate(body)
    s = fixture.Setup(wrapper, s.policy, s.admission_policy, s.rows, s.tree)
    job = fixture.stage(s, tmp_path / "attempt", (0, 1), 0)
    (tmp_path / "attempt/job.json").write_text(job.model_dump_json())
    keyfile = tmp_path / "key"
    keyfile.write_bytes(b"b" * 32)
    keyfile.chmod(0o600)
    cfg = MinerConfig(
        "https://service.test",
        keyfile,
        tmp_path / "work",
        "unused",
        wrapper.training.reference_spec.image_digest,
        run_id=wrapper.run_id(),
        owner_hotkey=fixture.COORD.ss58,
    )
    transport = httpx.MockTransport(
        lambda _: httpx.Response(
            200,
            json={
                "manifest_envelope": seal(
                    fixture.COORD, "RunManifestV2", wrapper.run_id(), wrapper, 1000
                )
            },
        )
    )
    with httpx.Client(transport=transport) as client:
        network = NetworkMiner(cfg, client)
        # When
        result = network.launch(tmp_path / "attempt/job.json")
    # Then: two real rank artifacts, one identical island commitment.
    summary0 = json.loads((tmp_path / "attempt/published/rank-0/summary.json").read_bytes())
    summary1 = json.loads((tmp_path / "attempt/published/rank-1/summary.json").read_bytes())
    assert summary0["commitments"] == summary1["commitments"]
    assert Path(result["state"]).exists()


def test_run_v2_cli_dispatches_exact_round_argument(monkeypatch, capsys, tmp_path):
    from hypertrain.miner import cli

    # Parser-only contract; actual NetworkMiner.run_round has separate real HTTP/TLS proof.
    calls = []

    class ParserActor:
        def __init__(self, cfg, client):
            pass

        def run_round(self, w):
            calls.append(w)
            return "UPLOADED"

    monkeypatch.setattr(cli, "NetworkMiner", ParserActor)
    monkeypatch.setattr(cli.MinerConfig, "load", lambda _: object())
    assert cli.main(["run-v2", "--round", "7"]) == 0
    assert calls == [7]
    assert json.loads(capsys.readouterr().out) == {"status": "UPLOADED"}
