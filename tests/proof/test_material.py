"""Private harvest packs bind to the complete owner-signed topic before rent."""

import json
import os

import pytest

from cortex.errors import ServiceError
from cortex.proof.material import MAX_MATERIAL_BYTES, HarvestMaterial, PrivateFileMaterialSource
from cortex.proof.models import Baseline, Metric, Rule, Topic, digest
from cortex.proof.service import sign_topic
from cortex.protocol.crypto import public_key
from cortex.vm.setup import SetupManifest, export_setup

OWNER = bytes([62]) * 32
PRIVATE_DATASET = "private-holdout-never-public"


@pytest.fixture
def material_fixture(tmp_path):
    import hashlib

    pack = tmp_path / "pack"
    pack.mkdir(mode=0o700)
    script = b"print('synthetic operator adaptor')\n"
    holdout = b"synthetic private evaluation record\n"
    (pack / "run.py").write_bytes(script)
    (pack / "inspect.py").write_bytes(b"print('synthetic inspector')\n")
    (pack / "holdout.txt").write_bytes(holdout)
    export = export_setup(
        pack,
        SetupManifest(
            content_hashes=[hashlib.sha256(holdout).hexdigest()],
            dataset_ids=[PRIVATE_DATASET],
            flops_budget=1000,
            wall_budget_s=30,
        ),
    )
    _, evidence = export.verified()
    metrics = {"holdout_nll": 1.0}
    topic = sign_topic(
        Topic(
            id="material-fixture",
            statement="Compare a synthetic measured fixture",
            status="open",
            metric=Metric(family="nll", primary="holdout_nll", direction="min", epsilon=0.02),
            flops_budget=1000,
            wall_budget_s=30,
            params={"experiment_pack_digest": "sha256:" + evidence.environment_digest},
            checklist=[Rule(id="integrity", text="Verify fixture integrity")],
            baseline=Baseline(
                script_sha256=evidence.script_sha256,
                metrics=metrics,
                metrics_commitment=digest(metrics),
                evidence_digest="cd" * 32,
                flops_budget=1000,
                wall_budget_s=30,
            ),
            holdout_commitment=evidence.private_holdout_digest,
            eval_image_digest="sha256:" + "ab" * 32,
            inference_offer_commitment="bc" * 32,
        ),
        OWNER,
    )
    root = tmp_path / "materials"
    root.mkdir(mode=0o700)
    path = root / f"{topic.content_digest()}.json"
    path.touch(mode=0o600)
    path.write_text(export.model_dump_json())
    return topic, export, evidence, root, path


def test_private_material_load_verifies_pack_topic_and_public_commitments(material_fixture):
    topic, export, evidence, root, _ = material_fixture

    material = PrivateFileMaterialSource(root, topic_public_key=public_key(OWNER)).load(topic)
    raw, verified = material.verified(topic, public_key(OWNER))

    assert (raw, verified) == export.verified()
    assert material.topic_digest == topic.content_digest()
    assert material.environment_digest == evidence.environment_digest
    assert material.private_holdout_digest == topic.holdout_commitment
    assert material.script_sha256 == topic.baseline.script_sha256
    assert material.flops_budget == topic.flops_budget
    assert material.wall_budget_s == topic.wall_budget_s


def test_material_repr_and_serialization_never_include_private_export(material_fixture):
    topic, export, _, root, _ = material_fixture
    material = PrivateFileMaterialSource(root, topic_public_key=public_key(OWNER)).load(topic)

    for serialized in (repr(material), material.model_dump_json(), str(material.model_dump())):
        assert PRIVATE_DATASET not in serialized
        assert export.pack_b64 not in serialized
        assert "content_hashes" not in serialized
        assert "setup_export" not in serialized


@pytest.mark.parametrize("change", ["signature", "owner", "topic_body"])
def test_material_requires_the_trusted_owner_signature(material_fixture, change):
    topic, export, _, _, _ = material_fixture
    owner = public_key(OWNER)
    if change == "signature":
        topic = topic.model_copy(update={"signature": "0" * 128})
    elif change == "owner":
        owner = public_key(bytes([63]) * 32)
    else:
        topic = topic.model_copy(update={"statement": "changed after signing"})

    with pytest.raises(ServiceError) as failure:
        HarvestMaterial.from_export(export, topic, owner)

    assert failure.value.status == 503


