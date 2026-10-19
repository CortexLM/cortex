"""scripts/vast_e2e.py against a local fake Vast provider (real HTTP, no ssh, no spend)."""

from __future__ import annotations

import importlib.util
import json
import os
import shlex
import signal
import subprocess
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
ARGS = ["--key-file", "k", "--max-usd", "1", "--max-minutes", "1"]  # parse() only


class Fake:
    def __init__(self, dph: float = 0.6, credit: float = 50.0) -> None:
        self.calls: list[tuple[str, str]] = []
        self.credit, self.dph = credit, dph
        self.label: str | None = None
        self.deleted = False
        self.linger = 0
        self.throttle: dict[str, int] = {}
        self.asks: list[str] = []
        self.gone: set[str] = set()
        self.queries: list[dict] = []

    def offers(self) -> list[dict]:
        base = dict(
            gpu_name="RTX 5090",
            num_gpus=2,
            verified=True,
            rentable=True,
            rented=False,
            reliability=0.995,
            inet_down=1200,
            storage_cost=0.2,
            machine_id=1,
            cuda_max_good=13.2,
            driver_version="580.178.04",
        )
        return [
            base | {"id": 11, "dph_total": self.dph + 0.3, "machine_id": 2},
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
            assert q["num_gpus"] == {"eq": vast.NUM_GPUS} and q["gpu_name"] == {"eq": "RTX 5090"}
            self.queries.append(q)
            if "driver_version" in q:  # real API answer, 2026-10-19
                return 400, {"error": "driver_vers in None: query values can't be None"}
            # Real API: sorted by price, server-side dph_total floor, at most `limit` offers.
            floor = q.get("dph_total", {}).get("gte", 0)
            rows = sorted(self.offers(), key=lambda o: o["dph_total"])
            return 200, {"offers": [o for o in rows if o["dph_total"] >= floor][: q["limit"]]}
        if method == "PUT" and path.startswith("/api/v0/asks/"):
            if path in self.gone:
                return 400, {"success": False, "error": "no_such_ask"}
            assert body and body["runtype"] == "ssh" and body["disk"] == 40
            assert body["image"] == vast.IMAGE
            self.label, self.deleted = body["label"], False
            self.asks.append(path)
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


def test_host_stuck_booting_is_replaced_by_next_offer(fake, tmp_path):
    class StuckFirst(Scripted):
        def ssh_probe(self, host, port):
            return len(fake.asks) > 1  # first host never answers ssh

    rc, journal, _ = run(fake, tmp_path, "--boot-timeout", "0.3", harness=StuckFirst)
    assert rc == 0
    assert fake.asks == ["/api/v0/asks/12/", "/api/v0/asks/11/"]
    ev = events(journal)
    assert ev.index("boot_timeout") < ev.index("ssh_ready")
    deletes = [i for i, j in enumerate(journal) if j.get("method") == "DELETE"]
    assert len(deletes) == 2 and deletes[0] < ev.index("ssh_ready")
    assert json.loads((tmp_path / "ev/harness-result.json").read_text())["absent"] is True


def test_every_host_stuck_still_ends_with_parsed_absence(fake, tmp_path):
    # Real run (instance 56659539): ssh never accepted the key, boot_timeout, DELETE 200, then
    # the run ended with no cleanup_done and no listing after the boot-time absence check.
    class NeverReady(Scripted):
        def ssh_probe(self, host, port):
            self.probe_error = "root@127.0.0.1: Permission denied (publickey)."
            return False

    rc, journal, _ = run(
        fake, tmp_path, "--boot-timeout", "0.3", "--boot-retries", "0", harness=NeverReady
    )
    ev = events(journal)
    assert rc == 1 and fake.asks == ["/api/v0/asks/12/"]
    assert ev.index("boot_timeout") < ev.index("cleanup_start") < ev.index("cleanup_done")
    listed = [i for i, j in enumerate(journal) if j.get("path", "").startswith("/api/v1/")]
    assert listed[-1] > ev.index("cleanup_start")  # absence re-proved at the end
    done = journal[ev.index("cleanup_done")]
    assert done["absent"] is True and done["deleted"] is True


def test_every_host_stuck_alerts_when_it_reappears(fake, tmp_path):
    class NeverReady(Scripted):
        def ssh_probe(self, host, port):
            return False

        def cleanup(self, reason):
            fake.deleted, fake.linger = True, 100  # the deleted host is listed again
            super().cleanup(reason)

    rc, _, _ = run(
        fake, tmp_path, "--boot-timeout", "0.3", "--boot-retries", "0", harness=NeverReady
    )
    assert rc == 3
    assert json.loads((tmp_path / "ev/harness-result.json").read_text())["absent"] is False


def test_excluded_offer_is_never_rented(fake, tmp_path, capsys):
    rc, _, _ = run(fake, tmp_path, "--dry-run", "--exclude-offer", "12")
    assert rc == 0 and json.loads(capsys.readouterr().out)["chosen_offer"]["id"] == 11


def test_excluded_offer_honoured_when_profile_overrides_eligible(fake, tmp_path, capsys):
    # scripts/genuine_cuda_tests.py and scripts/vast_parity.py override eligible() without the
    # exclude check: --exclude-offer was a silent no-op there.
    class OwnFilter(Scripted):
        def eligible(self, o):
            return vast.eligible(o)

    rc, _, _ = run(fake, tmp_path, "--dry-run", "--exclude-offer", "12", harness=OwnFilter)
    assert rc == 0 and json.loads(capsys.readouterr().out)["chosen_offer"]["id"] == 11


def test_vanished_offer_falls_back_to_next(fake, tmp_path):
    fake.gone = {"/api/v0/asks/12/"}
    rc, journal, _ = run(fake, tmp_path)
    assert rc == 0
    assert fake.asks == ["/api/v0/asks/11/"]
    assert "offer_gone" in events(journal)
    assert json.loads((tmp_path / "ev/harness-result.json").read_text())["absent"] is True


def test_key_reattached_after_permission_denied(fake, tmp_path):
    class Denied(Scripted):
        probes = 0

        def ssh_probe(self, host, port):
            Denied.probes += 1
            ok = Denied.probes > 2
            self.probe_error = "" if ok else "root@127.0.0.1: Permission denied (publickey)."
            return ok

    rc, journal, _ = run(fake, tmp_path, harness=Denied)
    assert rc == 0
    attaches = [c for c in fake.calls if c == ("POST", f"/api/v0/instances/{IID}/ssh/")]
    assert len(attaches) == 3
    assert [j["event"] for j in journal].count("ssh_probe_failed") == 1


@pytest.mark.parametrize("during", ["remote", "rescue"])
def test_stop_signals_never_skip_rescue_or_delete(fake, tmp_path, during):
    class Stopped(Scripted):
        def run_remote(self):
            if during == "remote":  # service manager stop/restart mid-run
                os.kill(os.getpid(), signal.SIGTERM)
                assert self.cancel.is_set()
                return 143
            return 0

        def rescue(self):
            os.kill(os.getpid(), signal.SIGTERM)  # a stop landing while cleanup runs
            os.kill(os.getpid(), signal.SIGHUP)  # and the other stop signal right after
            return super().rescue()

    before = signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGHUP)
    rc, journal, _ = run(fake, tmp_path, harness=Stopped)
    assert (signal.getsignal(signal.SIGTERM), signal.getsignal(signal.SIGHUP)) == before
    result = json.loads((tmp_path / "ev/harness-result.json").read_text())
    assert rc == 1 and fake.deleted and result["absent"] is True and not result["remote_ok"]
    assert (tmp_path / "ev/out/result.json").exists() and "cleanup_done" in events(journal)


