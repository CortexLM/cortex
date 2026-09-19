from cortex.proof.models import Rule
from cortex.proof.service import sign_topic
from cortex.proof.store import ProofStore

from .conftest import OWNER, submission


async def test_rule_revision_does_not_erase_already_earned_epoch_rewards(setup):
    service, _, topic, _ = setup
    body, artifact = submission()
    await service.submit(body, artifact)
    revised = sign_topic(
        topic.model_copy(
            update={
                "revision": 2,
                "checklist": [
                    *topic.checklist,
                    Rule(id="new-check", text="Additional future check"),
                ],
            }
        ),
        OWNER,
    )
    await service.publish(revised)
    assert service.scores(4) == {body.miner_hotkey: 1000000}


async def test_older_champion_and_seen_artifact_remove_repeat_novelty_after_restart(
    setup, tmp_path
):
    service, _, topic, _ = setup
    topic = sign_topic(topic.model_copy(update={"revision": 2, "payout_mode": "discovery"}), OWNER)
    await service.publish(topic)
    body, artifact = submission()
    await service.submit(body, artifact)
    service.epoch = lambda: 5
    repeated, _ = submission(nonce="34" * 32)
    await service.submit(repeated, artifact)
    original_store = service.store
    with ProofStore(tmp_path / "proof.sqlite3") as restarted:
        service.store = restarted
        assert service.scores(5) == {body.miner_hotkey: 300000}
    service.store = original_store


async def test_closing_topic_does_not_erase_previous_epoch_rewards(setup):
    service, _, topic, _ = setup
    body, artifact = submission()
    await service.submit(body, artifact)
    service.epoch = lambda: 5
    await service.publish(
        sign_topic(topic.model_copy(update={"revision": 2, "status": "closed"}), OWNER)
    )
    assert service.scores(4) == {body.miner_hotkey: 1000000}
