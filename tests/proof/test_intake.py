import asyncio
import hashlib
import json

import httpx
import pytest

from cortex.errors import ServiceError
from cortex.proof.service import Readiness, UnwiredBackend, sign_submission, sign_topic
from cortex.proof.store import ProofStore

from .conftest import MINER, OWNER, submission


async def post(client, signed, artifact, path="/v1/submissions"):
    body = signed.model_dump()
    body["env"] = signed.env
    return await client.post(
        path,
        data={"json": json.dumps(body)},
        files={"artifact": ("artifact.tar", artifact, "application/x-tar")},
    )


def lookup_body(signed):
    return signed.model_dump(exclude={"artifact_uri", "env"})


async def test_signed_upload_scores_and_survives_restart(setup, tmp_path):
    service, backend, topic, app = setup
    signed, data = submission()
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        response = await post(client, signed, data)
    assert response.status_code == 201, response.text
    assert response.json()["status"] == "accepted"
    assert response.json()["metrics"] == {"quality": 0.8}
    payload_digest = hashlib.sha256(signed.signing_payload()).hexdigest()
    with ProofStore(tmp_path / "proof.sqlite3") as restarted:
        assert restarted.submission(response.json()["id"])["status"] == "accepted"
        assert (
            restarted.lookup_submission(signed.miner_hotkey, signed.submit_nonce, payload_digest)[
                "status"
            ]
            == "accepted"
        )
    assert service.scores(4) == {signed.miner_hotkey: 1_000_000}


async def test_replay_cannot_trigger_a_second_evaluation(setup):
    service, backend, _, _ = setup
    signed, data = submission()
    await service.submit(signed, data)
    with pytest.raises(ServiceError, match="submit_nonce reused") as caught:
        await service.submit(signed, data)
    assert caught.value.status == 401
    assert len(backend.jobs) == 1


@pytest.mark.parametrize(
    "uri",
    [
        "https://[broken",
        "https://artifact.invalid:invalid/candidate.tar",
        "https://artifact.invalid:65536/candidate.tar",
        "https://artifact.invalid:0/candidate.tar",
        "https://:fixture-uri-secret@artifact.invalid/candidate.tar",
        "https://fixture-user@artifact.invalid/candidate.tar",
        "https://@artifact.invalid/candidate.tar",
        "https://artifact.invalid/candidate.tar#part",
        "http://artifact.invalid/candidate.tar",
        "https:///candidate.tar",
    ],
)
async def test_invalid_uri_is_400_without_reserving_nonce_or_starting_evaluation(
    setup, tmp_path, uri
):
    service, backend, _, app = setup
    signed, _ = submission()
    body = {**signed.model_dump(), "artifact_uri": uri}
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app, raise_app_exceptions=False), base_url="http://test"
    ) as client:
        rejected = await client.post("/v1/submissions", json=body)

        assert rejected.status_code == 400, rejected.text
        assert "fixture-uri-secret" not in rejected.text
        assert backend.jobs == []
        with service.store.transaction() as connection:
            assert connection.execute("SELECT count(*) FROM proof_nonces").fetchone()[0] == 0
            assert connection.execute("SELECT count(*) FROM proof_jobs").fetchone()[0] == 0
        for path in tmp_path.glob("proof.sqlite3*"):
            assert b"fixture-uri-secret" not in path.read_bytes()

        body["artifact_uri"] = "https://artifact.invalid:8443/candidate.tar?download=1"
        corrected = await client.post("/v1/submissions", json=body)

    assert corrected.status_code == 201, corrected.text
    assert corrected.json()["status"] == "accepted"
    assert len(backend.jobs) == 1


async def test_uri_transport_is_validated_before_hotkey_signature(setup):
    _, backend, _, app = setup
    signed, _ = submission()
    body = {
        **signed.model_dump(),
        "artifact_uri": "https://[broken",
        "hotkey_signature": "00" * 64,
    }
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app, raise_app_exceptions=False), base_url="http://test"
    ) as client:
        malformed_transport = await client.post("/v1/submissions", json=body)
        body["artifact_uri"] = "https://artifact.invalid/candidate.tar"
        invalid_signature = await client.post("/v1/submissions", json=body)

    assert malformed_transport.status_code == 400
    assert invalid_signature.status_code == 401
    assert backend.jobs == []


