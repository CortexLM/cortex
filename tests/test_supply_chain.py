"""Supply-chain workflows publish and approve only immutable runtime images."""

import json
import re
import runpy
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
VERIFY_PATH = ROOT / "scripts/verify_image_update.py"
WORKFLOWS = ROOT / ".github/workflows"


def verifier():
    return runpy.run_path(str(VERIFY_PATH))["verify_update"]


def manifest(tmp_path: Path, digest: str) -> Path:
    path = tmp_path / "manifest.json"
    path.write_text(
        json.dumps(
            {
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "digest": digest,
            }
        )
    )
    return path


def descriptor(tmp_path: Path, digest: str, *, size: object = 1234) -> Path:
    path = tmp_path / "descriptor.json"
    path.write_text(
        json.dumps(
            {
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "digest": digest,
                "size": size,
            }
        )
    )
    return path


def workflow_job(source: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [a-zA-Z0-9_-]+:\n|\Z)",
        source,
    )
    assert match is not None
    return match.group("body")


def workflow_job_header(source: str, name: str) -> str:
    job = workflow_job(source, name)
    header, separator, _steps = job.partition("    steps:\n")
    assert separator
    return header


def test_update_approval_binds_repository_digest_commit_and_requester(tmp_path):
    digest = "sha256:" + "a" * 64
    commit = "b" * 40
    output = tmp_path / "update.json"

    result = verifier()(
        candidate=f"ghcr.io/cortexlm/cortex/python:{commit}",
        expected_repository="ghcr.io/cortexlm/cortex/python",
        expected_digest=digest,
        source_commit=commit,
        approval=f"APPROVE {digest}",
        requested_by="operator",
        manifest_path=manifest(tmp_path, digest),
        output_path=output,
    )

    assert result == {
        "schema_version": 1,
        "approved": True,
        "requested_by": "operator",
        "candidate": f"ghcr.io/cortexlm/cortex/python:{commit}",
        "digest": digest,
        "image": f"ghcr.io/cortexlm/cortex/python@{digest}",
        "source_commit": commit,
    }
    assert json.loads(output.read_text()) == result


def test_update_accepts_real_single_platform_oci_descriptor(tmp_path):
    digest = "sha256:" + "a" * 64
    commit = "b" * 40

    result = verifier()(
        candidate=f"ghcr.io/cortexlm/cortex/python:{commit}",
        expected_repository="ghcr.io/cortexlm/cortex/python",
        expected_digest=digest,
        source_commit=commit,
        approval=f"APPROVE {digest}",
        requested_by="operator",
        manifest_path=descriptor(tmp_path, digest),
        output_path=tmp_path / "update.json",
    )

    assert result["digest"] == digest


@pytest.mark.parametrize("size", [0, -1, True, "1234"])
def test_update_refuses_invalid_oci_descriptor_size(tmp_path, size):
    digest = "sha256:" + "a" * 64
    commit = "b" * 40

    with pytest.raises(ValueError, match="descriptor size"):
        verifier()(
            candidate=f"ghcr.io/cortexlm/cortex/python:{commit}",
            expected_repository="ghcr.io/cortexlm/cortex/python",
            expected_digest=digest,
            source_commit=commit,
            approval=f"APPROVE {digest}",
            requested_by="operator",
            manifest_path=descriptor(tmp_path, digest, size=size),
            output_path=tmp_path / "update.json",
        )


def test_update_refuses_descriptor_with_unexpected_fields(tmp_path):
    digest = "sha256:" + "a" * 64
    commit = "b" * 40
    path = descriptor(tmp_path, digest)
    value = json.loads(path.read_text())
    value["platform"] = {"architecture": "amd64", "os": "linux"}
    path.write_text(json.dumps(value))

    with pytest.raises(ValueError, match="contain only"):
        verifier()(
            candidate=f"ghcr.io/cortexlm/cortex/python:{commit}",
            expected_repository="ghcr.io/cortexlm/cortex/python",
            expected_digest=digest,
            source_commit=commit,
            approval=f"APPROVE {digest}",
            requested_by="operator",
            manifest_path=path,
            output_path=tmp_path / "update.json",
        )


