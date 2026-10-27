"""Export a staged admission trial into a copyable probe-v2 folder."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tempfile
from pathlib import Path

import httpx
from pydantic import TypeAdapter

from hypertrain.protocol import envelope_v2
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import Hex64, IslandJobV1, JoinChallenge


def _checked(response: httpx.Response) -> httpx.Response:
    if not response.is_success:
        response.read()
        raise ValueError(f"HTTP {response.status_code} {response.text}")
    return response


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--admission-id", required=True)
    parser.add_argument("--admin-token-file", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args(argv)
    out: Path = args.out
    try:
        if out.is_symlink() or out.exists() and (not out.is_dir() or any(out.iterdir())):
            raise ValueError("--out must not exist or must be an empty directory")
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 2

    token = ""
    try:
        destination = out.resolve()
        run_id = TypeAdapter(Hex64).validate_python(args.run_id)
        admission_id = TypeAdapter(Hex64).validate_python(args.admission_id)
        token = args.admin_token_file.read_text().strip()
        if not re.fullmatch(r"[A-Za-z0-9._~+/-]+=*", token):
            raise ValueError("admin token file must contain one ASCII bearer token")
        url = args.api.rstrip("/") + f"/v2/runs/{run_id}"
        with httpx.Client(timeout=60, follow_redirects=False) as client:
            data = _checked(
                client.post(
                    url + f"/admin/trial/{admission_id}/export",
                    headers={"Authorization": f"Bearer {token}"},
                )
            ).json()
            job = IslandJobV1.model_validate(data["job"])
            challenge = JoinChallenge.model_validate(data["challenge"])
            objects = TypeAdapter(dict[str, Hex64]).validate_python(data["objects"])
            if (
                job.run_id != run_id
                or challenge.manifest_hash != run_id
                or challenge.admission_id != admission_id
                or challenge.nonce != data["nonce"]
                or set(objects) != set(job.object_paths)
            ):
                raise ValueError("export job/challenge/object binding differs")
            for name in ("start_state", "ef_in", "v0"):
                if objects.get(name) != getattr(job, name + "_sha256"):
                    raise ValueError(f"{name} sha256 differs from job")
            envelope = data["challenge_envelope"]
            signed = envelope_v2.parse_envelope(envelope)
            if (
                signed.type != "JoinChallenge"
                or signed.run_id != run_id
                or signed.signer != job.manifest.training.coord_pubkey
                or signed.body != challenge.body()
                or not envelope_v2.verify_envelope(envelope)
            ):
                raise ValueError("signed challenge differs from exported trial")

            parent = destination.parent
            parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(prefix=".trial-export-", dir=parent) as temporary:
                root = Path(temporary).resolve()
                destinations = {root / "job.json", root / "challenge.json"}
                paths: dict[str, Path] = {}
                for name, relative in job.object_paths.items():
                    target = (root / relative).resolve()
                    final = (out / relative).resolve()
                    if (
                        target == root
                        or not target.is_relative_to(root)
                        or not final.is_relative_to(destination)
                        or any(
                            target == p or target in p.parents or p in target.parents
                            for p in destinations
                        )
                    ):
                        raise ValueError(f"unsafe object path: {relative}")
                    destinations.add(target)
                    paths[name] = target
                files: dict[str, str] = {}
                for name, digest in objects.items():
                    target = paths[name]
                    target.parent.mkdir(parents=True, exist_ok=True)
                    hasher = hashlib.sha256()
                    with client.stream("GET", url + "/objects/" + digest) as response:
                        _checked(response)
                        with target.open("xb") as stream:
                            for chunk in response.iter_bytes():
                                hasher.update(chunk)
                                stream.write(chunk)
                    if hasher.hexdigest() != digest:
                        raise ValueError(f"{name} sha256 mismatch")
                    files[target.relative_to(root).as_posix()] = digest
                for name, raw in (
                    ("job.json", canonicalize(job.body())),
                    ("challenge.json", canonicalize(envelope)),
                ):
                    (root / name).write_bytes(raw)
                    files[name] = hashlib.sha256(raw).hexdigest()
                if destination.exists():
                    if destination.is_symlink() or any(destination.iterdir()):
                        raise ValueError("--out is no longer empty")
                    published: list[Path] = []
                    try:
                        for path in root.iterdir():
                            target = path.rename(destination / path.name)
                            published.append(target)
                    except OSError:
                        for path in reversed(published):
                            path.rename(root / path.name)
                        raise
                else:
                    root.rename(destination)
        print(
            json.dumps(
                {
                    "out": str(out),
                    "run_id": run_id,
                    "admission_id": admission_id,
                    "nonce": challenge.nonce,
                    "deadline": job.deadline,
                    "files": files,
                }
            )
        )
    except (httpx.HTTPError, OSError, ValueError, KeyError, TypeError) as error:
        message = str(error)
        print(message.replace(token, "[redacted]") if token else message, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
