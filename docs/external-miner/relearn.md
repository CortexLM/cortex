<!-- protocol_version: 1 -->

# Relearn — miners

One live challenge. Long guide, eval image, and harness:
[CortexLM/relearn](https://github.com/CortexLM/relearn).
Cortex pin: [`config/relearn-pin.toml`](../../config/relearn-pin.toml).

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

## Submit

```bash
curl -sS -X POST https://<gateway>/challenge/relearn/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
    "artifact_digest": "<sha256 of your artifact>",
    "artifact_uri": "optional-url"
  }'
```

Poll `GET /challenge/relearn/v1/submissions/{id}`. Eligible runs sit at
`awaiting_admin` until an operator promotes. You do not promote.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
