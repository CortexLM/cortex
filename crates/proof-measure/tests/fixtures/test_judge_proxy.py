"""Run with python3 test_judge_proxy.py; synthetic loopback traffic only."""

import http.client
import http.server
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest


MAX_BODY = 1 << 20
KEY = "synthetic-private-token"


class Upstream(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.do_POST()

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.server.calls.append((self.path, self.headers.get("Authorization"), body))
        mode = body.decode()
        status = int(mode) if mode.isdigit() else 200
        data = b'{"choices":[{"message":{"content":"ok"}}]}'
        if status != 200:
            data = (KEY + " http://private-upstream.invalid/secret").encode()
        elif mode == "oversize":
            data = b"x" * (MAX_BODY + 1)
        elif mode == "boundary":
            data = b"x" * MAX_BODY
        self.send_response(status)
        if 300 <= status < 400:
            self.send_header("Location", self.server.redirect)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class ProxyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.servers = []
        for _ in range(2):
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
            server.calls = []
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            cls.servers.append(server)
            cls.addClassCleanup(server.server_close)
            cls.addClassCleanup(server.shutdown)
        cls.upstream, cls.canary = cls.servers
        cls.upstream.redirect = f"http://127.0.0.1:{cls.canary.server_port}/stolen"
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            cls.port = sock.getsockname()[1]
        directory = tempfile.TemporaryDirectory()
        cls.addClassCleanup(directory.cleanup)
        config = Path(directory.name) / "judge.json"
        with open(config, "x", opener=lambda p, flags: os.open(p, flags, 0o600)) as f:
            json.dump({"url": f"http://127.0.0.1:{cls.upstream.server_port}",
                       "api_key": KEY, "tls": False}, f)
        cls.proxy = subprocess.Popen(
            [sys.executable, str(Path(__file__).with_name("judge-proxy.py"))],
            env={**os.environ, "PROOF_JUDGE_CONFIG": str(config),
                 "PROOF_JUDGE_LISTEN": str(cls.port)},
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
        cls.addClassCleanup(cls.stop_proxy)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if cls.proxy.poll() is not None:
                raise AssertionError(cls.proxy.stderr.read().decode())
            try:
                with socket.create_connection(("127.0.0.1", cls.port), timeout=0.1):
                    return
            except OSError:
                time.sleep(0.02)
        raise AssertionError("proxy did not start")

    @classmethod
    def stop_proxy(cls):
        cls.proxy.terminate()
        try:
            cls.proxy.wait(timeout=5)
        except subprocess.TimeoutExpired:
            cls.proxy.kill()
            cls.proxy.wait()
        cls.proxy.stderr.close()

    def request(self, body=b"valid", path="/v1/chat/completions"):
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        try:
            connection.request("POST", path, body=body)
            response = connection.getresponse()
            return response.status, response.read()
        finally:
            connection.close()

    def test_valid_and_boundary(self):
        for path in ("/v1/chat/completions", "/v1/completions", "/v1/embeddings"):
            status, body = self.request(path=path)
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(body)["choices"][0]["message"]["content"], "ok")
            self.assertEqual(self.upstream.calls[-1], (path, f"Bearer {KEY}", b"valid"))
        status, body = self.request(b"boundary")
        self.assertEqual((status, len(body)), (200, MAX_BODY))

    def test_redirects_errors_and_oversize(self):
        for mode in (b"301", b"302", b"303", b"307", b"308", b"400", b"401", b"500"):
            with self.subTest(mode=mode):
                self.assertEqual(self.request(mode), (502, b'{"error": "upstream unavailable"}'))
        self.assertEqual(self.canary.calls, [], "redirect delivered traffic/credential to canary")
        self.assertEqual(self.request(b"oversize"),
                         (502, b'{"error": "upstream response too large"}'))

    def test_paths_and_framing(self):
        before = len(self.upstream.calls)
        cases = [(path, b"Content-Length: 2\r\n", b"{}", 403) for path in (
            "/other/chat/completions", "/v1/chat/completions?key=x",
            "/v1/chat/completions#fragment", "//v1/chat/completions",
            "/v1/../v1/chat/completions", "/v1/%63hat/completions",
            "/v1/chat/completions/", "http://example.com/v1/chat/completions",
        )]
        for headers, body, status in (
            (b"", b"{}", 400),
            (b"Content-Length: 2\r\nContent-Length: 2\r\n", b"{}", 400),
            (b"Content-Length: 2\r\nContent-Length: 3\r\n", b"{}", 400),
            (b"Content-Length: +2\r\n", b"{}", 400),
            (b"Content-Length: -2\r\n", b"{}", 400),
            (b"Content-Length: 2, 2\r\n", b"{}", 400),
            (b"Content-Length: nope\r\n", b"{}", 400),
            (b"Content-Length: 0\r\n", b"", 413),
            (b"Content-Length: 1048577\r\n", b"{}", 413),
            (b"Content-Length: 3\r\n", b"{}", 400),
            (b"Transfer-Encoding: chunked\r\n", b"0\r\n\r\n", 400),
            (b"Content-Length: 2\r\nTransfer-Encoding: identity\r\n", b"{}", 400),
        ):
            cases.append(("/v1/chat/completions", headers, body, status))
        for path, headers, body, status in cases:
            with self.subTest(path=path, headers=headers):
                with socket.create_connection(("127.0.0.1", self.port), timeout=5) as sock:
                    sock.sendall(b"POST " + path.encode() + b" HTTP/1.1\r\nHost: proxy\r\n"
                                 + headers + b"\r\n" + body)
                    sock.shutdown(socket.SHUT_WR)
                    response = http.client.HTTPResponse(sock)
                    response.begin()
                    self.assertEqual(response.status, status)
                    self.assertEqual(response.getheader("Connection"), "close")
                    response.read()
        self.assertEqual(len(self.upstream.calls), before)


if __name__ == "__main__":
    unittest.main()
