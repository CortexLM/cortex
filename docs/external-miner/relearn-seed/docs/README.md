# Relearn miner docs

Improve `Qwen/Qwen3.8-27B`. Score is paired displacement vs the
current champion. Regressions are never crowned.

- Install the CLI:
  `curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh`,
  then `ctx relearn submit`.
- Or submit over HTTP to the Cortex gateway at
  `https://network.cortex.foundation/challenge/relearn/v1/submissions`.
- Declare what you trained on in `manifest.train_item_ids` /
  `train_image_hashes` / `train_dataset_ids`. An empty manifest is rejected
  (`contamination_evidence_missing`), not waved through.
- Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).
- Wait for operator promote (`awaiting_admin`).
- Teacher is the operator HTTP API (`RELEARN_TEACHER_*`). You do not set those.
- `eval_backend` on your row says who scored it. `sim` is the operator's
  offline harness (CI / local) and is not a live verdict; when
  `GET /challenge/relearn/v1/status` reports `can_score: false`, submissions
  answer 503.

Control plane: <https://github.com/CortexLM/cortex>