@pytest.mark.parametrize(
    "change",
    [
        "closed",
        "custom",
        "baseline",
        "holdout",
        "pack",
        "unprefixed_pack",
        "script",
        "flops",
        "wall",
    ],
)
def test_material_rejects_signed_but_incompatible_topic(material_fixture, change):
    topic, export, _, _, _ = material_fixture
    body = topic.model_dump()
    if change == "closed":
        body["status"] = "closed"
    elif change == "custom":
        body["metric"]["family"] = "custom"
        body["metric"]["custom_id"] = "custom-fixture"
    elif change == "baseline":
        body["baseline"] = None
    elif change == "holdout":
        body["holdout_commitment"] = "aa" * 32
    elif change in {"pack", "unprefixed_pack"}:
        body["params"]["experiment_pack_digest"] = (
            "sha256:" + "aa" * 32
            if change == "pack"
            else topic.params["experiment_pack_digest"].removeprefix("sha256:")
        )
    elif change == "script":
        body["baseline"]["script_sha256"] = "aa" * 32
    else:
        key = "flops_budget" if change == "flops" else "wall_budget_s"
        body[key] += 1
        body["baseline"][key] += 1
    # model_construct exercises revalidation of typed-but-unchecked caller objects.
    body["eval_executor"] = topic.eval_executor
    altered = Topic.model_construct(**body)
    if body["baseline"] is not None:
        altered = altered.model_copy(update={"baseline": Baseline(**body["baseline"])})
    altered = altered.model_copy(
        update={
            "metric": Metric(**body["metric"]),
            "checklist": [Rule(**rule) for rule in body["checklist"]],
        }
    )
    altered = sign_topic(altered, OWNER)

    with pytest.raises(ServiceError):
        HarvestMaterial.from_export(export, altered, public_key(OWNER))


def test_material_verification_detects_mutation_after_initial_validation(material_fixture):
    topic, export, _, _, _ = material_fixture
    material = HarvestMaterial.from_export(export, topic, public_key(OWNER))
    material.setup_export.manifest.dataset_ids.append("changed-private-dataset")

    with pytest.raises(ServiceError) as failure:
        material.verified(topic, public_key(OWNER))

    assert "changed-private-dataset" not in failure.value.reason


def test_material_owns_independent_snapshot_of_private_export(material_fixture):
    topic, export, evidence, _, _ = material_fixture
    material = HarvestMaterial.from_export(export, topic, public_key(OWNER))
    export.manifest.dataset_ids.append("changed-caller-dataset")

    _, verified = material.verified(topic, public_key(OWNER))

    assert verified.private_holdout_digest == evidence.private_holdout_digest


@pytest.mark.parametrize(
    "field",
    [
        "schema_version",
        "topic_digest",
        "environment_digest",
        "private_holdout_digest",
        "script_sha256",
        "flops_budget",
        "wall_budget_s",
    ],
)
def test_material_verification_rejects_changed_public_bindings(material_fixture, field):
    topic, export, _, _, _ = material_fixture
    material = HarvestMaterial.from_export(export, topic, public_key(OWNER))
    previous = getattr(material, field)
    altered = material.model_copy(
        update={field: previous + 1 if isinstance(previous, int) else "ef" * 32}
    )

    with pytest.raises(ServiceError):
        altered.verified(topic, public_key(OWNER))


@pytest.mark.parametrize("attack", ["symlink", "hardlink", "mode", "fifo", "malformed"])
def test_file_material_rejects_unsafe_sources_without_leaking_content(material_fixture, attack):
    topic, _, _, root, path = material_fixture
    content = path.read_bytes()
    path.unlink()
    outside = root.parent / "private-material.json"
    outside.touch(mode=0o600)
    outside.write_bytes(content)
    if attack == "symlink":
        path.symlink_to(outside)
    elif attack == "hardlink":
        os.link(outside, path)
    elif attack == "fifo":
        os.mkfifo(path, mode=0o600)
    else:
        path.touch(mode=0o600)
        path.write_bytes(content if attack == "mode" else PRIVATE_DATASET.encode())
        if attack == "mode":
            path.chmod(0o644)

    with pytest.raises(ServiceError) as failure:
        PrivateFileMaterialSource(root, topic_public_key=public_key(OWNER)).load(topic)

    assert failure.value.status == 503
    assert PRIVATE_DATASET not in failure.value.reason
    assert str(path) not in failure.value.reason


def test_source_rechecks_private_directory_on_each_load(material_fixture):
    topic, _, _, root, _ = material_fixture
    source = PrivateFileMaterialSource(root, topic_public_key=public_key(OWNER))
    root.chmod(0o770)

    with pytest.raises(ServiceError):
        source.load(topic)


def test_source_rejects_directory_symlink(material_fixture):
    topic, _, _, root, _ = material_fixture
    link = root.parent / "linked-materials"
    link.symlink_to(root, target_is_directory=True)

    with pytest.raises(ServiceError):
        PrivateFileMaterialSource(link, topic_public_key=public_key(OWNER)).load(topic)


def test_source_rejects_oversized_file_before_loading_content(material_fixture):
    topic, _, _, root, path = material_fixture
    with path.open("r+b") as source:
        source.truncate(MAX_MATERIAL_BYTES + 1)

    with pytest.raises(ServiceError):
        PrivateFileMaterialSource(root, topic_public_key=public_key(OWNER)).load(topic)


def test_source_rejects_mutated_pack_even_when_json_remains_valid(material_fixture):
    topic, _, _, root, path = material_fixture
    data = json.loads(path.read_text())
    data["pack_b64"] = "Y2hhbmdlZA=="
    path.write_text(json.dumps(data))

    with pytest.raises(ServiceError):
        PrivateFileMaterialSource(root, topic_public_key=public_key(OWNER)).load(topic)