def test_update_refuses_non_integer_schema_version(tmp_path):
    digest = "sha256:" + "a" * 64
    commit = "b" * 40
    path = manifest(tmp_path, digest)
    value = json.loads(path.read_text())
    value["schemaVersion"] = 2.0
    path.write_text(json.dumps(value))

    with pytest.raises(ValueError, match="schema version 2"):
        verifier()(
            candidate=f"ghcr.io/cortexlm/cortex/python:{commit}",
            expected_repository="ghcr.io/cortexlm/cortex/python",
            expected_digest=digest,
            source_commit=commit,
            approval=f"APPROVE {digest}",
            requested_by="operator",
            manifest_path=path,
            output_path=tmp_path / "update.json",
        )


def test_update_refuses_multi_platform_index(tmp_path):
    digest = "sha256:" + "a" * 64
    commit = "b" * 40
    path = manifest(tmp_path, digest)
    value = json.loads(path.read_text())
    value["mediaType"] = "application/vnd.oci.image.index.v1+json"
    path.write_text(json.dumps(value))

    with pytest.raises(ValueError, match="single-platform"):
        verifier()(
            candidate=f"ghcr.io/cortexlm/cortex/python:{commit}",
            expected_repository="ghcr.io/cortexlm/cortex/python",
            expected_digest=digest,
            source_commit=commit,
            approval=f"APPROVE {digest}",
            requested_by="operator",
            manifest_path=path,
            output_path=tmp_path / "update.json",
        )


@pytest.mark.parametrize(
    "candidate,digest,approval",
    [
        ("ghcr.io/cortexlm/cortex/python:latest", "sha256:" + "a" * 64, None),
        ("ghcr.io/cortexlm/cortex/python:main", "sha256:" + "a" * 64, None),
        ("ghcr.io/other/cortex/python:v1.2.3", "sha256:" + "a" * 64, None),
        ("ghcr.io/cortexlm/cortex/python:v1.2.3", "sha256:" + "b" * 64, None),
        ("ghcr.io/cortexlm/cortex/python:v1.2.3", "sha256:" + "a" * 64, "APPROVE"),
    ],
)
def test_update_refuses_mutable_wrong_or_unapproved_candidate(
    tmp_path, candidate, digest, approval
):
    resolved = "sha256:" + "a" * 64

    with pytest.raises(ValueError):
        verifier()(
            candidate=candidate,
            expected_repository="ghcr.io/cortexlm/cortex/python",
            expected_digest=digest,
            source_commit="b" * 40,
            approval=approval or f"APPROVE {digest}",
            requested_by="operator",
            manifest_path=manifest(tmp_path, resolved),
            output_path=tmp_path / "update.json",
        )


def test_update_refuses_digest_pin_that_differs_from_registry_manifest(tmp_path):
    resolved = "sha256:" + "a" * 64
    claimed = "sha256:" + "b" * 64

    with pytest.raises(ValueError, match="candidate digest"):
        verifier()(
            candidate=f"ghcr.io/cortexlm/cortex/python@{claimed}",
            expected_repository="ghcr.io/cortexlm/cortex/python",
            expected_digest=resolved,
            source_commit="c" * 40,
            approval=f"APPROVE {resolved}",
            requested_by="operator",
            manifest_path=manifest(tmp_path, resolved),
            output_path=tmp_path / "update.json",
        )


def test_update_refuses_commit_tag_that_differs_from_source_commit(tmp_path):
    digest = "sha256:" + "a" * 64

    with pytest.raises(ValueError, match="source commit"):
        verifier()(
            candidate=f"ghcr.io/cortexlm/cortex/python:{'b' * 40}",
            expected_repository="ghcr.io/cortexlm/cortex/python",
            expected_digest=digest,
            source_commit="c" * 40,
            approval=f"APPROVE {digest}",
            requested_by="operator",
            manifest_path=manifest(tmp_path, digest),
            output_path=tmp_path / "update.json",
        )


def test_all_external_actions_are_pinned_to_commits():
    for workflow in WORKFLOWS.glob("*.yml"):
        for line in workflow.read_text().splitlines():
            match = re.search(r"\buses:\s*([^\s#]+)", line)
            if not match or match.group(1).startswith("./"):
                continue
            reference = match.group(1)
            assert re.fullmatch(r"[^@]+@[0-9a-f]{40}", reference), (
                workflow.name,
                reference,
            )