@pytest.mark.parametrize("contaminated", [False, True])
async def test_uploaded_bytes_discard_ignored_uri_before_any_durable_result(
    setup, tmp_path, contaminated
):
    service, _, _, app = setup
    signed, artifact = submission(
        manifest={"train_content_hashes": ["private-fixture-content"]} if contaminated else None
    )
    signed = signed.model_copy(
        update={"artifact_uri": "https://:fixture-uri-secret@artifact.invalid/candidate.tar"}
    )
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        response = await post(client, signed, artifact)

    assert response.status_code == 201, response.text
    assert response.json()["status"] == ("rejected" if contaminated else "accepted")
    stored = service.store.submission(response.json()["id"])
    assert stored["body"]["artifact_uri"] == (
        None if contaminated else "proof-artefact://" + signed.artifact_digest
    )
    for path in tmp_path.glob("proof.sqlite3*"):
        assert b"fixture-uri-secret" not in path.read_bytes()


async def test_bad_env_does_not_consume_nonce_and_secrets_never_enter_store(setup, tmp_path):
    service, backend, topic, _ = setup
    updated = sign_topic(
        topic.model_copy(update={"revision": 2, "params": {"miner_byok": "MINER_API_KEY"}}), OWNER
    )
    await service.publish(updated)
    signed, data = submission(env={"UNDECLARED": "sensitive-fixture-value"})
    with pytest.raises(ServiceError, match="undeclared env"):
        await service.submit(signed, data)
    signed = signed.model_copy(update={"env": {"MINER_API_KEY": "sensitive-fixture-value"}})
    row = await service.submit(signed, data)
    assert row["status"] == "accepted"
    assert "sensitive-fixture-value" not in json.dumps(row)
    assert list(service.vault.root.iterdir()) == []
    for path in tmp_path.glob("proof.sqlite3*"):
        assert b"sensitive-fixture-value" not in path.read_bytes()


async def test_enqueue_failure_after_credential_write_leaves_no_job_or_credentials(
    setup, monkeypatch
):
    service, _, topic, _ = setup
    await service.publish(
        sign_topic(
            topic.model_copy(update={"revision": 2, "params": {"miner_byok": "MINER_API_KEY"}}),
            OWNER,
        )
    )
    signed, data = submission(env={"MINER_API_KEY": "private-test-value"})
    original_enqueue = service.store.enqueue

    def fail_after_credentials(job_id, *args, **kwargs):
        # The credential directory must already exist when the row is written:
        # a crash between the two must never leave a queued job without it.
        assert (service.vault.root / job_id).is_dir()
        original_enqueue(job_id, *args, **kwargs)
        raise ServiceError(503, "injected enqueue failure")

    monkeypatch.setattr(service.store, "enqueue", fail_after_credentials)

    with pytest.raises(ServiceError, match="injected enqueue failure"):
        await service.submit(signed, data)

    with service.store.transaction() as connection:
        assert connection.execute("SELECT count(*) FROM proof_jobs").fetchone()[0] == 0
        assert connection.execute("SELECT count(*) FROM proof_nonces").fetchone()[0] == 1
    recovered = service.store.lookup_submission(
        signed.miner_hotkey,
        signed.submit_nonce,
        hashlib.sha256(signed.signing_payload()).hexdigest(),
    )
    assert recovered["status"] == "failed"
    assert list(service.vault.root.iterdir()) == []


