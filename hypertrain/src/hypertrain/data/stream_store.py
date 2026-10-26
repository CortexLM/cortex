"""Bounded transfers over existing LocalFS/S3 stores, shared conditional metadata.

S3 requires strong read-after-write consistency and conditional PutObject support.
LocalFS metadata uses flock on shared POSIX storage, not a pod's emptyDir.
"""

from __future__ import annotations

import datetime as dt
import fcntl
import hashlib
import os
import tempfile
from collections.abc import AsyncIterator, Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import BinaryIO

import anyio
import httpx

from hypertrain.data.store import (
    CorruptObjectError,
    LocalFSStore,
    ObjectNotFound,
    S3Store,
    StoreError,
    _check_key,
    sigv4_headers,
)

BLOCK = 64 << 10


class MetadataConflict(StoreError):
    """Another replica committed the same metadata generation first."""


@contextmanager
def spool() -> Iterator[BinaryIO]:
    """Disk-only temporary transfer; never materialize a whole object in RAM."""
    with tempfile.TemporaryFile() as file:
        yield file


async def receive(source: AsyncIterator[bytes], file: BinaryIO, size: int, sha: str) -> None:
    digest, received = hashlib.sha256(), 0
    with anyio.fail_after(90):
        async for data in source:
            received += len(data)
            if received > size:
                raise CorruptObjectError("stream exceeds declared size")
            digest.update(data)
            for off in range(0, len(data), BLOCK):
                file.write(data[off : off + BLOCK])
    if received != size or digest.hexdigest() != _check_key(sha):
        raise CorruptObjectError("stream hash/size mismatch")
    file.seek(0)


async def blocks(file: BinaryIO) -> AsyncIterator[bytes]:
    while data := file.read(BLOCK):
        yield data
        await anyio.lowlevel.checkpoint()


