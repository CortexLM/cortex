<!-- protocol_version: 1 -->

# Relearn

Public challenge repo for the Cortex **Relearn** subnet.

**This repo is miner + eval only.** It does not contain the Cortex control
plane, gateway, or validator. Those live in [`CortexLM/cortex`](https://github.com/CortexLM/cortex).

| Pin | Value |
|-----|--------|
| Base model | `Qwen/Qwen3.8-Flash-Next` |
| Teacher / judge | HTTP API (operator env) |
| Score | Displacement vs the previous champion |

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).
Teacher URL, model, and key are operator host env:
`RELEARN_TEACHER_API_URL`, `RELEARN_TEACHER_MODEL`, `RELEARN_TEACHER_API_KEY`.

Cortex pins this repo's git SHA and the eval image digest in
`config/relearn-pin.toml`. Deploy = bump the pin after this repo's CI is green.

## Miner path

See [docs/](./docs/) and the control-plane mirror
[`docs/external-miner/relearn.md`](https://github.com/CortexLM/cortex/blob/main/docs/external-miner/relearn.md).

```bash
curl -sS -X POST https://<gateway>/challenge/relearn/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{"miner_hotkey":"<64-hex>","artifact_digest":"<sha256>"}'
```

Never commit `LIUM_API_KEY` or any secret.

## Layout

| Path | Role |
|------|------|
| `eval/Dockerfile` | Digest-pinned eval image |
| `eval/harness/` | Pod entry: load base + miner artifact, score holdout |
| `eval/generators/` | Disjoint train / eval synthetic factory |
| `eval/teacher/` | HTTP judge (env-only URL; never serves miner weights) |
| `eval/decontam/` | n-gram blocklist |
| `docs/` | Miner-facing human docs |