def test_absence_never_confirmed_alerts(fake, tmp_path):
    fake.linger = 100
    rc, _, _ = run(fake, tmp_path)
    assert rc == 3
    assert json.loads((tmp_path / "ev/harness-result.json").read_text())["absent"] is False


@pytest.fixture
def mfu_globals():
    saved = vast.IMAGE, vast.NUM_GPUS, vast.MIN_PCIE_BW, vast.ALLOWED_DRIVERS
    yield
    vast.IMAGE, vast.NUM_GPUS, vast.MIN_PCIE_BW, vast.ALLOWED_DRIVERS = saved


def test_mfu_profile_picks_fast_pcie_host_and_passes_remote_env(fake, tmp_path, mfu_globals):
    offers = fake.offers

    def with_pcie() -> list[dict]:
        fast = {"pcie_bw": 53.8, "cuda_max_good": 13.3}
        out = [o | fast for o in offers()]
        out[1] |= {"pcie_bw": 26.9}  # cheapest 2x offer: slow PCIe, must be skipped
        return out

    fake.offers = with_pcie
    sent: list[str] = []

    class Env(Scripted):
        def run_remote(self):
            sent.extend(self.a.remote_env)
            return 0

    rc, journal, _ = run(
        fake, tmp_path, "--profile", "mfu", "--remote-env", "HT_MFU_MB=8", harness=Env
    )
    assert rc == 0 and sent == ["HT_MFU_MB=8"]
    assert ("PUT", "/api/v0/asks/11/") in fake.calls  # the pcie_bw 53.8 offer
    assert vast.IMAGE.startswith("docker.io/vastai/pytorch:")


