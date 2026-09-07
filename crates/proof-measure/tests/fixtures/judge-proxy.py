"""Judge-only egress proxy.

Runs beside the scoring container on an `internal` Docker network. The scoring
container executes untrusted miner code next to the operator holdout, so it gets
no route off the host; this process is its only peer and forwards to exactly one
upstream origin, adding the API key the container is never given.

It is deliberately not a general proxy: the request path is the only thing the
caller controls, the method is always POST, and the body is size-bounded.
Errors are sanitized; successful judge text is passed through unchanged and
may itself contain URLs or other sensitive information.
"""

from __future__ import annotations

import http.server
import json
import os
import ssl
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

MAX_BODY = 1 << 20
TIMEOUT_S = 60.0
# ponytail: only the observer's /v1 API; add explicit paths if another API is needed.
ALLOWED_PATHS = ("/v1/chat/completions", "/v1/completions", "/v1/embeddings")


def _env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise SystemExit(f"{name} is required")
    return value


LISTEN = int(os.environ.get("PROOF_JUDGE_LISTEN", "8080"))

# Upstream address AND credential arrive in one private file, never in the
# environment, so neither is visible to `docker inspect` or to anything that
# reads this container's env. The controller stages it right after create, which
# can land just after start, so wait briefly rather than racing it. Refuse to
# serve without it: a missing config must never become an open relay.
_CONFIG_PATH = _env("PROOF_JUDGE_CONFIG")
_config: dict = {}
for _ in range(100):
    try:
        with open(_CONFIG_PATH, encoding="utf-8") as handle:
            _config = json.loads(handle.read() or "{}")
    except (OSError, ValueError):
        _config = {}
    if _config.get("url") and _config.get("api_key"):
        break
    time.sleep(0.1)
if not _config.get("url") or not _config.get("api_key"):
    raise SystemExit("judge configuration was never staged")

ORIGIN = str(_config["url"]).rstrip("/")
API_KEY = str(_config["api_key"])
SNI_HOST = str(_config.get("sni_host") or "")
USE_TLS = bool(_config.get("tls"))

# The staged file has done its job; remove it so a later compromise of this
# container cannot read the credential back off the volume.
try:
    os.unlink(_CONFIG_PATH)
except OSError:
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(
    urllib.request.ProxyHandler({}),
    NoRedirect(),
    urllib.request.HTTPSHandler(context=ssl.create_default_context()),
)


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self) -> None:
        super().setup()
        self.connection.settimeout(TIMEOUT_S)

    def log_message(self, *_args) -> None:  # noqa: D102 - never echo request data
        return

    def _refuse(self, code: int, reason: str) -> None:
        # Never parse unread/rejected body bytes as another request.
        self.close_connection = True
        body = json.dumps({"error": reason}).encode()
        self.send_response(code)
        self.send_header("Connection", "close")
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self) -> None:  # noqa: N802 - stdlib signature
        # BaseHTTPRequestHandler normalizes leading //; check the raw target too.
        path = self.requestline.split()[1]
        # Only the judge's own endpoints; the upstream host is never caller-chosen.
        if path not in ALLOWED_PATHS:
            self._refuse(403, "path not allowed")
            return
        lengths = self.headers.get_all("Content-Length", [])
        if (self.headers.get_all("Transfer-Encoding") or len(lengths) != 1
                or not lengths[0].isascii() or not lengths[0].isdigit()):
            self._refuse(400, "bad length")
            return
        try:
            length = int(lengths[0])
        except ValueError:
            self._refuse(400, "bad length")
            return
        if length <= 0 or length > MAX_BODY:
            self._refuse(413, "body too large")
            return
        try:
            payload = self.rfile.read(length)
        except (TimeoutError, OSError):
            self._refuse(408, "request body unavailable")
            return
        if len(payload) != length:
            self._refuse(400, "incomplete body")
            return
        request = urllib.request.Request(ORIGIN + path, data=payload, method="POST")
        request.add_header("Content-Type", "application/json")
        # The key is added here so it never exists inside the scoring container.
        request.add_header("Authorization", f"Bearer {API_KEY}")
        # Host selects HTTP routing only: TLS SNI/certificate verification still
        # uses ORIGIN's dial hostname, not SNI_HOST. Alternate-dial TLS needs a
        # separate transport fix; never disable verification to work around it.
        if SNI_HOST and SNI_HOST != urllib.parse.urlsplit(ORIGIN).hostname:
            request.add_header("Host", SNI_HOST)
        try:
            with OPENER.open(request, timeout=TIMEOUT_S) as response:
                status = int(getattr(response, "status", 200) or 200)
                if not 200 <= status < 300:
                    self._refuse(502, "upstream unavailable")
                    return
                body = response.read(MAX_BODY + 1)
                if len(body) > MAX_BODY:
                    self._refuse(502, "upstream response too large")
                    return
        except urllib.error.HTTPError as exc:
            exc.close()
            self._refuse(502, "upstream unavailable")
            return
        except Exception:  # noqa: BLE001 - never leak upstream detail downstream
            self._refuse(502, "upstream unavailable")
            return
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802 - stdlib signature
        self._refuse(405, "method not allowed")


if __name__ == "__main__":
    sys.stderr.write(f"judge proxy listening on {LISTEN}\n")
    sys.stderr.flush()
    http.server.ThreadingHTTPServer(("0.0.0.0", LISTEN), Handler).serve_forever()
