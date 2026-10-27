"""What the executor's R2 transfer assumes of the store (docs/adr/0013).

One body, two backends. `fake` runs on every CI run against tests/storage/fake_r2.py behind a
loopback server, so the fake's rules stay the ones this transfer relies on. `live` is opt-in: it
needs HT_R2_EXEC_TOKEN_FILE (a 0600 env file with the R2 names of a bucket-scoped token, never the
Cloudflare account token) and HT_R2_TEST_BUCKET (a `cortex-v3-test-` bucket), writes only under
`live/<random>/` and `runs/<run of this test>/`, and deletes what it wrote. With
HT_R2_ARTIFACT_MIB (default 256) it moves a real artifact through the real runner and challenge side
and, if HT_R2_ARTIFACT_EVIDENCE names a file outside the repository, writes sizes, workers and
seconds there. It never runs in CI and nothing it prints is a credential or a URL."""

from __future__ import annotations

import hashlib
import io
import json
import os
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest

from hypertrain.gpu_ops import artifact_transfer as at
from hypertrain.gpu_ops import cuda_executor_runner as runner
from hypertrain.gpu_ops.journal import Journal
from hypertrain.storage.r2 import R2Client, Retry

from .artifact_fakes import (
    FILES,
    Clock,
    MiB,
    TimedFakeR2,
    credentials_file,
    isolated_process,
    package,
    probe_ok,
    request_bytes,
    serve_http,
    sha,
)
from .test_artifact_transfer import Job

LIVE = bool(os.environ.get("HT_R2_EXEC_TOKEN_FILE") and os.environ.get("HT_R2_TEST_BUCKET"))


