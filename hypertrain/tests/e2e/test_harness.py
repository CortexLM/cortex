from __future__ import annotations

import socket
import struct
import threading
from pathlib import Path
from typing import Any

from harness import Server


def test_server_client_does_not_reuse_expiring_idle_connection(tmp_path: Path) -> None:
    """Expire the old socket while the next request awaits its response."""
    checked, closed = threading.Event(), threading.Event()
    failures: list[BaseException] = []
    requests: list[bytes] = []
    response = b'HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n{"ok":true}'
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    listener.settimeout(10)
    port = listener.getsockname()[1]

    def serve() -> None:
        try:
            with listener:
                with listener.accept()[0] as first:
                    first.settimeout(10)
                    requests.append(first.recv(65536))
                    first.sendall(response)
                    assert checked.wait(10), "idle-pool check never reached"
                    first.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                closed.set()
                with listener.accept()[0] as second:
                    second.settimeout(10)
                    requests.append(second.recv(65536))
                    second.sendall(response)
        except (AssertionError, OSError) as error:
            failures.append(error)
            closed.set()

    def before_response(event: str, info: dict[str, Any]) -> None:
        if event == "http11.receive_response_headers.started":
            checked.set()
            assert closed.wait(10), "peer close never reached"

    thread = threading.Thread(target=serve)
    thread.start()
    server = Server(tmp_path, 3)
    try:
        url = f"http://127.0.0.1:{port}/health"
        assert server.c.get(url).status_code == 200
        assert server.c.get(url, extensions={"trace": before_response}).status_code == 200
        assert len(requests) == 2
    finally:
        checked.set()
        server.stop()
        thread.join(12)
    assert not thread.is_alive()
    assert failures == []
