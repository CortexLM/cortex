"""scripts/vast_e2e.py against a local fake Vast provider (real HTTP, no ssh, no spend)."""

from __future__ import annotations

import importlib.util
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import pytest

spec = importlib.util.spec_from_file_location(
    "vast_e2e", Path(__file__).parents[1] / "scripts/vast_e2e.py"
)
assert spec and spec.loader
vast = importlib.util.module_from_spec(spec)
sys.modules["vast_e2e"] = vast
spec.loader.exec_module(vast)

KEY = "fake-secret-key-0123456789"
IID = 777


class Fake:
    def __init__(self, dph: float = 0.6, credit: float = 50.0) -> None:
        self.calls: list[tuple[str, str]] = []
        self.credit, self.dph = credit, dph
        self.label: str | None = None
        self.deleted = False
        self.linger = 0
        self.throttle: dict[str, int] = {}

    def offers(self) -> list[dict]:
        base = dict(
            gpu_name="RTX 5090",
            num_gpus=2,
            verified=True,
            rentable=True,
            rented=False,
            reliability=0.995,
            inet_down=900,
            storage_cost=0.2,
            machine_id=1,
        )
        return [
            base | {"id": 11, "dph_total": self.dph + 0.3},
            base | {"id": 12, "dph_total": self.dph},
            base | {"id": 13, "dph_total": 0.01, "num_gpus": 1},
        ]

    def handle(self, method: str, url: str, body: dict | None) -> tuple[int, dict]:
        path = urlsplit(url).path
        self.calls.append((method, path))
        key = f"{method} {path}"
        if self.throttle.get(key, 0) > 0:
            self.throttle[key] -= 1
            return 429, {"error": "slow down"}
        if key == "GET /api/v0/users/current/":
            return 200, {"id": 1, "credit": self.credit}
        if key == "GET /api/v0/bundles/":
            q = json.loads(parse_qs(urlsplit(url).query)["q"][0])
            assert q["num_gpus"] == {"eq": 2} and q["gpu_name"] == {"eq": "RTX 5090"}
            return 200, {"offers": self.offers()}
        if method == "PUT" and path.startswith("/api/v0/asks/"):
            assert body and body["runtype"] == "ssh" and body["disk"] == 40
            assert body["image"] == vast.IMAGE
            self.label = body["label"]
            return 200, {"success": True, "new_contract": IID}
        if key == f"POST /api/v0/instances/{IID}/ssh/":
            return 200, {"success": True}
        if key == "GET /api/v1/instances/":
            if self.label is None or self.deleted and self.linger <= 0:
                return 200, {"success": True, "instances": []}
            if self.deleted:
                self.linger -= 1
            row = {
                "id": IID,
                "label": self.label,
                "actual_status": "running",
                "ssh_host": "127.0.0.1",
                "ssh_port": 2222,
            }
            return 200, {"success": True, "instances": [row]}
        if key == f"DELETE /api/v0/instances/{IID}/":
            self.deleted = True
            return 200, {"success": True}
        return 404, {"error": key}


@pytest.fixture
def fake():
    state = Fake()

    class Handler(BaseHTTPRequestHandler):
        def _go(self):
            assert self.headers["Authorization"] == "Bearer " + KEY
            n = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(n)) if n else None
            status, payload = state.handle(self.command, self.path, body)
            raw = json.dumps(payload).encode()
            self.send_response(status)
            if status == 429:
                self.send_header("Retry-After", "7")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        do_GET = do_PUT = do_POST = do_DELETE = _go

        def log_message(self, *a):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    state.url = f"http://127.0.0.1:{server.server_port}"
    yield state
    server.shutdown()


class Scripted(vast.Harness):
    """Real provider lifecycle; ssh side replaced by scripted outcomes."""

    remote_rc = 0
    block_remote = False

    def identity(self):
        key = self.out / "id_ed25519"
        key.with_suffix(".pub").write_text("ssh-ed25519 AAAA test")
        return key

    def ssh_probe(self, host, port):
        return True

    def upload(self):
        self.api.log("upload", rc=0)
        return 0

    def run_remote(self):
        if self.block_remote:
            assert self.cancel.wait(30), "watchdog never fired"
            return 137
        return self.remote_rc

    def rescue(self):
        (self.out / "out").mkdir(exist_ok=True)
        (self.out / "out/result.json").write_text('{"ok": false}')
        return 0


