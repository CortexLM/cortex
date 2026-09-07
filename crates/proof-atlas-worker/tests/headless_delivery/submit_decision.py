# Deterministic faux-model output, executed only inside the real isolated kernel.
import os
from rlm import host_request

assert os.getuid() == 65532
assert not os.path.exists("/root/cortex")
assert not os.path.exists("/var/run/docker.sock")
assert not any(name in os.environ for name in (
    "DATABASE_URL", "BASE_GATEWAY_SK", "PROOF_SK", "OPENAI_API_KEY",
    "LIUM_API_KEY", "WANDB_API_KEY", "FACTORY_API_KEY",
))

async def controller(operation, arguments):
    return await host_request("cortex.call", {
        "operation": operation, "arguments": arguments,
    })

page = await controller("read_evidence", {"limit": 1})
history = await controller("history", {"limit": 1})
assert page["snapshot"] == history["snapshot"]
assert page["total"] == history["total"] == 1
evidence_digest, evidence = page["items"][0]
contribution_digest, contribution = history["items"][0]
assert contribution["rewardable"]
assert evidence_digest in contribution["evidence_digests"]
record = await controller("read_evidence", {"evidence_digest": evidence_digest})
assert record["evidence"]["summary"] == evidence
artifact = await controller("read_evidence", {
    "evidence_digest": evidence_digest,
    "artifact_digest": record["evidence"]["recipe"]["candidate_script_digest"],
    "offset": 0, "limit": 8,
})
assert bytes.fromhex(artifact["hex"]) == b"print('l"

result = await controller("submit_decision", {
    "schema_version": 1,
    "scoring_version": __SCORING_VERSION__,
    "snapshot": page["snapshot"],
    "awards": [{
        "contribution_digest": contribution_digest,
        "miner_hotkey": bytes(contribution["miner_hotkey"]).hex(),
        "units": 500000,
        "evidence_digests": contribution["evidence_digests"],
        "rationale": "Synthetic local fixture, not a scientific discovery.",
        "decay": {
            "first_round": 0, "initial_units": 500000,
            "retention_ppm": 500000, "expires_round": 10,
        },
        "decay_revision": None,
    }],
    "rationale": "Deterministic faux inference through the real isolated kernel.",
})
assert result["publication_pending"] is True
assert len(result["decision_digest"]) == 64
print("ATLAS_KERNEL_DECISION_COMMITTED")