def test_image_workflow_builds_runtime_automatically_without_deploying():
    source = (WORKFLOWS / "images.yml").read_text()

    assert "branches: [main]" in source
    assert '"v*.*.*"' in source
    assert "workflow_dispatch:" not in source
    assert "target: runtime" in source
    assert "format: cyclonedx" in source
    assert "severity: HIGH,CRITICAL" in source
    assert "push-to-registry: true" in source
    for forbidden in ("git pull", "watchtower", "docker.sock", "docker compose up", "ssh "):
        assert forbidden not in source.lower()


def test_image_workflow_has_no_manual_entrypoint_and_scopes_publication_permissions():
    source = (WORKFLOWS / "images.yml").read_text()
    trigger = source.split("permissions:", 1)[0]
    global_policy = source.split("jobs:", 1)[0]
    job_names = set(re.findall(r"(?m)^  ([a-zA-Z0-9_-]+):\n", source.split("jobs:\n", 1)[1]))
    publication = workflow_job_header(source, "publish")

    assert "publish:" not in trigger
    assert "workflow_dispatch:" not in trigger
    assert job_names == {"publish"}
    assert "permissions:\n  contents: read" in global_policy
    assert "write-all" not in source
    for permission in ("packages", "attestations", "id-token"):
        grant = f"{permission}: write"
        assert source.count(grant) == 1
        assert grant in publication


def test_image_workflow_allows_the_full_ci_timeout_before_refusing_publication():
    source = (WORKFLOWS / "images.yml").read_text()
    attempts = re.search(r"for attempt in \$\(seq 1 ([0-9]+)\)", source)

    assert attempts is not None
    assert int(attempts.group(1)) * 15 >= 30 * 60


def test_release_tag_aliases_the_existing_sha_digest():
    source = (WORKFLOWS / "images.yml").read_text()

    assert "steps.policy.outputs.release_tag == ''" in source
    assert "docker buildx imagetools create --prefer-index=false" in source
    assert 'docker tag cortex-python:test "$release_ref"' not in source


def test_immutable_sha_tag_is_created_only_after_provenance_attestation():
    source = (WORKFLOWS / "images.yml").read_text()

    candidate = source.index("Publish the exact tested runtime candidate")
    attestation = source.index("Attest build provenance")
    immutable = source.index("Publish the immutable SHA tag")
    assert candidate < attestation < immutable
    assert 'docker push "$sha_ref"' not in source


def test_publication_cannot_override_the_source_pinned_python_image():
    source = (WORKFLOWS / "images.yml").read_text()

    assert 'test "$python_image" = "$DEFAULT_PYTHON_IMAGE"' in source


def test_workflow_summaries_do_not_use_shell_backticks_or_unicode_escapes():
    for name in ("images.yml", "approve-image-update.yml"):
        source = (WORKFLOWS / name).read_text()

        assert "\\u0060" not in source
        assert 'run: echo "' not in source


def test_update_workflow_only_verifies_an_operator_approved_digest():
    source = (WORKFLOWS / "approve-image-update.yml").read_text()
    trigger = source.split("permissions:", 1)[0]

    assert "workflow_dispatch:" in trigger
    assert "push:" not in trigger
    assert "environment: production" in source
    assert "gh attestation verify" in source
    assert "verify_image_update.py" in source
    assert "APPROVE sha256:" in source
    assert '--requested-by "$GITHUB_ACTOR"' in source
    assert "approved_by" not in source
    for forbidden in ("git pull", "watchtower", "docker.sock", "docker compose up", "ssh "):
        assert forbidden not in source.lower()


def test_update_workflow_rescans_the_digest_before_recording_approval():
    source = (WORKFLOWS / "approve-image-update.yml").read_text()
    scan_start = source.index("Re-scan immutable digest for current vulnerabilities")
    approval_start = source.index("Bind operator approval to the resolved digest")
    scan = source[scan_start:approval_start]

    assert scan_start < approval_start
    assert "aquasecurity/trivy-action@" in scan
    assert "image-ref: ${{ steps.scope.outputs.repository }}@${{ inputs.expected_digest }}" in scan
    assert "format: json" in scan
    assert "output: trivy-results.json" in scan
    assert 'exit-code: "1"' in scan
    assert "ignore-unfixed: true" in scan
    assert "severity: HIGH,CRITICAL" in scan
    assert "cache: false" in scan
    assert "trivy-results.json" in source[source.index("Upload approved update evidence") :]
