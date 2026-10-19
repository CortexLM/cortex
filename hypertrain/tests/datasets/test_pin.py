import json
import os
from collections.abc import Callable
from pathlib import Path

import httpx
import numpy as np
import pytest

from hypertrain.data.store import LocalFSStore
from hypertrain.datasets.__main__ import main
from hypertrain.datasets.shards16 import ShardSet16Manifest, shard_name16, write_shards16
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import DatasetSpec

from .test_datasets import _build, _docs

RUN = "ab" * 32
TOKEN = "admin-secret-token"


def _holdout(out: Path, seq_len: int) -> ShardSet16Manifest:
    return write_shards16([np.arange(seq_len + 1) % 200 + i for i in range(4)], out, seq_len, 4)


def _flip(shard: Path) -> None:
    raw = bytearray(shard.read_bytes())
    raw[5] ^= 0xFF
    shard.write_bytes(raw)


def test_pin_writes_service_objects(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    m, _ = _build(tmp_path / "data", _docs())
    hold = _holdout(tmp_path / "hold", m.seq_len)
    out = tmp_path / "objects"
    argv = ["pin", "--data-dir", str(tmp_path / "data"), "--out-objects", str(out)]
    assert main([*argv, "--holdout-dir", str(tmp_path / "hold")]) == 0
    got = json.loads(capsys.readouterr().out)
    n, width = m.n_samples, (m.seq_len + 1) * 2
    assert m.n_shards > 1 and got["n_samples"] == n
    store = LocalFSStore(out)
    samples = store.get(got["samples_hash"])
    proofs = json.loads(store.get(got["proofs_hash"]))
    assert len(samples) == n * width and len(proofs) == n
    for i in (0, n // 2, n - 1):
        assert MerkleTree.verify(
            samples[i * width : (i + 1) * width],
            i,
            [bytes.fromhex(p) for p in proofs[i]],
            bytes.fromhex(m.merkle_root),
            n,
        )
    assert got["dataset_fragment"] == {
        "merkle_root": m.merkle_root,
        "depth": m.depth,
        "n_samples": n,
        "sample_format": m.sample_format,
        "shard_uri_template": m.shard_uri_template,
        "shard_sha256_root": m.shard_sha256_root,
        "assign_unit": m.unit,
        "unit_sha256_root": m.unit_sha256_root,
        "holdout_commit": hold.merkle_root,
    }
    DatasetSpec.model_validate(got["dataset_fragment"])


@pytest.mark.parametrize(
    ("case", "want"),
    [
        ("data", "shard-00001.u16: sha256 mismatch"),
        ("holdout", "holdout shards differ"),
        ("holdout-seq", "holdout seq_len 8"),
    ],
)
def test_pin_refuses_bad_shards(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], case: str, want: str
) -> None:
    m, _ = _build(tmp_path / "data", _docs())
    _holdout(tmp_path / "hold", 8 if case == "holdout-seq" else m.seq_len)
    if case == "data":
        _flip(tmp_path / "data" / shard_name16(1))
    elif case == "holdout":
        _flip(tmp_path / "hold" / shard_name16(0))
    out = tmp_path / "objects"
    argv = ["pin", "--data-dir", str(tmp_path / "data"), "--out-objects", str(out)]
    assert main([*argv, "--holdout-dir", str(tmp_path / "hold")]) == 1
    err = capsys.readouterr().err
    assert want in err and (case != "data" or m.merkle_root in err)
    assert not [p for p in out.rglob("*") if p.is_file()]


def _hosted(tmp_path: Path, token: str = TOKEN) -> tuple[list[str], Path, str, str]:
    """`register` argv against a service state dir hosting RUN as a sub-store."""
    pinned = LocalFSStore(tmp_path / "pinned")
    samples, proofs = pinned.put(b"\x01\x00" * 18), pinned.put(b'[["00"]]')
    state = tmp_path / "state"
    (state / "runs" / RUN / "objects").mkdir(parents=True)
    (state / "challenge.db").touch()
    (state / "runs" / RUN / "challenge.db").touch()
    (tmp_path / "admin.token").write_text(token + "\n")
    argv = ["register", "--api", "https://master.test", "--run-id", RUN]
    argv += ["--admin-token-file", str(tmp_path / "admin.token"), "--objects", str(pinned.root)]
    argv += ["--state-dir", str(state), "--samples-hash", samples, "--proofs-hash", proofs]
    return argv, state, samples, proofs


def _serve(
    monkeypatch: pytest.MonkeyPatch, handler: Callable[[httpx.Request], httpx.Response]
) -> None:
    real = httpx.Client
    monkeypatch.setattr(
        httpx, "Client", lambda **kw: real(transport=httpx.MockTransport(handler), **kw)
    )


def test_register_copies_objects_then_links_dataset(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    argv, state, samples, proofs = _hosted(tmp_path)
    target = LocalFSStore(state / "runs" / RUN / "objects")
    seen: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        for h in (samples, proofs):  # linked objects must already be in the run's store
            assert target.get(h) == LocalFSStore(tmp_path / "pinned").get(h)
        return httpx.Response(200, json={"status": "configured"})

    _serve(monkeypatch, handler)
    before = set(state.rglob("*"))
    assert main(argv) == 0
    [req] = seen
    assert (req.method, str(req.url)) == ("PUT", f"https://master.test/v2/runs/{RUN}/admin/dataset")
    assert json.loads(req.content) == {"samples_hash": samples, "proofs_hash": proofs}
    assert req.headers["authorization"] == f"Bearer {TOKEN}"
    assert all(target.root in (p, *p.parents) for p in set(state.rglob("*")) - before)
    out = capsys.readouterr()
    assert "configured" in out.out and TOKEN not in out.out + out.err


@pytest.mark.parametrize(
    ("status", "detail"), [(422, "expected dataset object hashes"), (503, "store unavailable")]
)
def test_register_reports_service_refusal(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
    status: int,
    detail: str,
) -> None:
    argv, *_ = _hosted(tmp_path)
    _serve(monkeypatch, lambda _r: httpx.Response(status, json={"detail": detail}))
    assert main(argv) == 1
    out = capsys.readouterr()
    assert f"HTTP {status}" in out.err and detail in out.err
    assert TOKEN not in out.out + out.err


@pytest.mark.parametrize(
    ("case", "want"),
    [
        ("unhosted", "challenge.db not found"),
        ("traversal", "challenge.db not found"),
        ("no-db", "challenge.db not found"),
        ("foreign-uid", "another uid"),
        ("no-objects", "not a directory"),
        ("token", "printable ASCII"),
    ],
)
def test_register_refuses_before_any_write(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
    case: str,
    want: str,
) -> None:
    argv, state, *_ = _hosted(tmp_path, TOKEN + "\nline2" if case == "token" else TOKEN)
    i = argv.index
    if case == "unhosted":
        argv[i("--run-id") + 1] = "cd" * 32
    elif case == "traversal":
        argv[i("--run-id") + 1] = ".."
    elif case == "no-db":
        (state / "runs" / RUN / "challenge.db").unlink()
    elif case == "foreign-uid":
        monkeypatch.setattr(os, "geteuid", lambda: os.stat(state).st_uid + 1)
    elif case == "no-objects":
        argv[i("--objects") + 1] = str(tmp_path / "missing")

    def handler(_r: httpx.Request) -> httpx.Response:
        raise AssertionError("refused before any request")

    _serve(monkeypatch, handler)
    before = set(tmp_path.rglob("*"))
    assert main(argv) == 1
    assert set(tmp_path.rglob("*")) == before
    out = capsys.readouterr()
    assert want in out.err and TOKEN not in out.out + out.err


@pytest.mark.parametrize("link", ["run", "objects", "prefix"])
def test_register_refuses_symlink_into_root_store(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch: pytest.MonkeyPatch,
    link: str,
) -> None:
    argv, state, *_ = _hosted(tmp_path)
    (state / "objects").mkdir()
    run = state / "runs" / RUN
    if link == "run":
        (run / "objects").rmdir()
        (run / "challenge.db").unlink()
        run.rmdir()
        run.symlink_to(state)
    elif link == "objects":
        (run / "objects").rmdir()
        (run / "objects").symlink_to(state / "objects")
    else:
        for h in argv[argv.index("--samples-hash") + 1], argv[argv.index("--proofs-hash") + 1]:
            (state / "objects" / h[:2]).mkdir(exist_ok=True)
            if not (run / "objects" / h[:2]).exists():
                (run / "objects" / h[:2]).symlink_to(state / "objects" / h[:2])

    def handler(_r: httpx.Request) -> httpx.Response:
        raise AssertionError("refused before any request")

    _serve(monkeypatch, handler)
    assert main(argv) == 1
    assert not [p for p in (state / "objects").rglob("*") if p.is_file()]
    assert "root store" in capsys.readouterr().err
