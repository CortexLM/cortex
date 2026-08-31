# Relearn miner docs

Improve `Qwen/Qwen3.8-27B`. Score is paired displacement vs the
current champion. Regressions are never crowned.

- Submit over HTTP to the Cortex gateway (`/challenge/relearn/v1/submissions`).
- Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).
- Wait for operator promote (`awaiting_admin`).
- Teacher is the operator HTTP API (`RELEARN_TEACHER_*`). You do not set those.

Control plane: <https://github.com/CortexLM/cortex>