async def test_resume_reconciles_orphans_and_preserves_pending_credentials(setup):
    service, _, topic, _ = setup
    await service.publish(
        sign_topic(
            topic.model_copy(
                update={
                    "revision": 2,
                    "params": {"miner_byok": "MINER_API_KEY", "defer_scoring": "true"},
                }
            ),
            OWNER,
        )
    )
    signed, data = submission(env={"MINER_API_KEY": "active-test-value"})
    row = await service.submit(signed, data)
    setup_job, orphan = "cd" * 32, "ab" * 32
    with service.store.transaction() as connection:
        connection.execute(
            "CREATE TABLE IF NOT EXISTS proof_setup_jobs ("
            "id TEXT PRIMARY KEY,policy TEXT NOT NULL,env_names TEXT NOT NULL,"
            "state TEXT NOT NULL,error TEXT)"
        )
        connection.execute(
            "INSERT INTO proof_setup_jobs VALUES (?,?,?,'pending',NULL)",
            (setup_job, "{}", json.dumps(["SETUP_API_KEY"])),
        )
    service.vault.put(setup_job, {"SETUP_API_KEY": "setup-test-value"})
    service.vault.put(orphan, {"OLD_API_KEY": "orphan-test-value"})

    await service.resume()

    assert service.vault.get(row["id"], ["MINER_API_KEY"]) == {"MINER_API_KEY": "active-test-value"}
    assert service.vault.get(setup_job, ["SETUP_API_KEY"]) == {"SETUP_API_KEY": "setup-test-value"}
    assert not (service.vault.root / orphan).exists()


async def test_resume_fails_closed_when_pending_credential_is_missing(setup):
    service, backend, topic, _ = setup
    await service.publish(
        sign_topic(
            topic.model_copy(
                update={
                    "revision": 2,
                    "params": {"miner_byok": "MINER_API_KEY", "defer_scoring": "true"},
                }
            ),
            OWNER,
        )
    )
    signed, data = submission(env={"MINER_API_KEY": "active-test-value"})
    row = await service.submit(signed, data)
    service.vault.delete(row["id"])

    with pytest.raises(ServiceError, match="credential.*missing"):
        await service.resume()

    assert backend.jobs == []


@pytest.mark.parametrize(
    "change",
    [
        {"hotkey_signature": "00" * 64},
        {"claim": "tampered claim"},
        {"declared_flops": 23},
        {"artifact_digest": "aa" * 32},
    ],
)
async def test_signature_binds_every_scoring_input_before_spend(setup, change):
    service, backend, _, _ = setup
    signed, data = submission()
    with pytest.raises(ServiceError) as caught:
        await service.submit(signed.model_copy(update=change), data)
    assert caught.value.status == 401
    assert service.store.submissions(4) == []
    assert backend.jobs == []


async def test_unwired_executor_returns_503_without_submission(setup):
    service, _, _, _ = setup
    service.backend = UnwiredBackend()
    signed, data = submission()
    with pytest.raises(ServiceError, match="UnwiredVmOrchestrator") as caught:
        await service.submit(signed, data)
    assert caught.value.status == 503
    assert service.store.submissions(4) == []


async def test_contamination_is_a_durable_reject_without_paid_execution(setup):
    service, backend, _, _ = setup
    signed, data = submission(manifest={"train_content_hashes": ["private-fixture-content"]})
    row = await service.submit(signed, data)
    assert row["status"] == "rejected"
    assert service.store.submission(row["id"])["status"] == "rejected"
    assert backend.jobs == []
    assert service.scores(4) == {}


async def test_report_for_another_artifact_cannot_create_a_scored_row(setup):
    service, backend, _, _ = setup
    backend.misbind = True
    signed, data = submission()
    with pytest.raises(ServiceError, match="evidence binding mismatch"):
        await service.submit(signed, data)
    assert service.store.submissions(4) == []
    assert list(service.vault.root.iterdir()) == []


async def test_infrastructure_exception_deletes_secret_before_failed_state(setup):
    service, backend, topic, _ = setup
    await service.publish(
        sign_topic(
            topic.model_copy(update={"revision": 2, "params": {"miner_byok": "MINER_API_KEY"}}),
            OWNER,
        )
    )

    async def fail_evaluation(**kwargs):
        raise RuntimeError("private backend detail")

    backend.evaluate = fail_evaluation
    signed, data = submission(env={"MINER_API_KEY": "active-test-value"})

    with pytest.raises(ServiceError, match="evaluation infrastructure failed"):
        await service.submit(signed, data)

    with service.store.transaction() as connection:
        row = connection.execute("SELECT state,error FROM proof_jobs").fetchone()
    assert tuple(row) == ("failed", "evaluation infrastructure failed")
    assert list(service.vault.root.iterdir()) == []


