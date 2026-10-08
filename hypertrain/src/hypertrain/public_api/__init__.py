"""Read-only public API (B3), mounted at /public on the challenge app."""

from __future__ import annotations

import hashlib
import json
import sqlite3
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse, Response

from hypertrain.public_api import views

__all__ = ["create_public_app"]

OK_CACHE = "public, max-age=15, s-maxage=60, stale-while-revalidate=300"


def _metrics(path: Path | None) -> list[dict[str, Any]]:
    if path is None or not path.is_file():
        return []
    out = []
    for line in path.read_text().splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if (
            isinstance(row, dict)
            and type(row.get("w")) is int
            and isinstance(row.get("value"), (int, float))
        ):
            out.append(row)
    return out


def create_public_app(
    db_path: Path,
    metrics_path: Path | None = None,
    *,
    clock: Callable[[], float] = time.time,
    ttl: float = 15.0,
) -> FastAPI:
    app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)
    cache: dict[str, tuple[float, int, Any]] = {}

    def read(path: str) -> tuple[int, Any]:
        """(status, body) from a read-only connection; cached per path for `ttl` seconds."""
        now = clock()
        hit = cache.get(path)
        if hit and now - hit[0] < ttl:
            return hit[1], hit[2]
        try:
            db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
            db.row_factory = sqlite3.Row
            try:
                status, body = build(db, path)
            finally:
                db.close()
        except sqlite3.Error:
            return 503, {"detail": "database unreadable"}
        cache[path] = (now, status, body)
        return status, body

    def build(db: sqlite3.Connection, path: str) -> tuple[int, Any]:
        metrics = _metrics(metrics_path)
        if path == "/v1/runs":
            return 200, views.list_runs(db)
        if path == "/v1/snapshot":
            snap = views.default_snapshot(db, metrics)
            return (200, snap) if snap else (404, {"detail": "no runs"})
        run_id, _, tail = path.removeprefix("/v1/runs/").partition("/")
        snap = views.run_snapshot(
            db, run_id, [x for x in metrics if x.get("run_id", run_id) == run_id]
        )
        if snap is None:
            return 404, {"detail": "unknown run"}
        if tail == "model":
            row = db.execute("SELECT manifest FROM runs WHERE run_id=?", (run_id,)).fetchone()
            desc = views.model_description(json.loads(row["manifest"]))
            if "manifest" in desc:
                desc["manifest"]["runId"] = run_id
            return 200, desc
        return 200, snap

    def respond(request: Request, path: str) -> Response:
        status, body = read(path)
        if status != 200:
            cc = "public, max-age=10" if status == 404 else "no-store"
            return JSONResponse(body, status_code=status, headers={"Cache-Control": cc})
        raw = json.dumps(body, separators=(",", ":"), sort_keys=True).encode()
        etag = '"' + hashlib.sha256(raw).hexdigest() + '"'
        headers = {"Cache-Control": OK_CACHE, "ETag": etag}
        if request.headers.get("if-none-match") == etag:
            return Response(status_code=304, headers=headers)
        return Response(raw, media_type="application/json", headers=headers)

    @app.get("/v1/runs")
    def runs(request: Request) -> Response:
        return respond(request, "/v1/runs")

    @app.get("/v1/snapshot")
    def snapshot(request: Request) -> Response:
        return respond(request, "/v1/snapshot")

    @app.get("/v1/runs/{run_id}")
    def run(run_id: str, request: Request) -> Response:
        return respond(request, f"/v1/runs/{run_id}")

    @app.get("/v1/runs/{run_id}/model")
    def model(run_id: str, request: Request) -> Response:
        return respond(request, f"/v1/runs/{run_id}/model")

    return app
