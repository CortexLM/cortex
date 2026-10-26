# Cloudflare resources created by batch v3-dataplane-cloudflare

Every resource below was created for a test of ADR 0003 in the Cortex Foundation account, with the
`cortex-v3-test-` prefix. All of them were deleted at the end of the session; none is kept.
A read-only inventory afterwards showed the same 8 buckets as before the batch.
Existing production resources (the `relay.joinbase.ai` relay, Bunny, the DNS of existing zones)
were not touched, and no DNS record was created.

The deletion commands read the credentials from their file inside a subshell and never print
them. A bucket must be empty before it can be deleted.

| Name | Kind | Purpose | Created (UTC) | Deleted (UTC) |
| --- | --- | --- | --- | --- |
| `cortex-v3-test-dataplane-20261026` | R2 bucket, location `WEUR`, Standard | R2 behaviour probes and the opt-in `cloudflare` 64 MiB throughput test | 2026-10-26T19:14:35Z | 2026-10-26T20:16:38Z |

## Deletion

Abort open uploads and delete every object, then delete the bucket. From `hypertrain/`:

```bash
( set -a; . /var/lib/openbot/.cloudflare-cortex.env; set +a
  uv run --no-sync python -I -c '
import os
from hypertrain.storage.r2 import R2Client, R2Config
c = R2Client(R2Config("", os.environ["R2_ACCESS_KEY_ID"], os.environ["R2_SECRET_ACCESS_KEY"],
                      "cortex-v3-test-dataplane-20261026", endpoint=os.environ["R2_S3_ENDPOINT"]))
for k, u in c.list_uploads(""): c.abort_multipart(k, u)
for k, _ in c.list_prefix(""): c.delete_object(k)
print("left:", len(c.list_prefix("")), len(c.list_uploads("")))'
  printf 'Authorization: Bearer %s\n' "$CLOUDFLARE_API_TOKEN" \
  | curl -sS -X DELETE -H @- \
    "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets/cortex-v3-test-dataplane-20261026" \
  | python3 -c 'import json,sys; b=json.load(sys.stdin); print(b["success"], b["errors"])' )
```

`printf` is a shell builtin and curl reads the header from stdin, so the token appears in no
process argument list.
