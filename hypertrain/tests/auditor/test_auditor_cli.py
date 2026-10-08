from __future__ import annotations

import json
from typing import Any

import httpx
from aud_fixtures import manifest

from hypertrain.auditor.__main__ import RunScopedApi


def _api(run_id: str, calls: list[tuple[str, Any]]) -> RunScopedApi:
    job = {"id": "j1", "lease": "l1", "manifest": manifest().body()}

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append((request.url.path, json.loads(request.content or b"null")))
        if request.url.path == "/v1/worker/lease":
            return httpx.Response(200, json=job)
        return httpx.Response(200, json={"ok": True})

    return RunScopedApi(
        httpx.Client(transport=httpx.MockTransport(handler)), "t", "http://c", run_id
    )


def test_foreign_run_job_is_handed_back_not_replayed() -> None:
    calls: list[tuple[str, Any]] = []
    assert _api("cd" * 32, calls).lease() is None
    assert calls[-1] == (
        "/v1/worker/jobs/j1/fail",
        {"lease": "l1", "reason": "auditor serves another run", "retry": True},
    )


def test_own_run_job_is_returned() -> None:
    calls: list[tuple[str, Any]] = []
    raw = _api(manifest().run_id(), calls).lease()
    assert raw is not None and raw["id"] == "j1" and len(calls) == 1
