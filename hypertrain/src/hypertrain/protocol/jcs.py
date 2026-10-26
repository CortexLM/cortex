"""RFC 8785 JSON Canonicalization Scheme (JCS)."""

from __future__ import annotations

import math
from collections.abc import Mapping, Sequence
from decimal import Decimal
from typing import Any

MAX_SAFE_INT = 2**53 - 1
_ESC = {0x08: "\\b", 0x09: "\\t", 0x0A: "\\n", 0x0C: "\\f", 0x0D: "\\r", 0x22: '\\"', 0x5C: "\\\\"}


class CanonicalizationError(ValueError):
    pass


def _string(s: str) -> str:
    out = ['"']
    for ch in s:
        c = ord(ch)
        if 0xD800 <= c <= 0xDFFF:
            raise CanonicalizationError("lone surrogate in string")
        if c in _ESC:
            out.append(_ESC[c])
        elif c < 0x20:
            out.append(f"\\u{c:04x}")
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def _number(x: float) -> str:
    # ECMA-262 Number::toString; repr() yields the same shortest round-trip digits.
    if not math.isfinite(x):
        raise CanonicalizationError("NaN/Infinity not allowed")
    if x == 0:
        return "0"
    sign = "-" if x < 0 else ""
    dec = Decimal(repr(abs(x))).normalize().as_tuple()
    digits = "".join(map(str, dec.digits))
    k = len(digits)
    n = k + int(dec.exponent)
    if k <= n <= 21:
        body = digits + "0" * (n - k)
    elif 0 < n <= 21:
        body = digits[:n] + "." + digits[n:]
    elif -6 < n <= 0:
        body = "0." + "0" * (-n) + digits
    else:
        e = n - 1
        es = ("+" if e >= 0 else "-") + str(abs(e))
        body = (digits if k == 1 else digits[0] + "." + digits[1:]) + "e" + es
    return sign + body


def _emit(v: Any, out: list[str], allow_float: bool) -> None:
    if v is None:
        out.append("null")
    elif v is True:
        out.append("true")
    elif v is False:
        out.append("false")
    elif isinstance(v, int):
        if abs(v) > MAX_SAFE_INT:
            raise CanonicalizationError(f"integer outside I-JSON safe range: {v}")
        out.append(str(v))
    elif isinstance(v, float):
        if not allow_float:
            raise CanonicalizationError("float in protocol preimage; use f32 hex")
        out.append(_number(v))
    elif isinstance(v, str):
        out.append(_string(v))
    elif isinstance(v, Mapping):
        keys = list(v.keys())
        if not all(isinstance(k, str) for k in keys):
            raise CanonicalizationError("object keys must be strings")
        out.append("{")
        for i, k in enumerate(sorted(keys, key=lambda s: s.encode("utf-16-be", "surrogatepass"))):
            if i:
                out.append(",")
            out.append(_string(k))
            out.append(":")
            _emit(v[k], out, allow_float)
        out.append("}")
    elif isinstance(v, Sequence) and not isinstance(v, bytes | bytearray):
        out.append("[")
        for i, item in enumerate(v):
            if i:
                out.append(",")
            _emit(item, out, allow_float)
        out.append("]")
    else:
        raise CanonicalizationError(f"unsupported JSON type {type(v).__name__}")


def canonicalize(value: Any, *, allow_float: bool = True) -> bytes:
    """UTF-8 JCS bytes. Protocol preimages pass allow_float=False (floats travel as f32 hex)."""
    out: list[str] = []
    _emit(value, out, allow_float)
    return "".join(out).encode("utf-8")
