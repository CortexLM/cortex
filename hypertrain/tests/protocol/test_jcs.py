import json
import struct

import pytest

from hypertrain.protocol.jcs import CanonicalizationError, canonicalize

RFC_322_INPUT = r"""{
  "numbers": [333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
  "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
  "literals": [null, true, false]
}"""
RFC_324_HEX = (
    "7b 22 6c 69 74 65 72 61 6c 73 22 3a 5b 6e 75 6c 6c 2c 74 72"
    " 75 65 2c 66 61 6c 73 65 5d 2c 22 6e 75 6d 62 65 72 73 22 3a"
    " 5b 33 33 33 33 33 33 33 33 33 2e 33 33 33 33 33 33 33 2c 31"
    " 65 2b 33 30 2c 34 2e 35 2c 30 2e 30 30 32 2c 31 65 2d 32 37"
    " 5d 2c 22 73 74 72 69 6e 67 22 3a 22 e2 82 ac 24 5c 75 30 30"
    " 30 66 5c 6e 41 27 42 5c 22 5c 5c 5c 5c 5c 22 2f 22 7d"
)
APPENDIX_B = [
    ("0000000000000000", "0"),
    ("8000000000000000", "0"),
    ("0000000000000001", "5e-324"),
    ("8000000000000001", "-5e-324"),
    ("7fefffffffffffff", "1.7976931348623157e+308"),
    ("ffefffffffffffff", "-1.7976931348623157e+308"),
    ("4340000000000000", "9007199254740992"),
    ("c340000000000000", "-9007199254740992"),
    ("4430000000000000", "295147905179352830000"),
    ("44b52d02c7e14af5", "9.999999999999997e+22"),
    ("44b52d02c7e14af6", "1e+23"),
    ("44b52d02c7e14af7", "1.0000000000000001e+23"),
    ("444b1ae4d6e2ef4e", "999999999999999700000"),
    ("444b1ae4d6e2ef4f", "999999999999999900000"),
    ("444b1ae4d6e2ef50", "1e+21"),
    ("3eb0c6f7a0b5ed8c", "9.999999999999997e-7"),
    ("3eb0c6f7a0b5ed8d", "0.000001"),
    ("41b3de4355555553", "333333333.3333332"),
    ("41b3de4355555554", "333333333.33333325"),
    ("41b3de4355555555", "333333333.3333333"),
    ("41b3de4355555556", "333333333.3333334"),
    ("41b3de4355555557", "333333333.33333343"),
    ("becbf647612f3696", "-0.0000033333333333333333"),
    ("43143ff3c1cb0959", "1424953923781206.2"),
]


def test_rfc_section_3_2_4_bytes() -> None:
    assert canonicalize(json.loads(RFC_322_INPUT)) == bytes.fromhex(RFC_324_HEX.replace(" ", ""))


@pytest.mark.parametrize(("ieee", "expected"), APPENDIX_B)
def test_rfc_appendix_b_numbers(ieee: str, expected: str) -> None:
    assert canonicalize(struct.unpack(">d", bytes.fromhex(ieee))[0]) == expected.encode()


def test_rfc_section_3_2_3_utf16_sort_order() -> None:
    data = json.loads(
        r'{"\u20ac":"Euro Sign","\r":"Carriage Return","\ufb33":"Hebrew Letter Dalet With Dagesh",'
        r'"1":"One","\ud83d\ude00":"Emoji: Grinning Face","\u0080":"Control",'
        r'"\u00f6":"Latin Small Letter O With Diaeresis"}'
    )
    values = list(json.loads(canonicalize(data)).values())
    assert values == [
        "Carriage Return",
        "One",
        "Control",
        "Latin Small Letter O With Diaeresis",
        "Euro Sign",
        "Emoji: Grinning Face",
        "Hebrew Letter Dalet With Dagesh",
    ]


def test_insertion_order_irrelevant() -> None:
    assert canonicalize({"b": [1, {"y": 2, "x": 1}], "a": None}) == canonicalize(
        {"a": None, "b": [1, {"x": 1, "y": 2}]}
    )


@pytest.mark.parametrize(
    "bad", [float("nan"), float("inf"), "\ud800", 2**53, {1: "x"}, b"raw", {1.5}]
)
def test_rejects_non_canonicalizable(bad: object) -> None:
    with pytest.raises(CanonicalizationError):
        canonicalize(bad)


def test_floats_refused_in_protocol_preimages() -> None:
    with pytest.raises(CanonicalizationError):
        canonicalize({"loss": 0.5}, allow_float=False)
    assert canonicalize({"loss": "0000003f"}, allow_float=False) == b'{"loss":"0000003f"}'
