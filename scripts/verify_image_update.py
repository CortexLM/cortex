"""Validate an operator-approved runtime image update without deploying it."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

DIGEST = re.compile(r"sha256:[0-9a-f]{64}")
COMMIT = re.compile(r"[0-9a-f]{40}")
REPOSITORY = re.compile(r"ghcr\.io/[a-z0-9][a-z0-9._-]*(?:/[a-z0-9][a-z0-9._-]*)+")
RELEASE_TAG = re.compile(
    r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?"
)
MANIFEST_MEDIA_TYPES = {
    "application/vnd.docker.distribution.manifest.v2+json",
    "application/vnd.oci.image.manifest.v1+json",
}


def _load_manifest(path: Path) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError("registry manifest is unavailable") from error
    if not isinstance(value, dict):
        raise ValueError("registry manifest must be a JSON object")
    if value.get("mediaType") not in MANIFEST_MEDIA_TYPES:
        raise ValueError("registry object must be a single-platform image manifest")
    digest = value.get("digest")
    if not isinstance(digest, str) or not DIGEST.fullmatch(digest):
        raise ValueError("registry manifest digest is invalid")
    if "schemaVersion" in value:
        schema_version = value["schemaVersion"]
        if (
            isinstance(schema_version, bool)
            or not isinstance(schema_version, int)
            or schema_version != 2
        ):
            raise ValueError("registry manifest must use schema version 2")
        return value
    if set(value) != {"mediaType", "digest", "size"}:
        raise ValueError("registry descriptor must contain only mediaType, digest and size")
    if value["mediaType"] not in MANIFEST_MEDIA_TYPES:
        raise ValueError("registry descriptor must identify a single-platform image manifest")
    size = value["size"]
    if isinstance(size, bool) or not isinstance(size, int) or size <= 0:
        raise ValueError("registry descriptor size must be a positive integer")
    return value


def _validate_candidate(candidate: str, repository: str, digest: str, source_commit: str) -> None:
    if candidate.startswith(repository + "@"):
        if candidate.removeprefix(repository + "@") != digest:
            raise ValueError("candidate digest differs from the approved digest")
        return
    if not candidate.startswith(repository + ":"):
        raise ValueError("candidate repository differs from the expected GHCR repository")
    tag = candidate.removeprefix(repository + ":")
    if COMMIT.fullmatch(tag) and tag != source_commit:
        raise ValueError("candidate commit tag differs from the source commit")
    if not (COMMIT.fullmatch(tag) or RELEASE_TAG.fullmatch(tag)):
        raise ValueError("candidate tag must be a full commit SHA or semantic release tag")


def verify_update(
    *,
    candidate: str,
    expected_repository: str,
    expected_digest: str,
    source_commit: str,
    approval: str,
    requested_by: str,
    manifest_path: Path,
    output_path: Path,
) -> dict[str, object]:
    if not REPOSITORY.fullmatch(expected_repository):
        raise ValueError("expected repository must be a lowercase GHCR repository")
    if not DIGEST.fullmatch(expected_digest):
        raise ValueError("expected digest must be sha256 followed by 64 lowercase hex characters")
    if not COMMIT.fullmatch(source_commit):
        raise ValueError("source commit must be 40 lowercase hex characters")
    if (
        not requested_by
        or len(requested_by) > 128
        or any(ord(character) < 32 for character in requested_by)
    ):
        raise ValueError("requester identity is invalid")
    if approval != f"APPROVE {expected_digest}":
        raise ValueError("operator approval does not bind the expected digest")

    _validate_candidate(candidate, expected_repository, expected_digest, source_commit)
    manifest = _load_manifest(manifest_path)
    if manifest.get("digest") != expected_digest:
        raise ValueError("registry manifest digest differs from the expected digest")

    result: dict[str, object] = {
        "schema_version": 1,
        "approved": True,
        "candidate": candidate,
        "digest": expected_digest,
        "image": f"{expected_repository}@{expected_digest}",
        "requested_by": requested_by,
        "source_commit": source_commit,
    }
    output_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--expected-digest", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--approval", required=True)
    parser.add_argument("--requested-by", required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        result = verify_update(
            candidate=arguments.candidate,
            expected_repository=arguments.repository,
            expected_digest=arguments.expected_digest,
            source_commit=arguments.source_commit,
            approval=arguments.approval,
            requested_by=arguments.requested_by,
            manifest_path=arguments.manifest,
            output_path=arguments.output,
        )
    except ValueError as error:
        raise SystemExit(f"image update refused: {error}") from None
    print(result["image"])


if __name__ == "__main__":
    main()