class StreamStore:
    """Content objects immutable; one bounded CAS journal shared by all replicas."""

    def __init__(self, store: LocalFSStore | S3Store, client: httpx.AsyncClient) -> None:
        self.store, self.client = store, client

    def _headers(
        self, method: str, url: str, digest: str, extra: dict[str, str] | None = None
    ) -> dict[str, str]:
        if not isinstance(self.store, S3Store):
            raise StoreError("HTTP backing requires S3")
        return {
            **sigv4_headers(
                method,
                url,
                self.store._creds,
                self.store.region,
                dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ"),
                digest,
                extra=extra,
            ),
            **(extra or {}),
        }

    async def put(self, file: BinaryIO, sha: str, size: int) -> str:
        """Caller supplies a verified spool; backing bytes rechecked before publish."""
        _check_key(sha)
        file.seek(0)
        digest, count = hashlib.sha256(), 0
        while data := file.read(BLOCK):
            digest.update(data)
            count += len(data)
        if count != size or digest.hexdigest() != sha:
            raise CorruptObjectError("unverified durable upload")
        file.seek(0)
        if isinstance(self.store, LocalFSStore):
            target = self.store._path(sha)
            target.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(dir=target.parent) as temp:
                while data := file.read(BLOCK):
                    temp.write(data)
                temp.flush()
                os.fsync(temp.fileno())
                try:
                    os.link(temp.name, target)
                except FileExistsError:
                    with spool() as check:
                        await self.get(sha, size, check)
                with _directory(target.parent):
                    pass
        else:
            url = self.store._url(sha)
            headers = self._headers("PUT", url, sha, {"If-None-Match": "*"})
            headers["Content-Length"] = str(size)
            response = await self.client.put(url, content=blocks(file), headers=headers)
            if response.status_code == 412:
                with spool() as check:
                    await self.get(sha, size, check)
            else:
                response.raise_for_status()
        return sha

    async def get(self, sha: str, size: int, file: BinaryIO) -> None:
        _check_key(sha)
        if isinstance(self.store, LocalFSStore):
            path = self.store._path(sha)
            if not path.is_file():
                raise ObjectNotFound(sha)
            with path.open("rb") as source:
                await receive(blocks(source), file, size, sha)
        else:
            url = self.store._url(sha)
            async with self.client.stream(
                "GET", url, headers=self._headers("GET", url, hashlib.sha256(b"").hexdigest())
            ) as r:
                if r.status_code == 404:
                    raise ObjectNotFound(sha)
                r.raise_for_status()
                await receive(r.aiter_raw(BLOCK), file, size, sha)

    async def delete(self, sha: str) -> None:
        _check_key(sha)
        if isinstance(self.store, LocalFSStore):
            path = self.store._path(sha)
            path.unlink(missing_ok=True)
            if path.parent.exists():
                with _directory(path.parent):
                    pass
        else:
            url = self.store._url(sha)
            response = await self.client.delete(
                url, headers=self._headers("DELETE", url, hashlib.sha256(b"").hexdigest())
            )
            response.raise_for_status()

    async def journal(self, name: str) -> tuple[bytes, str | None]:
        """The mutable journal key is not a content object; bounded to 16MiB."""
        _check_key(name)
        if isinstance(self.store, LocalFSStore):
            path = self.store.root / "metadata" / name
            if not path.exists():
                return b"", None
            if path.stat().st_size > 16 << 20:
                raise StoreError("journal exceeds metadata quota")
            data = path.read_bytes()
            return data, hashlib.sha256(data).hexdigest()
        url = self.store._url(name) + ".journal"
        async with self.client.stream(
            "GET", url, headers=self._headers("GET", url, hashlib.sha256(b"").hexdigest())
        ) as response:
            if response.status_code == 404:
                return b"", None
            response.raise_for_status()
            collected = bytearray()
            async for part in response.aiter_bytes(BLOCK):
                collected.extend(part)
                if len(collected) > 16 << 20:
                    raise StoreError("journal exceeds metadata quota")
            return bytes(collected), response.headers["etag"]

    async def compare_swap(self, name: str, data: bytes, etag: str | None) -> None:
        _check_key(name)
        if len(data) > 16 << 20:
            raise StoreError("journal exceeds metadata quota")
        if isinstance(self.store, LocalFSStore):
            directory = self.store.root / "metadata"
            directory.mkdir(exist_ok=True)
            path = directory / _check_key(name)
            with (directory / (name + ".lock")).open("ab") as lock:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    raise MetadataConflict(name) from None
                current = hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None
                if current != etag:
                    raise MetadataConflict(name)
                with tempfile.NamedTemporaryFile(dir=directory, delete=False) as temp:
                    temp.write(data)
                    temp.flush()
                    os.fsync(temp.fileno())
                    temporary = Path(temp.name)
                os.replace(temporary, path)
                with _directory(directory):
                    pass
        else:
            url = self.store._url(name) + ".journal"
            condition = {"If-Match": etag} if etag is not None else {"If-None-Match": "*"}
            response = await self.client.put(
                url,
                content=data,
                headers=self._headers("PUT", url, hashlib.sha256(data).hexdigest(), condition),
            )
            if response.status_code == 412:
                raise MetadataConflict(name)
            response.raise_for_status()

    async def mutate[T](
        self,
        name: str,
        parse: Callable[[bytes], T],
        serialize: Callable[[T], bytes],
        change: Callable[[T], None],
    ) -> T:
        """Bounded optimistic CAS: concurrent replicas never replace accepted receipts."""
        for _ in range(8):
            data, etag = await self.journal(name)
            state = parse(data)
            change(state)
            try:
                await self.compare_swap(name, serialize(state), etag)
                return state
            except MetadataConflict:
                await anyio.lowlevel.checkpoint()
                continue
        raise MetadataConflict("metadata contention exceeds bounded attempts")


@contextmanager
def _directory(path: Path) -> Iterator[None]:
    fd = os.open(path, os.O_DIRECTORY)
    try:
        os.fsync(fd)
        yield
    finally:
        os.close(fd)