def test_mfu_eligibility_floors(mfu_globals):
    vast.MIN_PCIE_BW = vast.Decimal("50")
    o = {**Fake().offers()[1], "pcie_bw": 53.8, "cuda_max_good": 13.3}
    assert vast.eligible(o)
    assert not vast.eligible({**o, "pcie_bw": 26.9})
    assert not vast.eligible({**o, "cuda_max_good": 13.0})
    assert not vast.eligible({k: v for k, v in o.items() if k != "pcie_bw"})
    vast.MIN_PCIE_BW = None
    assert vast.eligible({k: v for k, v in o.items() if k != "pcie_bw"})


def test_cuda_floor_enforced_without_pcie(mfu_globals):
    vast.MIN_PCIE_BW = None
    o = Fake().offers()[1]
    assert vast.eligible(o | {"cuda_max_good": 13.2})
    assert not vast.eligible(o | {"cuda_max_good": 13.0})
    assert not vast.eligible({k: v for k, v in o.items() if k != "cuda_max_good"})


def test_num_gpus_applies_to_default_profile(fake, tmp_path, capsys, mfu_globals):
    offers = fake.offers
    fake.offers = lambda: [*offers(), offers()[1] | {"id": 14, "num_gpus": 8, "dph_total": 2.0}]
    rc, _, _ = run(fake, tmp_path, "--num-gpus", "8", "--dry-run", usd="10")
    assert rc == 0 and fake.queries[-1]["num_gpus"] == {"eq": 8}
    chosen = json.loads(capsys.readouterr().out)["chosen_offer"]
    assert chosen["id"] == 14 and chosen["num_gpus"] == 8


def test_miner_flow_refuses_num_gpus_other_than_one(capsys):
    argv = [*ARGS, "--profile", "miner-flow"]
    assert vast.parse(argv).num_gpus == 1
    with pytest.raises(SystemExit) as e:
        vast.parse([*argv, "--num-gpus", "2"])
    assert e.value.code == 2 and "--profile miner-flow" in capsys.readouterr().err


def test_default_evidence_dir_is_repo_relative(fake, tmp_path, monkeypatch, mfu_globals):
    monkeypatch.setattr(vast, "ROOT", tmp_path)
    (tmp_path / "key").write_text(KEY + "\n")
    argv = ["--key-file", str(tmp_path / "key"), "--max-usd", "6", "--max-minutes", "90"]
    assert vast.parse(argv).evidence_dir == tmp_path / ".evidence/vast"
    argv += ["--api-base", fake.url, "--min-gap", "0", "--dry-run"]
    assert vast.main(argv, harness=Scripted, sleep=lambda s: None) == 0
    assert (tmp_path / ".evidence/vast/journal.jsonl").is_file()


def test_driver_filter_in_query_and_eligible(fake, tmp_path, mfu_globals):
    base = fake.offers()[1]
    shape = [("615.71.09", 0.1), ("580.178.04", 0.4), ("570.211.01", 0.3), (None, 0.2)]
    offers = [
        base | {"id": 20 + i, "dph_total": d, "driver_version": v} for i, (v, d) in enumerate(shape)
    ]
    del offers[3]["driver_version"]
    fake.offers = lambda: offers

    def dry(*extra):
        rc, journal, _ = run(fake, tmp_path, "--dry-run", *extra)
        chosen = [j for j in journal if j["event"] == "offer_chosen"]
        return rc, [(j["eligible"], j["quote"]["id"]) for j in chosen[-1:]]

    # The allow-list is applied client-side only: the server answers 400 to a driver filter.
    assert dry() == (0, [(2, 22)])
    assert dry("--driver", "580.178.04") == (0, [(1, 21)])
    assert dry("--any-driver") == (0, [(4, 20)])
    assert not any("driver_version" in q for q in fake.queries)
    with pytest.raises(SystemExit):
        vast.parse([*ARGS, "--driver", "999.0"])


