"""Rebuild seed objects on a host from the pinned HF genesis checkpoint instead of uploading them.

A seed object (start_state, ef_in) is a safetensors file whose tensors are either all zero or a
`theta/<name>` copy of the genesis checkpoint's `<name>`. plan() derives that layout from the
controller's local file; the host gets only the header (~100 KB) and a copy list, downloads the
checkpoint with parallel range requests and writes header + zeros + theta. The caller proves the
result with the usual size+sha256 check and falls back to uploading on any mismatch.
"""

import json
import shlex
import struct
from pathlib import Path

REPO_ID = "OpenType/neon-1.2-genesis"
# Resolved 2026-10-15 via https://huggingface.co/api/models/OpenType/neon-1.2-genesis
REVISION = "1aa51eb5561719044f206061ba63a0d8e94ab5b0"
URL = f"https://huggingface.co/{REPO_ID}/resolve/{REVISION}/model.safetensors"
SIZE = 1_415_480_056
SHA256 = "c2b614d2353af47583d4f0604db006e04430a6e84f16066161e748e7fc0b6aa9"
RANGES = 8
CHUNK = 16 << 20


def plan(local: Path) -> tuple[bytes, list[list]] | None:
    """(header bytes, copy list) for `local`, or None when it is not header + zeros + theta."""
    with local.open("rb") as f:
        raw = f.read(8)
        head = raw + f.read(struct.unpack("<Q", raw)[0])
        tensors = json.loads(head[8:])
        tensors.pop("__metadata__", None)
        base = len(head)
        if base + max(t["data_offsets"][1] for t in tensors.values()) != local.stat().st_size:
            return None
        copy = []
        for name, t in sorted(tensors.items(), key=lambda kv: kv[1]["data_offsets"][0]):
            lo, hi = t["data_offsets"]
            f.seek(base + lo)
            left, zero = hi - lo, True
            while left and zero:
                b = f.read(min(left, CHUNK))
                zero = bool(b) and b.count(0) == len(b)
                left -= len(b)
            if zero:
                continue
            if not name.startswith("theta/"):
                return None
            copy.append([base + lo, name.removeprefix("theta/"), hi - lo, t["dtype"], t["shape"]])
    return head, copy


def payload(head: bytes, copy: list[list], out: str, src: str, size: int) -> bytes:
    spec = json.dumps({"out": out, "src": src, "size": size, "copy": copy}).encode()
    return struct.pack("<Q", len(spec)) + spec + head


# Host side: stdin = payload(); checks every copied tensor's dtype/shape/size against the
# downloaded checkpoint header; zeros are a sparse truncate.
BUILD = r"""
import json, os, struct, sys
data = sys.stdin.buffer.read()
n = struct.unpack("<Q", data[:8])[0]
spec, head = json.loads(data[8:8 + n]), data[8 + n:]
part = spec["out"] + ".hf-part"
with open(part, "wb") as f:
    f.write(head)
    f.truncate(spec["size"])
    if spec["copy"]:
        src = open(spec["src"], "rb")
        hl = struct.unpack("<Q", src.read(8))[0]
        hh = json.loads(src.read(hl))
        for dst, name, nbytes, dtype, shape in spec["copy"]:
            t = hh[name]
            lo, hi = t["data_offsets"]
            if (hi - lo, t["dtype"], t["shape"]) != (nbytes, dtype, shape):
                sys.exit("layout mismatch " + name)
            src.seek(8 + hl + lo)
            f.seek(dst)
            while nbytes:
                b = src.read(min(nbytes, 1 << 24))
                if not b:
                    sys.exit("short checkpoint " + name)
                f.write(b)
                nbytes -= len(b)
os.replace(part, spec["out"])
print("HF_BUILT", spec["out"], spec["size"])
"""


def download_cmd(dst: str, ranges: int = RANGES) -> str:
    """bash: fetch the pinned checkpoint to `dst` with `ranges` parallel curls; verify size+sha."""
    q, step = shlex.quote(dst), -(-SIZE // ranges)
    gets = "".join(
        f"curl -fsSL --retry 3 --connect-timeout 20 -r {i * step}-{min(SIZE, (i + 1) * step) - 1}"
        f' -o {q}.r{i} {URL} & p="$p $!"; '
        for i in range(ranges)
    )
    parts = " ".join(f"{q}.r{i}" for i in range(ranges))
    return (
        f'set -e; [ "$(sha256sum {q} 2>/dev/null | cut -c1-64)" = {SHA256} ] && exit 0; p=; '
        f"{gets}for x in $p; do wait $x; done; cat {parts} > {q}.part; rm -f {parts}; "
        f'[ "$(stat -c %s {q}.part)" = {SIZE} ]; '
        f'[ "$(sha256sum {q}.part | cut -c1-64)" = {SHA256} ]; mv {q}.part {q}'
    )