@pytest.fixture(autouse=True)
def process_state(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[None]:
    yield from isolated_process(tmp_path, monkeypatch)


@dataclass
class Backend:
    name: str
    credentials: Path
    wait: Callable[[float], None]  # let a URL of one second expire
    size_mib: int


@pytest.fixture(
    params=[
        "fake",
        pytest.param(
            "live",
            marks=[
                pytest.mark.cloudflare,
                pytest.mark.skipif(
                    not LIVE, reason="opt-in: set HT_R2_EXEC_TOKEN_FILE and HT_R2_TEST_BUCKET"
                ),
            ],
        ),
    ]
)
def backend(request: pytest.FixtureRequest, tmp_path: Path) -> Iterator[Backend]:
    if request.param == "live":
        path = Path(os.environ["HT_R2_EXEC_TOKEN_FILE"])
        assert os.environ["HT_R2_TEST_BUCKET"].startswith("cortex-v3-test-")
        assert at.load_credentials(path).bucket == os.environ["HT_R2_TEST_BUCKET"]
        yield Backend(
            "live",
            path,
            lambda seconds: time.sleep(seconds),
            int(os.environ.get("HT_R2_ARTIFACT_MIB", "256")),
        )
        return
    clock = Clock(time.time())
    store = TimedFakeR2(clock)
    front = serve_http(store)
    try:
        yield Backend("fake", credentials_file(tmp_path, front.base), clock.advance, 17)
    finally:
        front.close()


def status_of(response: Any) -> int:
    return int(response.status_code)


def test_the_store_answers_as_the_transfer_assumes(backend: Backend) -> None:
    creds = at.load_credentials(backend.credentials)
    assert creds.bucket.startswith("cortex-v3-test-")
    client = R2Client(creds, retry=Retry())
    http, prefix = client.http, f"live/{os.urandom(8).hex()}/"
    try:
        # a presigned PUT whose signature covers if-none-match is write-once
        once = client.presign("PUT", prefix + "once", headers={"if-none-match": "*"})
        assert status_of(http.put(once, content=b"first", headers={"if-none-match": "*"})) == 200
        assert status_of(http.put(once, content=b"second", headers={"if-none-match": "*"})) == 412
        assert client.get_object(prefix + "once") == b"first"

        # the container's preflight and polling read a key that is not there yet
        assert status_of(http.get(client.presign("GET", prefix + "absent"))) == 404

        # an expired URL is refused, with a status the runner treats as final
        short = client.presign("GET", prefix + "once", expires=1)
        assert status_of(http.get(short)) == 200
        backend.wait(2.5)
        assert status_of(http.get(short)) == 403

        # an UploadPart URL of an upload that was aborted answers 404
        key, body = prefix + "gone", b"x" * (6 * MiB)
        upload_id = client.create_multipart(key)
        part = client.presign(
            "PUT",
            key,
            query=f"partNumber=1&uploadId={upload_id}",
            headers={"x-amz-content-sha256": sha(body)},
        )
        client.abort_multipart(key, upload_id)
        assert (
            status_of(http.put(part, content=body, headers={"x-amz-content-sha256": sha(body)}))
            == 404
        )

        # a part number can be sent again; the store keeps the last one
        key = prefix + "twice"
        upload_id = client.create_multipart(key)
        client.upload_part(key, upload_id, 1, b"a" * (6 * MiB))
        second = b"b" * (6 * MiB)
        etag = client.upload_part(key, upload_id, 1, second)
        listed = client.list_parts(key, upload_id) or []
        assert [(p.number, p.etag) for p in listed] == [(1, etag)]
        assert etag.strip('"') == hashlib.md5(second, usedforsecurity=False).hexdigest()
        client.abort_multipart(key, upload_id)
    finally:
        for object_key, upload in client.list_uploads(prefix):
            client.abort_multipart(object_key, upload)
        for object_key, _ in client.list_prefix(prefix):
            client.delete_object(object_key)
    assert client.list_prefix(prefix) == [] and client.list_uploads(prefix) == []


def test_a_real_artifact_goes_through_the_runner_and_the_challenge_side(
    backend: Backend, tmp_path: Path
) -> None:
    creds = at.load_credentials(backend.credentials)
    settings = at.ArtifactSettings(
        credentials_file=backend.credentials,
        part_bytes=16 * MiB,
        workers=16 if backend.name == "live" else 3,
        poll_ms=100,
        max_artifact_bytes=64 << 30,
    )
    state = tmp_path / "state"
    state.mkdir()
    broker = at.Broker(settings, creds, Journal(state))
    job = Job()
    session = broker.begin(job, "audit")  # type: ignore[arg-type]
    assert session is not None
    raw = request_bytes(FILES, session.block)
    session.bind(raw)
    wanted = backend.size_mib

    def launch(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
        (attempt / "rank-0").mkdir()
        with (attempt / "rank-0" / "state.safetensors").open("wb") as handle:
            for _ in range(wanted):
                handle.write(os.urandom(MiB))
        return 0

    out_path = tmp_path / "status.out"
    try:
        session.start()
        out, err = io.BytesIO(), io.StringIO()
        began = time.perf_counter()
        code = runner.serve(
            io.BytesIO(package(FILES, raw)), out, err, probe=probe_ok, launch=launch
        )
        sent = time.perf_counter() - began
        session.stop_thread()
        assert code == 0, err.getvalue()
        out_path.write_bytes(out.getvalue())
        began = time.perf_counter()
        session.collect(out_path, lambda: None, lost=False)
        fetched = time.perf_counter() - began
        declared = session.declared
        assert declared is not None
        with out_path.open("rb") as stream:
            assert hashlib.file_digest(stream, "sha256").hexdigest() == declared.sha256
        evidence = {
            "backend": backend.name,
            "artifact_bytes": declared.size,
            "part_bytes": settings.part_bytes,
            "parts": len(declared.parts),
            "workers": settings.workers,
            "runner_seconds": round(sent, 2),
            "collect_seconds": round(fetched, 2),
            "upload_mb_per_s": round(declared.size / sent / 1e6, 1),
            "download_mb_per_s": round(declared.size / fetched / 1e6, 1),
        }
        print("\nR2 ARTIFACT", json.dumps(evidence, sort_keys=True))
        target = os.environ.get("HT_R2_ARTIFACT_EVIDENCE")
        if target and backend.name == "live":
            assert not Path(target).resolve().is_relative_to(Path(__file__).parents[3])
            Path(target).write_text(json.dumps(evidence, sort_keys=True) + "\n")
    finally:
        session.close()
        client = R2Client(creds, retry=Retry())
        root = f"runs/{job.run_id}/exec/"
        for object_key, upload in client.list_uploads(root):
            client.abort_multipart(object_key, upload)
        for object_key, _ in client.list_prefix(root):
            client.delete_object(object_key)
        assert client.list_prefix(root) == [] and client.list_uploads(root) == []
