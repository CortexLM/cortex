# Relearn LLM (live challenge)

Sibling challenges: [`RELEARN-T2I.md`](./RELEARN-T2I.md) (image generation,
judged by Q-Judger) and [`RELEARN-MM.md`](./RELEARN-MM.md) (vision encoder on
this challenge's champion). They share the champion-versus-challenger holdout
shape and the Lium payment model, and each signs leaves under its own key.

Control-plane notes. Miners start at [`external-miner/relearn.md`](./external-miner/relearn.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

Eval image and harness live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
This repo pins them in `config/relearn-pin.toml`.

| Field | Value |
|-------|--------|
| `challenge_id` | `relearn` |
| `challenge_scoring_version` | `1` |
| Base model | `Qwen/Qwen3.8-Flash-Next` |
| Teacher / judge | HTTP API (operator sets `RELEARN_TEACHER_*`) |
| Port | `8095` (local host `28095`) |
| Emission | `4000` bps (default) |

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). Operator promote is
`POST /v1/admin/promote`. Epoch emit is champion lattice; others `NoScore` (D24).