def test_refusal_counts_driver_rejections(fake, tmp_path, mfu_globals):
    offers = fake.offers
    fake.offers = lambda: [o | {"driver_version": "615.71.09"} for o in offers()]
    rc, journal, _ = run(fake, tmp_path, "--dry-run")
    reason = next(j["reason"] for j in journal if j["event"] == "refused")
    assert rc == 2
    assert reason == "no eligible 2x RTX 5090 offer among 3 (2 rejected by driver allow-list)"


def test_qualified_offer_beyond_first_page_is_found(fake, tmp_path, capsys, mfu_globals):
    base = fake.offers()[1] | {"driver_version": "615.71.09"}
    cheap = [base | {"id": 100 + i, "dph_total": 0.1 + i / 1000} for i in range(70)]
    good = base | {"id": 99, "dph_total": 0.5, "driver_version": "580.178.04"}
    fake.offers = lambda: [*cheap, good]
    rc, journal, _ = run(fake, tmp_path, "--dry-run")
    assert rc == 0 and json.loads(capsys.readouterr().out)["chosen_offer"]["id"] == 99
    assert [q.get("dph_total") for q in fake.queries] == [None, {"gte": cheap[63]["dph_total"]}]


def test_refusal_says_search_was_capped(fake, tmp_path, mfu_globals, monkeypatch):
    monkeypatch.setattr(vast, "OFFER_PAGES", 2)
    base = fake.offers()[1] | {"driver_version": "615.71.09"}
    fake.offers = lambda: [base | {"id": 100 + i, "dph_total": 0.1 + i / 100} for i in range(200)]
    rc, journal, _ = run(fake, tmp_path, "--dry-run")
    reason = next(j["reason"] for j in journal if j["event"] == "refused")
    assert rc == 2 and len(fake.queries) == 2
    assert reason.endswith("search stopped after 2 pages of 64: cheaper-first market not exhausted")


def test_gpu8_profile_dry_run_selects_eight_gpu_qualified_offer(
    fake, tmp_path, capsys, mfu_globals
):
    base = fake.offers()[1] | {"num_gpus": 8, "cuda_max_good": 13.2}
    unqualified = [
        ("615.71.09", 11.75),
        ("575.64.03", 2.9),
        ("590.44.01", 3.4),
        ("550.163.01", 3.8),
        ("560.35.03", 4.0),
        ("565.77", 4.1),
        ("600.12", 6.2),
    ]
    offers = [
        base | {"id": 30 + i, "machine_id": 30 + i, "driver_version": v, "dph_total": d}
        for i, (v, d) in enumerate(unqualified)
    ]
    offers += [
        base | {"id": 40, "driver_version": "580.178.04", "dph_total": 4.28, "pcie_bw": 26.4},
        base | {"id": 41, "driver_version": "595.84", "dph_total": 5.66, "pcie_bw": 27.0},
    ]
    fake.offers = lambda: offers
    rc, _, _ = run(fake, tmp_path, "--profile", "gpu8", "--dry-run", minutes="30", usd="10")
    chosen = json.loads(capsys.readouterr().out)["chosen_offer"]
    assert rc == 0 and not any(m == "PUT" for m, _ in fake.calls)
    assert fake.queries[-1]["num_gpus"] == {"eq": 8}
    assert chosen["num_gpus"] == 8 and chosen["dph_total"] == 4.28
    assert chosen["driver_version"] in vast.QUALIFIED_DRIVERS
    with pytest.raises(SystemExit):
        vast.parse([*ARGS, "--profile", "gpu8", "--num-gpus", "2"])


def test_gpu_e2e_remote_gets_host_gpu_count(tmp_path):
    class Capture(vast.Harness):
        def sh(self, command, **kw):
            self.command = command
            return 0

    a = vast.parse(
        [
            *ARGS,
            "--evidence-dir",
            str(tmp_path),
            "--profile",
            "gpu8",
            "--remote-env",
            "HT_E2E_GPUS=2",
        ]
    )
    h = Capture(a, vast.Api("http://127.0.0.1:9", KEY, tmp_path / "j.jsonl"))
    assert h.run_remote() == 0
    words = shlex.split(h.command)
    assert words[words.index("bash") - 1] == "HT_E2E_GPUS=8"  # env(1): the last value wins
    remote = (vast.ROOT / "scripts/vast_e2e_remote.sh").read_text()
    assert 'N = int(os.environ.get("HT_E2E_GPUS", "2"))' in remote


