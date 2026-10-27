"""The operator probe of a cuda-executor config reports what a job would see, with no secret."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from scripts.ops.s100_test import cuda_executor_check as check

from . import fake_cuda_host as fake

DIGEST = "sha256:" + "ab" * 32
IMAGE = f"{fake.REPOSITORY}@{DIGEST}"


def configure(root: Path, monkeypatch: pytest.MonkeyPatch, **host: object) -> Path:
    toolbox = fake.make_toolbox(root / "fake")
    toolbox.activate(monkeypatch)
    secrets = root / "secrets"
    secrets.mkdir(mode=0o700)
    key = secrets / "id"
    key.write_text(f"{fake.PEM}BEGIN FAKE KEY-----\n{fake.CANARY}\n{fake.PEM}END FAKE KEY-----\n")
    key.chmod(0o600)
    pinned = secrets / "known_hosts"
    pinned.write_text("192.0.2.10 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5aaaa\n")
    toolbox.scenario.install(
        {
            "192.0.2.10": {
                "port": 22,
                "user": "ubuntu",
                "identity": str(key),
                "gpus": [
                    fake.gpu(0, i, cc="10.3", sm=148, driver=fake.DRIVER, mem=810) for i in range(8)
                ],
                "images": [IMAGE],
                **host,
            }
        },
        {},
    )
    path = root / "executor.json"
    path.write_text(
        json.dumps(
            {
                "run_id": "ab" * 32,
                "image_repository": fake.REPOSITORY,
                "state_dir": str(root / "state"),
                "max_clock_skew_seconds": 20,
                "hosts": [
                    {
                        "label": "x",
                        "address": "192.0.2.10",
                        "user": "ubuntu",
                        "identity_file": str(key),
                        "known_hosts_file": str(pinned),
                    }
                ],
            }
        )
    )
    return path


def test_probe_reports_the_gpus_the_busy_ones_and_the_image(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    busy = fake.gpu_uuid(0, 3)
    path = configure(tmp_path, monkeypatch, busy=[busy])
    [row] = check.probe(path, DIGEST)
    assert row["status"] == "ok" and row["image_present"] is True
    assert len(row["gpus"]) == 8 and row["busy"] == [busy] and busy not in row["idle"]
    assert len(row["idle"]) == 7
    assert {g["driver"] for g in row["gpus"]} == {fake.DRIVER}
    assert {tuple(g["capability"]) for g in row["gpus"]} == {(10, 3)}
    assert isinstance(row["machine_id"], int) and 0 < row["machine_id"] < 1 << 52
    assert fake.CANARY not in json.dumps(row)


def test_probe_says_when_the_image_is_missing_and_when_the_host_is_unreachable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = configure(tmp_path, monkeypatch, images=[])
    [missing] = check.probe(path, DIGEST)
    assert missing["status"] == "ok" and missing["image_present"] is False
    assert check.main([str(path), DIGEST]) == 0
    down = configure(tmp_path / "down", monkeypatch, down=True)
    [lost] = check.probe(down, DIGEST)
    assert lost["status"] == "unreachable" and "ssh could not reach" in lost["detail"]
    assert check.main([str(down), DIGEST]) == 1


def test_probe_refuses_a_host_whose_clock_is_off(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = configure(tmp_path, monkeypatch, clock_skew=3600)
    [row] = check.probe(path, DIGEST)
    assert row["status"] == "refused" and "clock" in row["detail"]
