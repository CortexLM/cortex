"""The single signer (data.store.sigv4_headers) vs the official AWS vectors."""

import hashlib

from hypertrain.data.store import S3Credentials, sigv4_headers

EMPTY = hashlib.sha256(b"").hexdigest()


def test_aws_suite_get_vanilla() -> None:
    out = sigv4_headers(
        "GET",
        "https://example.amazonaws.com/",
        S3Credentials("AKIDEXAMPLE", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"),
        "us-east-1",
        "20150830T123600Z",
        EMPTY,
        "service",
        content_sha_header=False,
    )
    assert out["Authorization"] == (
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, "
        "SignedHeaders=host;x-amz-date, "
        "Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    )


def test_s3_docs_get_object_range() -> None:
    out = sigv4_headers(
        "GET",
        "https://examplebucket.s3.amazonaws.com/test.txt",
        S3Credentials("AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"),
        "us-east-1",
        "20130524T000000Z",
        EMPTY,
        extra={"range": "bytes=0-9"},
    )
    assert out["Authorization"] == (
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, "
        "SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, "
        "Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    )