@pytest.mark.parametrize("ranks", [2, 8])
def test_gpu_e2e_remote_runs_on_cpu_and_keeps_rank_logs(tmp_path, ranks):
    pytest.importorskip("torch")
    bindir = Path(sys.executable).parent
    if not (bindir / "hypertrain-miner").exists():
        pytest.skip("hypertrain-miner not installed next to this interpreter")
    env = {
        **os.environ,
        "PATH": f"{bindir}:{os.environ['PATH']}",
        "HT_E2E_BACKEND": "cpu",
        "HT_E2E_GPUS": str(ranks),
        "HT_E2E_PYTHON": sys.executable,
        "HT_E2E_SRC": str(vast.ROOT),
        "HT_E2E_OUT": str(tmp_path / "out"),
        "HT_E2E_WORK": str(tmp_path / "work"),
    }
    script = vast.ROOT / "scripts/vast_e2e_remote.sh"
    p = subprocess.run(["bash", str(script)], env=env, capture_output=True, text=True, timeout=900)
    log = tmp_path / "out/network_e2e.log"
    assert p.returncode == 0, log.read_text()[-2000:] if log.exists() else p.stderr
    e2e = json.loads((tmp_path / "out/e2e.json").read_text())
    assert [r["auditor_match"] for r in e2e["rounds"]] == [True, True] and e2e["repeat_match"]
    assert len(e2e["rounds"][0]["miner"]["files"]) == ranks * 4  # every rank's artifacts
    # Every torchrun worker's stderr lands in the rescued out/ (the 8x5090 failure left none).
    logs = sorted((tmp_path / "out/torchelastic").rglob("stderr.log"))
    assert {p.parent.name for p in logs} == {str(r) for r in range(ranks)}


def test_gpu_e2e_remote_forces_math_attention_for_the_fixture_model(tmp_path):
    # Real 2x RTX 5090 (instance 56659094): every rank died in sdpa-flash with "No available
    # kernel"; the fixture model is fp32, head_dim 4. Probe the env the remote gives its python.
    probe = tmp_path / "bin"
    probe.mkdir()
    env_dump = tmp_path / "env.json"
    (probe / "python").write_text(
        f"#!/bin/sh\nenv | grep -E '^(HT_ATTN_BACKEND|PET_)' > {env_dump}\nexit 1\n"
    )
    (probe / "python").chmod(0o755)
    env = {
        **os.environ,
        "HT_E2E_BACKEND": "cpu",
        "HT_E2E_PYTHON": str(probe / "python"),
        "HT_E2E_SRC": str(tmp_path),
        "HT_E2E_OUT": str(tmp_path / "out"),
    }
    env.pop("HT_ATTN_BACKEND", None)
    script = vast.ROOT / "scripts/vast_e2e_remote.sh"
    subprocess.run(["bash", str(script)], env=env, capture_output=True, timeout=60, check=False)
    seen = dict(line.split("=", 1) for line in env_dump.read_text().split())
    assert seen["HT_ATTN_BACKEND"] == "math" and seen["PET_TEE"] == "3"


def test_miner_cycle_profile_uses_new_remote_script(fake, tmp_path, capsys, mfu_globals):
    a = vast.parse([*ARGS, "--profile", "miner-cycle"])
    assert a.remote_script == "vast_miner_cycle_remote.sh" and a.num_gpus == 1
    assert (vast.ROOT / "scripts" / a.remote_script).is_file()
    rc, _, _ = run(fake, tmp_path, "--profile", "miner-cycle", "--dry-run")
    assert rc == 0 and vast.NUM_GPUS == 1 and vast.IMAGE == vast.MINER_FLOW_IMAGE
    assert json.loads(capsys.readouterr().out)["chosen_offer"]["id"] == 13


def test_remote_env_must_be_key_value(tmp_path):
    with pytest.raises(SystemExit):
        vast.parse(["--key-file", "k", "--max-usd", "1", "--max-minutes", "1", "--remote-env", "X"])
