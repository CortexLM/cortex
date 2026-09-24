<!-- protocol_version: 1 -->

# Challenge containers

A Cortex challenge is a Docker image that scores miners. The master runs it,
reads its weights once per completed epoch, signs one leaf per expected hotkey
and seals the epoch. Validators only verify the sealed bundle and submit it.

```text
owner (offline)          challenges.toml v>=3: id, leaf key, emission bps, policy
master operator          challenge-registry.toml: image, channel, resources, env
challenge-supervisor     pulls, verifies, canaries, runs and updates containers
challenge container      public routes + GET /internal/v1/get_weights
master                   polls weights, signs leaves, seals, proxies /challenge/<id>/
validator                GET /v1/weights/latest + /v1/bundle/<epoch>, recompute, submit
```

The owner-signed trust root decides which challenges earn emission and how
much. The unsigned registry only decides what runs. A registered challenge that
is absent from the trust root runs without emission (burn-in). A trusted
challenge that is not registered or cannot score emits
`NoScore(ChallengeInternal)` for every expected hotkey and its share burns.

## Container contract, version 1

The container serves plain HTTP on port `8000` of the private
`cortex-challenges` network. It never publishes a host port. It runs as UID
65532 with a read-only root filesystem, no capabilities, a `/tmp` tmpfs and one
writable named volume at `/data`. The image must create `/data` owned by
`65532:65532`, because Docker copies that ownership into a new named volume.

### Environment

| Variable | Value |
| --- | --- |
| `CHALLENGE_SLUG` | the challenge id, for example `bounty` |
| `CHALLENGE_STATE_DIR` | `/data` |
| `CHALLENGE_INTERNAL_TOKEN_FILE` | `/run/secrets/internal.token`, the master bearer |
| `CHALLENGE_ADMIN_TOKEN_FILE` | `/run/secrets/admin.token`, the operator bearer, optional |
| `CHALLENGE_MASTER_URL` | `http://cortex-master:8080`, for `/v1/metagraph/latest` |
| registry `env` | challenge-specific settings |

Every file under `/run/secrets/` comes from the host directory
`<BASE_CHALLENGE_SECRETS_DIR>/<id>/`, mounted read-only. A container never
receives a leaf-signing seed.

### Routes

| Route | Contract |
| --- | --- |
| `GET /health` | `200 {"ok": true}` when the container can score; `503` otherwise. Readiness only: an outage of an external dependency must not restart the container |
| `GET /version` | `200 {"slug", "version", "contract": 1, "capabilities": [...]}` |
| `GET /internal/v1/get_weights?epoch=<u64>` | weights for a completed epoch, described below |
| any other path | public, proxied when the capabilities include `proxy_routes` |

`capabilities` is a subset of `get_weights` and `proxy_routes`.

`get_weights` requires `Authorization: Bearer <internal token>` (`401`
otherwise) and `X-Platform-Challenge-Slug: <slug>` (`403` on mismatch). It
returns:

```json
{
  "challenge_slug": "bounty",
  "epoch": 25316,
  "weights": {"5F...": 3.0, "5G...": 1.0},
  "full_share_mass": 10.0,
  "metadata": {},
  "computed_at": "2026-09-24T12:00:00Z"
}
```

- `weights` maps an SS58 or 64-hex hotkey to a finite non-negative number. It
  holds at most 65,536 entries.
- `full_share_mass` is optional. When it is present, the challenge pays its full
  share only once the total weight of expected hotkeys reaches it, and the
  shortfall burns. When it is omitted or `null`, the expected weights are
  normalized to the full share.
- The first successful answer for an epoch is final. The container persists it
  and returns the same body for every later call with that epoch.
- `503` means the container cannot score. The master then burns the share for
  that epoch.
- The call must answer within 60 seconds.

### Leaves the master signs

Let `E` be the owner-policy participant set of the sealed metagraph, `w_i` the
returned weight (0 when absent), `W = sum(w_i for i in E)` and
`D = max(W, full_share_mass or 0)`. A hotkey outside `E` is ignored and never
changes `D`.

| Algorithm | Leaf score for `i` in `E` with `w_i > 0` | Challenge payout |
| --- | --- | --- |
| 3 (trust root version >= 3) | `floor(10^12 * w_i / D)` | `share * sum(leaves) / 10^12` |
| 2 (bounty/proof 3000/7000) | `w_i`, which must be an integer | Bounty: `share * min(N, 10) / 10`; Proof: `share` |
| 1 (legacy bounty/proof 2000/8000) | `w_i`, which must be an integer | `share` when any leaf is positive |

Every other hotkey in `E` receives `NoScore(NotAttempted)`. A failed or invalid
call gives `NoScore(ChallengeInternal)` to every hotkey in `E`. Unpaid mass
always burns to UID0 and never moves to another challenge. Under algorithm 3,
Bounty with `full_share_mass = 10` pays exactly what algorithm 2 pays:
`share * min(N, 10) / 10` in total and `share * n_i / max(10, N)` per author.

### Public proxy

The master forwards `ANY /challenge/<id>/<path>?<query>` to
`http://cortex-challenge-<id>:8000/<path>?<query>`. It refuses the following
with `404`:

- a path under `internal/`;
- an empty, `.` or `..` segment;
- a backslash or a percent sign in the path.

It forwards the request body, up to the registry `proxy_body_limit` (default 1
MiB, `413` beyond it). Only the `content-type`, `accept` and `authorization`
request headers pass through. The master sets `X-Forwarded-For` itself,
overwriting any client value. It returns the status,
the `content-type` and at most 8 MiB of the response body. The request times out
after the registry `proxy_timeout_seconds` (default 30). A failure returns
`502`, and an unknown challenge returns `404`.

`GET /v1/metagraph/latest` on the master returns
`{"epoch", "block", "netuid", "hotkeys": {"<ss58>": uid}}` from the latest
sealed bundle, or `503` when no seal exists. Challenges use it to admit only
registered hotkeys.

## Images and updates

A challenge repository publishes `ghcr.io/<owner>/<repo>`:

| Tag | Moves | Built by |
| --- | --- | --- |
| `sha-<commit>` | never | every push to `main`, after tests |
| `edge` | every push to `main` | alias of the tested `sha-` digest |
| `vX.Y.Z` | never | annotated tag; aliases the existing `sha-` digest, no rebuild |
| `stable` | every release | alias of the release digest |

Every image carries these labels: `org.opencontainers.image.source`,
`org.opencontainers.image.version`, `org.opencontainers.image.revision`,
`io.cortex.challenge.slug=<id>` and `io.cortex.challenge.contract=1`.

For each registry entry, the supervisor runs this loop every `poll_seconds`:

1. Pull `image:channel`, or `image@pin` when a pin is set, and read the digest.
   An unchanged digest only ensures the container is running.
2. Refuse the image when the slug label differs from the id, the contract label
   is not `1`, or the source label differs from the registry `source`. The
   current container keeps running.
3. Unless `attestation = false`, require a GitHub build-provenance attestation
   for the digest in the `source` repository.
4. Start a canary with the same image, no secrets and a tmpfs `/data`. It must
   answer `/version` with the expected slug and contract within 60 seconds.
5. Replace the container: stop the old one and start the new one on the same
   volume. Then wait until `/version` returns the expected slug and contract.
   `/health` is readiness and is only reported, because an external outage must
   not trigger a rollback.
6. If the new container fails, recreate the previous digest and log the refusal.
   The refused digest is not retried until the channel moves.

A managed container whose id is no longer in the registry is stopped and
removed. Its volume is kept.