async def test_cleanup_failure_keeps_submission_nonterminal_and_secret_present(setup):
    service, backend, topic, _ = setup
    await service.publish(
        sign_topic(
            topic.model_copy(update={"revision": 2, "params": {"miner_byok": "MINER_API_KEY"}}),
            OWNER,
        )
    )
    backend.release.clear()
    signed, data = submission(env={"MINER_API_KEY": "active-test-value"})
    waiter = asyncio.create_task(service.submit(signed, data))
    await backend.entered.wait()
    job_directory = next(service.vault.root.iterdir())
    credential = job_directory / "MINER_API_KEY"
    credential.chmod(0o640)
    backend.release.set()

    with pytest.raises(ServiceError, match="not private"):
        await waiter

    with service.store.transaction() as connection:
        row = connection.execute(
            "SELECT state FROM proof_jobs WHERE id=?", (job_directory.name,)
        ).fetchone()
    assert row["state"] == "running"
    assert service.store.submission(job_directory.name) is None
    assert credential.read_text() == "active-test-value"

    credential.chmod(0o600)
    service.vault.delete(job_directory.name)


async def test_stale_worker_cannot_delete_credentials_owned_by_replacement(setup):
    service, backend, topic, _ = setup
    await service.publish(
        sign_topic(
            topic.model_copy(update={"revision": 2, "params": {"miner_byok": "MINER_API_KEY"}}),
            OWNER,
        )
    )
    backend.release.clear()
    signed, data = submission(env={"MINER_API_KEY": "active-test-value"})
    waiter = asyncio.create_task(service.submit(signed, data))
    await backend.entered.wait()
    job_directory = next(service.vault.root.iterdir())
    with service.store.transaction() as connection:
        connection.execute(
            "UPDATE proof_jobs SET owner='replacement-worker',lease_until=9999999999 WHERE id=?",
            (job_directory.name,),
        )
    backend.release.set()

    with pytest.raises(ServiceError, match="lease lost"):
        await waiter

    assert (job_directory / "MINER_API_KEY").read_text() == "active-test-value"
    with service.store.transaction() as connection:
        row = connection.execute(
            "SELECT state,owner FROM proof_jobs WHERE id=?", (job_directory.name,)
        ).fetchone()
    assert tuple(row) == ("running", "replacement-worker")

    service.vault.delete(job_directory.name)


async def test_client_cancellation_does_not_cancel_accepted_evaluation(setup):
    service, backend, _, _ = setup
    backend.release.clear()
    signed, data = submission()
    waiter = asyncio.create_task(service.submit(signed, data))
    await backend.entered.wait()
    waiter.cancel()
    with pytest.raises(asyncio.CancelledError):
        await waiter
    backend.release.set()
    await service.drain()
    assert service.store.submissions(4)[0]["status"] == "accepted"


async def test_worker_cancellation_keeps_secret_for_restart(setup):
    service, backend, topic, _ = setup
    await service.publish(
        sign_topic(
            topic.model_copy(update={"revision": 2, "params": {"miner_byok": "MINER_API_KEY"}}),
            OWNER,
        )
    )
    backend.release.clear()
    signed, data = submission(env={"MINER_API_KEY": "active-test-value"})
    waiter = asyncio.create_task(service.submit(signed, data))
    await backend.entered.wait()
    worker = next(iter(service._tasks.values()))

    worker.cancel()
    with pytest.raises(asyncio.CancelledError):
        await waiter

    job_directory = next(service.vault.root.iterdir())
    assert (job_directory / "MINER_API_KEY").read_text() == "active-test-value"
    with service.store.transaction() as connection:
        row = connection.execute(
            "SELECT state FROM proof_jobs WHERE id=?", (job_directory.name,)
        ).fetchone()
    assert row["state"] == "running"

    backend.release.set()
    service.vault.delete(job_directory.name)


async def test_finished_worker_cannot_drop_a_replacement_task(setup):
    service, _, _, _ = setup
    old = asyncio.create_task(asyncio.sleep(0))
    await old
    release = asyncio.Event()
    replacement = asyncio.create_task(release.wait())
    service._tasks["same-job"] = replacement

    service._finished("same-job", old)

    assert service._tasks["same-job"] is replacement
    replacement.cancel()
    await asyncio.gather(replacement, return_exceptions=True)
    service._tasks.pop("same-job")