def run(fake, tmp_path, *extra, harness=Scripted, minutes="90", usd="6"):
    sleeps: list[float] = []
    argv = [
        "--key-file",
        str(tmp_path / "key"),
        "--max-usd",
        usd,
        "--max-minutes",
        minutes,
        "--api-base",
        fake.url,
        "--evidence-dir",
        str(tmp_path / "ev"),
        "--min-gap",
        "0",
        "--cleanup-reserve",
        "0",
        "--absence-attempts",
        "4",
        *extra,
    ]
    (tmp_path / "key").write_text(KEY + "\n")
    rc = vast.main(argv, harness=harness, sleep=lambda s: sleeps.append(s))
    journal = [json.loads(x) for x in (tmp_path / "ev/journal.jsonl").read_text().splitlines()]
    assert KEY not in (tmp_path / "ev/journal.jsonl").read_text()
    return rc, journal, sleeps


def events(journal):
    return [j["event"] for j in journal]


def test_refuses_over_budget_without_create(fake, tmp_path):
    fake.dph = 5.0
    rc, journal, _ = run(fake, tmp_path)
    assert rc == 2
    assert not any(m == "PUT" for m, _ in fake.calls)
    assert "refused" in events(journal)


def test_refuses_low_credit(fake, tmp_path):
    fake.credit = 10.0
    rc, _, _ = run(fake, tmp_path)
    assert rc == 2
    assert [p for _, p in fake.calls] == ["/api/v0/users/current/"]


def test_dry_run_picks_cheapest_two_gpu_offer(fake, tmp_path, capsys):
    rc, journal, _ = run(fake, tmp_path, "--dry-run")
    assert rc == 0 and not any(m == "PUT" for m, _ in fake.calls)
    chosen = json.loads(capsys.readouterr().out)["chosen_offer"]
    assert chosen["id"] == 12 and chosen["num_gpus"] == 2
    expected = 0.6 * 1.5 + 0.2 * 40 * 1.5 / 720
    assert abs(float(chosen["worst_case_usd"]) - expected) < 1e-4


def test_delete_on_remote_failure_after_rescue(fake, tmp_path):
    class Failing(Scripted):
        remote_rc = 1

    rc, journal, _ = run(fake, tmp_path, harness=Failing)
    assert rc == 1 and fake.deleted
    ev = events(journal)
    assert ev.index("rescued") < ev.index("cleanup_done")
    deletes = [i for i, j in enumerate(journal) if j.get("method") == "DELETE"]
    assert deletes and ev.index("rescued") < deletes[0]
    result = json.loads((tmp_path / "ev/harness-result.json").read_text())
    assert result["deleted"] and result["absent"] and result["remote_rc"] == 1
    assert (tmp_path / "ev/out/result.json").exists()


def test_delete_on_watchdog_timeout(fake, tmp_path):
    class Hanging(Scripted):
        block_remote = True

    rc, journal, _ = run(fake, tmp_path, harness=Hanging, minutes="0.01")
    assert rc == 1 and fake.deleted
    ev = events(journal)
    assert ev.index("watchdog_fired") < ev.index("rescued") < ev.index("cleanup_done")
    result = json.loads((tmp_path / "ev/harness-result.json").read_text())
    assert result["watchdog"] and result["absent"]


def test_429_backoff_honours_retry_after(fake, tmp_path):
    fake.throttle["GET /api/v0/users/current/"] = 2
    rc, journal, sleeps = run(fake, tmp_path, "--dry-run")
    assert rc == 0
    assert sleeps.count(7.0) == 2
    assert sum(1 for j in journal if j["event"] == "backoff_429") == 2


def test_min_gap_spacing_between_calls(fake, tmp_path):
    sleeps: list[float] = []
    api = vast.Api(fake.url, KEY, tmp_path / "j.jsonl", min_gap=5.0, sleep=sleeps.append)
    api.call("GET", "/api/v0/users/current/")
    api.call("GET", "/api/v0/users/current/")
    assert len(sleeps) == 1 and 4.0 < sleeps[0] <= 5.0


def test_absence_requires_parsed_empty_list(fake, tmp_path):
    fake.linger = 2
    rc, journal, _ = run(fake, tmp_path)
    assert rc == 0
    listings = [j for j in journal if j.get("path", "").startswith("/api/v1/instances/")]
    deleted_at = next(i for i, j in enumerate(journal) if j.get("method") == "DELETE")
    after = [j for j in listings if journal.index(j) > deleted_at]
    assert len(after) == 3
    assert json.loads((tmp_path / "ev/harness-result.json").read_text())["absent"] is True


def test_absence_never_confirmed_alerts(fake, tmp_path):
    fake.linger = 100
    rc, _, _ = run(fake, tmp_path)
    assert rc == 3
    assert json.loads((tmp_path / "ev/harness-result.json").read_text())["absent"] is False
