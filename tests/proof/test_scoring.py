from cortex.proof.models import EvaluationReport
from cortex.proof.scoring import judge, payout


def row(topic, hotkey, primary, artifact="11" * 32, duplicate=False):
    report = EvaluationReport(
        topic_id=topic.id,
        topic_digest=topic.content_digest(),
        submission_id="22" * 32,
        artifact_digest=artifact,
        verdict="clean",
        reproduced=True,
        claim_holds=True,
        rule_results={rule.id: True for rule in topic.checklist},
        metrics={"quality": primary},
        flops_used=1,
        wall_seconds=1,
        evidence_digest="33" * 32,
        vm_id="test-vm",
        sandboxed=True,
        teardown_confirmed=True,
        near_duplicate=duplicate,
    )
    return {
        "epoch": 4,
        "topic_id": topic.id,
        "topic_digest": topic.content_digest(),
        "hotkey": hotkey,
        "status": "accepted",
        "report": report.model_dump(),
    }


def test_winner_ties_split_only_their_topic_mass(setup):
    _, _, topic, _ = setup
    second = topic.model_copy(update={"id": "another-topic"})
    scores = payout(
        [topic, second],
        [row(topic, "alice", 0.9), row(topic, "bob", 0.9), row(topic, "carol", 0.8)],
        4,
    )
    assert scores == {"alice": 250000, "bob": 250000}


def test_multiple_topics_add_masses_and_missing_topics_do_not_dilute_a_pass(setup):
    _, _, topic, _ = setup
    second = topic.model_copy(update={"id": "another-topic"})
    scores = payout(
        [topic, second],
        [row(topic, "alice", 0.9), row(second, "alice", 0.8), row(second, "bob", 0.7)],
        4,
    )
    assert scores == {"alice": 1000000}


def test_discovery_duplicate_gets_floor_but_never_novelty(setup):
    _, _, original, _ = setup
    topic = original.model_copy(update={"payout_mode": "discovery"})
    scores = payout([topic], [row(topic, "alice", 0.8), row(topic, "bob", 0.8)], 4)
    assert scores == {"alice": 850000, "bob": 150000}


def test_discovery_novelty_respects_a_better_champion(setup):
    _, _, original, _ = setup
    topic = original.model_copy(update={"payout_mode": "discovery"})
    scores = payout(
        [topic],
        [row(topic, "alice", 0.8), row(topic, "bob", 0.9, artifact="44" * 32)],
        4,
        champions={topic.id: 0.85},
    )
    assert scores == {"alice": 150000, "bob": 850000}


def test_rejected_and_previous_epoch_results_cannot_receive_rewards(setup):
    _, _, topic, _ = setup
    rejected = row(topic, "alice", 0.9)
    rejected["status"] = "rejected"
    old = row(topic, "bob", 0.99)
    old["epoch"] = 3
    assert payout([topic], [rejected, old], 4) == {}


def test_suspicious_result_cannot_be_made_payable_by_database_status_alone(setup):
    _, _, topic, _ = setup
    tampered = row(topic, "alice", 0.9)
    tampered["report"]["rule_results"] = {"integrity-check": False}
    assert payout([topic], [tampered], 4) == {}


def test_preflight_rejection_needs_no_paid_measurement(setup):
    _, _, topic, _ = setup
    report = EvaluationReport.model_validate(row(topic, "alice", 0.9)["report"])
    rejected = report.model_copy(
        update={
            "verdict": "reject",
            "reproduced": False,
            "claim_holds": False,
            "rule_results": {"integrity-check": False},
            "metrics": {},
            "flops_used": 0,
        }
    )
    assert "checklist rejected" in judge(topic, rejected)