async def test_topic_endpoint_uses_the_same_signature_and_replay_gates(setup):
    _, backend, _, app = setup
    signed, data = submission()
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        first = await post(client, signed, data, "/v1/proof/topics/fixture-topic/submit")
        second = await post(client, signed, data)
    assert first.status_code == 201
    assert second.status_code == 401
    assert len(backend.jobs) == 1


async def test_topics_never_expose_private_holdout_records(setup):
    _, _, _, app = setup
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        response = await client.get("/v1/proof/topics")
    assert response.status_code == 200
    assert "private-fixture" not in response.text


async def test_invalid_body_and_operator_auth_do_not_echo_secrets(setup):
    _, _, topic, app = setup
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        body = {"env": {"MINER_API_KEY": "private-test-string"}}
        response = await client.post("/v1/submissions", json=body)
        publish = await client.post("/v1/admin/proof/topics", json=topic.model_dump())
    assert response.status_code == 401
    assert "private-test-string" not in response.text
    assert publish.status_code == 401


async def test_pending_quota_prevents_parallel_paid_runs(setup):
    service, backend, _, _ = setup
    service.max_pending_per_miner = 1
    backend.release.clear()
    signed, data = submission()
    first = asyncio.create_task(service.submit(signed, data))
    await backend.entered.wait()
    second, _ = submission(nonce="34" * 32)
    try:
        with pytest.raises(ServiceError) as caught:
            await service.submit(second, data)
        assert caught.value.status == 429
        assert len(backend.jobs) == 1
        with service.store.transaction() as connection:
            assert (
                connection.execute(
                    "SELECT count(*) FROM proof_jobs WHERE state='running'"
                ).fetchone()[0]
                == 1
            )
    finally:
        backend.release.set()
        await first
    assert list(service.vault.root.iterdir()) == []


async def test_authenticated_lookup_recovers_pending_and_terminal_result(setup):
    service, backend, _, app = setup
    backend.release.clear()
    signed, data = submission()
    waiter = asyncio.create_task(service.submit(signed, data))
    await backend.entered.wait()

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        pending = await client.post("/v1/submissions/lookup", json=lookup_body(signed))
        backend.release.set()
        terminal_row = await waiter
        terminal = await client.post("/v1/submissions/lookup", json=lookup_body(signed))

    assert pending.status_code == 200
    assert pending.json() == {
        "id": terminal_row["id"],
        "topic_id": signed.topic_id,
        "miner_hotkey": signed.miner_hotkey,
        "epoch": 4,
        "status": "pending",
        "artifact_digest": signed.artifact_digest,
        "reason": None,
        "metrics": None,
        "evidence_digest": None,
    }
    assert terminal.status_code == 200
    assert terminal.json()["status"] == "accepted"
    assert terminal.json()["metrics"] == {"quality": 0.8}


@pytest.mark.parametrize(
    "change",
    [
        {"hotkey_signature": "00" * 64},
        {"claim": "changed without a matching signature"},
        {"miner_hotkey": "ab" * 32},
        {"submit_nonce": "34" * 32},
    ],
)
async def test_lookup_rejects_tampered_original_envelope(setup, change):
    service, _, _, app = setup
    signed, data = submission()
    await service.submit(signed, data)
    body = {**lookup_body(signed), **change}

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        response = await client.post("/v1/submissions/lookup", json=body)

    assert response.status_code == 401


async def test_lookup_requires_exact_payload_without_cross_miner_or_nonce_oracle(setup):
    service, _, _, app = setup
    signed, data = submission()
    await service.submit(signed, data)
    same_nonce_different_payload = sign_submission(
        {
            "topic_id": signed.topic_id,
            "artifact_digest": signed.artifact_digest,
            "claim": "A different, validly signed claim",
            "submit_nonce": signed.submit_nonce,
        },
        MINER,
    )
    other_miner = sign_submission(
        {
            "topic_id": signed.topic_id,
            "artifact_digest": signed.artifact_digest,
            "claim": signed.claim,
            "submit_nonce": signed.submit_nonce,
        },
        bytes([91]) * 32,
    )

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        changed = await client.post(
            "/v1/submissions/lookup", json=lookup_body(same_nonce_different_payload)
        )
        foreign = await client.post("/v1/submissions/lookup", json=lookup_body(other_miner))
        forbidden_env = await client.post(
            "/v1/submissions/lookup",
            json={**lookup_body(signed), "env": {"MINER_API_KEY": "must-not-be-sent"}},
        )
        forbidden_uri = await client.post(
            "/v1/submissions/lookup",
            json={**lookup_body(signed), "artifact_uri": "https://example.invalid/a.tar"},
        )

    assert changed.status_code == 404
    assert foreign.status_code == 404
    assert forbidden_env.status_code == 400
    assert "must-not-be-sent" not in forbidden_env.text
    assert forbidden_uri.status_code == 400


