"""Guest daemon entry point: authenticated host channel supplied by Firecracker."""

from __future__ import annotations

import argparse
import asyncio
import json
import socket
import struct
import sys
import uuid
from pathlib import Path

from pydantic import BaseModel

from cortex.rlm import (
    AgentLimits,
    AgentRequest,
    GuestRlmService,
    KnowledgeStore,
    ProxyModelProvider,
    RunJournal,
)
from cortex.rlm.memory_proxy import SharedKnowledgeClient
from cortex.rlm.models import VmAction, VmContext, VmResult

from .guest import GuestExecutor, GuestIdentity
from .models import API_VERSION, MAX_FRAME, ExecuteRequest, VmError
from .transport import read_frame, write_frame


class HostCallback:
    async def exchange(self, message: dict) -> dict:
        sock = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
        sock.setblocking(False)
        writer = None
        try:
            async with asyncio.timeout(7500):
                await asyncio.get_running_loop().sock_connect(sock, (2, 5001))
                reader, writer = await asyncio.open_connection(sock=sock)
                await write_frame(writer, message)
                response = await read_frame(reader)
            if response.get("error"):
                raise VmError("host callback refused")
            return response
        finally:
            if writer is not None:
                writer.close()
                await writer.wait_closed()
            else:
                sock.close()


class ToolExecutor:
    def __init__(self, callback: HostCallback):
        self.callback = callback

    async def execute(self, context: VmContext, action: VmAction) -> VmResult:
        return await self.execute_once(context, action, f"tool-{uuid.uuid4().hex}")

    async def execute_once(
        self, context: VmContext, action: VmAction, execution_id: str
    ) -> VmResult:
        return await self._exchange("execute", context, action, execution_id)

    async def reconcile(self, context: VmContext, action: VmAction, execution_id: str) -> VmResult:
        return await self._exchange("reconcile", context, action, execution_id)

    async def _exchange(
        self, kind: str, context: VmContext, action: VmAction, execution_id: str
    ) -> VmResult:
        response = await self.callback.exchange(
            {
                "type": kind,
                "request": {
                    "execution_id": execution_id,
                    "context": context.model_dump(mode="json"),
                    "action": action.model_dump(mode="json"),
                },
            }
        )
        return VmResult.model_validate(response["result"])


class GuestServer:
    def __init__(
        self,
        identity: GuestIdentity,
        *,
        workspace: Path = Path("/workspace"),
        runners_dir: Path = Path("/opt/proof/runners"),
        callback: HostCallback | None = None,
        knowledge: KnowledgeStore | None = None,
    ):
        self.identity = identity
        self.executor = GuestExecutor(identity, workspace=workspace, runners_dir=runners_dir)
        self.workspace = workspace
        self.callback = callback or HostCallback()
        self.knowledge = knowledge

    async def handle(self, message: dict) -> dict:
        output: BaseModel
        if message.get("api_version") != API_VERSION:
            raise VmError("guest protocol version mismatch")
        if message.get("type") == "execute":
            output = await self.executor.execute(ExecuteRequest.model_validate(message["request"]))
        elif message.get("type") == "artifact_fetch":
            from .fetch import fetch_artifact

            encoded = await fetch_artifact(
                self.identity, VmContext.model_validate(message["context"]), message["uri"]
            )
            return {"api_version": API_VERSION, "artifact_b64": encoded}
        elif message.get("type") == "agent":
            if self.identity.kind != "topic":
                raise VmError("RLM must run in its topic VM")
            journal = self.workspace / "rlm-journal"
            journal.mkdir(mode=0o700, exist_ok=True)
            run_journal = RunJournal(journal)
            try:
                request = AgentRequest.model_validate(message["request"])
                knowledge = (
                    SharedKnowledgeClient(request.task.context, self.callback.exchange)
                    if message.get("shared_knowledge") is True
                    else self.knowledge
                )
                service = GuestRlmService(
                    binding=self.identity,
                    provider=ProxyModelProvider(
                        model=message["model"], exchange=self.callback.exchange
                    ),
                    executor_factory=lambda task: ToolExecutor(self.callback),
                    journal=run_journal,
                    limits=AgentLimits.model_validate(message["limits"]),
                    knowledge=knowledge,
                )
                output = await service.run(request)
            finally:
                run_journal.close()
        else:
            raise VmError("unknown guest message")
        return {"api_version": API_VERSION, "output": output.model_dump(mode="json")}

    async def connection(self, reader, writer):
        try:
            response = await self.handle(await read_frame(reader))
        except Exception:
            response = {"api_version": API_VERSION, "error": "guest execution failed"}
        try:
            await write_frame(writer, response)
        finally:
            writer.close()
            await writer.wait_closed()


async def serve(server: GuestServer) -> None:
    sock = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
    sock.bind((socket.VMADDR_CID_ANY, 5000))
    sock.listen(32)
    sock.setblocking(False)
    async with await asyncio.start_server(server.connection, sock=sock) as listener:
        await listener.serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser(description="Proof guest agent (requires VM boot binding)")
    parser.add_argument("--stdio", action="store_true")
    parser.add_argument("--knowledge-dir", type=Path, default=Path("/workspace/research-memory"))
    parser.add_argument("--knowledge-owner-public-key-file", type=Path)
    args = parser.parse_args()
    knowledge = None
    if args.knowledge_owner_public_key_file is not None:
        if args.knowledge_owner_public_key_file.is_symlink():
            raise VmError("knowledge owner public key must be a regular file")
        try:
            public_key = bytes.fromhex(args.knowledge_owner_public_key_file.read_text().strip())
        except (OSError, ValueError):
            raise VmError("knowledge owner public key unavailable") from None
        if len(public_key) != 32:
            raise VmError("knowledge owner public key must be 32 bytes")
        args.knowledge_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        if args.knowledge_dir.is_symlink() or args.knowledge_dir.stat().st_mode & 0o077:
            raise VmError("knowledge directory must be private")
        knowledge = KnowledgeStore(
            args.knowledge_dir / "knowledge.sqlite3", owner_public_key=public_key
        )
    server = GuestServer(GuestIdentity.from_system(), knowledge=knowledge)
    if args.stdio:
        while header := sys.stdin.buffer.read(4):
            if len(header) != 4:
                raise VmError("truncated frame")
            length = struct.unpack(">I", header)[0]
            if not 0 < length <= MAX_FRAME:
                raise VmError("invalid frame length")
            message = json.loads(sys.stdin.buffer.read(length))
            response = asyncio.run(server.handle(message))
            raw = json.dumps(response, allow_nan=False).encode()
            sys.stdout.buffer.write(struct.pack(">I", len(raw)) + raw)
            sys.stdout.buffer.flush()
    else:
        asyncio.run(serve(server))


if __name__ == "__main__":
    main()
