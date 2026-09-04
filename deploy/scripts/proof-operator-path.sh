#!/usr/bin/env bash
# Proof operator path: holdout → seal baseline on the pinned image → sign → publish.
#
# Secrets stay off git. This script prints the commands; it never writes under
# config/ or docs/, and it never prints a mini-secret.
#
# Topic schema (payout_mode, validation, discovery shares) is owned by the
# control-plane PR. This path only runs holdout / baseline / admin inject
# against whatever signed document the operator drafted.
#
# can_score becomes true only when ALL of: real digest pin, wired harvest
# (LIUM_API_KEY + SSH pub), ≥1 open signed topic, verified holdout, sealed
# baseline. Empty digest stays 503 (never invent a sha256).
set -euo pipefail

TOPIC_ID="${PROOF_TOPIC_ID:-dt-no-ib-v0}"
SECRETS="${PROOF_SECRETS_DIR:-$HOME/.base-secrets/proof}"
IMAGE="ghcr.io/cortexlm/proof-eval"
PROXY="Qwen/Qwen3.8-0.6B"

digest=$(python3 - <<'PY' || true
import tomllib, pathlib, sys
p = pathlib.Path("config/proof-pin.toml")
if not p.is_file():
    sys.exit(0)
print(tomllib.loads(p.read_text()).get("eval_image_digest","").strip())
PY
)
digest="${PROOF_EVAL_IMAGE_DIGEST:-$digest}"

cat <<EOF
# Proof operator ceremony (${TOPIC_ID})

# 0. Pin must already carry a real sha256. Empty digest → every submit 503.
#    Current eval_image_digest=${digest:-<empty>}
#    Proxy the image bakes: ${PROXY}

mkdir -p '${SECRETS}'
chmod 700 '${SECRETS}'

# 1. Select a stratified holdout (records NEVER enter git).
cargo run -p xtask -- proof-holdout \\
  --topic-id '${TOPIC_ID}' \\
  --synthetic \\
  --salt "\$PROOF_HOLDOUT_SALT" \\
  --size 120 \\
  --out '${SECRETS}/holdouts.json'

# Production: drop --synthetic and pass --catalog <private.json>.

# 2. Stage shard bytes the image will score (content-addressed).
#    PROOF_HOLDOUT_STORE/\${content_sha256} ← packed shard text.
#    The request carries fingerprints only.

# 3. Seal the AdamW / comms baseline ON THE PINNED IMAGE (not sim).
#    The image enforces 12.5 Gbit/s / no IB / no NVLink / no NCCL fast path
#    before it will emit numbers.
if [ -n "${digest}" ]; then
  echo "docker run --rm --entrypoint /usr/bin/proof-eval ${IMAGE}@${digest} baseline --request /tmp/proof_eval/request.json --out /tmp/proof_eval/baseline.json"
else
  echo "# digest still empty — do not invent a sha256; wait for publish-proof-eval-image"
fi
# Put the measurement JSON at ${SECRETS}/baselines.json keyed by topic id.
# script_sha256 = sha256(/opt/proof-eval/baselines/adamw.py) from that image.
# metrics_commitment = BaselineMeasurement.commitment() over the vector.

# 4. Sign the operator draft (YAML or JSON). Schema is payout_mode +
#    validation.{score_on,accept_if,reject_if} + metric; this helper does
#    not invent those fields. --synthetic is local/dev; production uses
#    --holdout so the commitment matches the host file.
cargo run -p xtask -- proof-topic \\
  --input '${SECRETS}/${TOPIC_ID}.yaml' \\
  --secret deploy/secrets/proof_sk \\
  --holdout '${SECRETS}/holdouts.json' \\
  --out '${SECRETS}/topics.json'

# 5. Publish (dynamic inject). Admin bearer from PROOF_ADMIN_TOKENS_FILE.
# curl -sS -X POST "\$PROOF_BASE/v1/admin/proof/topics" \\
#   -H "authorization: Bearer \$PROOF_ADMIN_TOKEN" \\
#   -H 'content-type: application/json' \\
#   --data-binary @${SECRETS}/topics.json

# 6. Point the host at the operator files (never in git):
#   PROOF_TOPICS_FILE=${SECRETS}/topics.json
#   PROOF_HOLDOUT_FILE=${SECRETS}/holdouts.json
#   PROOF_BASELINE_FILE=${SECRETS}/baselines.json
#   LIUM_API_KEY=…  LIUM_SSH_PUBLIC_KEY_FILE=…
# Restart proof-challenge, then:
#   curl -sS "\$PROOF_BASE/v1/status" | jq '{can_score,eval_image_digest,open_topics,live_harvest_wired,baseline_sealed}'

# can_score is true only with: real digest + harvest wired + open topic +
# sealed baseline + verified holdout. Empty digest stays 503.
EOF