async def test_unknown_lookup_does_not_consume_nonce(setup):
    service, _, _, app = setup
    signed, data = submission()

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        missing = await client.post("/v1/submissions/lookup", json=lookup_body(signed))
        submitted = await post(client, signed, data)

    assert missing.status_code == 404
    assert submitted.status_code == 201


async def test_lookup_recovers_failed_terminal_job(setup):
    service, backend, _, app = setup
    backend.misbind = True
    signed, data = submission()
    with pytest.raises(ServiceError, match="evidence binding mismatch"):
        await service.submit(signed, data)

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        response = await client.post("/v1/submissions/lookup", json=lookup_body(signed))

    assert response.status_code == 200
    assert response.json()["status"] == "failed"
    assert response.json()["reason"] == "VM evidence binding mismatch"
    assert response.json()["metrics"] is None


async def test_status_requires_an_open_topic_compatible_with_backend_readiness(setup):
    service, _, topic, app = setup
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        ready = await client.get("/v1/status")

        class MissingRunner:
            async def readiness(self):
                return Readiness(topic.eval_image_digest, topic.inference_offer_commitment)

        service.backend = MissingRunner()
        incompatible = await client.get("/v1/status")

        class NoisyFailure:
            async def readiness(self):
                raise ServiceError(503, "unavailable " + "x" * 10_000)

        service.backend = NoisyFailure()
        failed = await client.get("/v1/status")

    assert ready.status_code == 200
    assert ready.json()["can_score"] is True
    assert ready.json()["reason"] is None
    assert ready.json()["open_topics"] == 1
    assert ready.json()["scorable_topics"] == [topic.id]
    assert ready.json()["custom_scorable_topics"] == [topic.id]
    assert ready.json()["harvest_scorable_topics"] == []
    assert ready.json()["pins"] == {
        "eval_image_digest": topic.eval_image_digest,
        "inference_offer_commitment": topic.inference_offer_commitment,
        "executor_config_commitment": None,
        "executor_max_deadline_s": None,
    }
    assert incompatible.json()["can_score"] is False
    assert incompatible.json()["reason"] == "custom runner unavailable"
    assert incompatible.json()["open_topics"] == 1
    assert failed.json()["can_score"] is False
    assert len(failed.json()["reason"].encode()) <= 512


async def test_status_excludes_deferred_topics_from_immediate_scoring(setup):
    service, _, topic, app = setup
    await service.publish(
        sign_topic(
            topic.model_copy(update={"revision": 2, "params": {"defer_scoring": "true"}}),
            OWNER,
        )
    )

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        response = await client.get("/v1/status")

    assert response.status_code == 200
    assert response.json()["can_score"] is False
    assert response.json()["open_topics"] == 1
    assert response.json()["deferred_topics"] == [topic.id]
    assert response.json()["scorable_topics"] == []
    assert response.json()["reason"] == "all open topics defer scoring"


async def test_an_orphan_credential_directory_does_not_block_startup(setup):
    service, _, topic, _ = setup
    await service.publish(
        sign_topic(
            topic.model_copy(
                update={
                    "revision": 2,
                    "params": {"miner_byok": "MINER_API_KEY", "defer_scoring": "true"},
                }
            ),
            OWNER,
        )
    )
    # A crash after the credential write but before enqueue leaves this behind.
    service.vault.put("ab" * 32, {"MINER_API_KEY": "private-test-value"})
    signed, data = submission(env={"MINER_API_KEY": "private-test-value"})
    queued = await service.submit(signed, data)

    await service.resume()

    assert [entry.name for entry in service.vault.root.iterdir()] == [queued["id"]]
