"""Length-bounded JSON framing over Firecracker's vsock Unix bridge."""

from __future__ import annotations

import asyncio
import json
import struct
from pathlib import Path

from pydantic import ValidationError

from .models import API_VERSION, MAX_FRAME, ExecuteRequest, GuestOutput, VmError


async def read_frame(reader: asyncio.StreamReader) -> dict:
    size = struct.unpack(">I", await reader.readexactly(4))[0]
    if not 0 < size <= MAX_FRAME:
        raise VmError("invalid guest frame length")
    try:
        document = json.loads(await reader.readexactly(size))
    except (ValueError, UnicodeError):
        raise VmError("invalid guest frame JSON") from None
    if not isinstance(document, dict):
        raise VmError("guest frame must be an object")
    return document


async def write_frame(writer: asyncio.StreamWriter, value: dict) -> None:
    raw = json.dumps(value, separators=(",", ":"), allow_nan=False).encode()
    if len(raw) > MAX_FRAME:
        raise VmError("guest frame too large")
    writer.write(struct.pack(">I", len(raw)) + raw)
    await writer.drain()


class VsockTransport:
    def __init__(self, *, boot_timeout: float = 60.0, port: int = 5000):
        self.boot_timeout, self.port = boot_timeout, port

    async def exchange(self, socket_path: Path, message: dict, budget_seconds: float) -> dict:
        deadline = asyncio.get_running_loop().time() + self.boot_timeout
        reader = writer = None
        while asyncio.get_running_loop().time() < deadline:
            try:
                reader, writer = await asyncio.open_unix_connection(str(socket_path), limit=4096)
                writer.write(f"CONNECT {self.port}\n".encode())
                await writer.drain()
                async with asyncio.timeout(2):
                    handshake = await reader.readline()
                if not handshake.startswith(b"OK "):
                    raise VmError("guest vsock handshake rejected")
                break
            except (OSError, TimeoutError, VmError):
                if writer is not None:
                    writer.close()
                    await writer.wait_closed()
                reader = writer = None
                await asyncio.sleep(0.1)
        if reader is None or writer is None:
            raise VmError("guest agent did not become ready")
        try:
            async with asyncio.timeout(budget_seconds):
                await write_frame(writer, message)
                response = await read_frame(reader)
            if response.get("api_version") != API_VERSION:
                raise VmError("guest protocol version mismatch")
            if response.get("error"):
                # Guest error content may contain command output or secrets.
                raise VmError("guest rejected execution")
            return response
        except (OSError, EOFError, asyncio.IncompleteReadError, TimeoutError):
            raise VmError("guest channel unavailable") from None
        finally:
            writer.close()
            await writer.wait_closed()

    async def execute(self, socket_path: Path, job: ExecuteRequest) -> GuestOutput:
        response = await self.exchange(
            socket_path,
            {"api_version": API_VERSION, "type": "execute", "request": job.model_dump(mode="json")},
            job.action.timeout_seconds + 10,
        )
        try:
            return GuestOutput.model_validate(response["output"])
        except (KeyError, ValidationError):
            raise VmError("invalid guest output") from None
