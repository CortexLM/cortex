#!/usr/bin/env bash
# proof-metal-smoke — run the ONE-task in-guest smoke on the KVM host over SSH.
#
# The cloud ↔ metal test path. From any box with `ssh <host>` (Owner /
# Architecte laptop, an ops box), ship THIS checkout's guest adaptor and the
# smoke driver to the metal host, resolve the topic's pinned pack there, and
# run `proof-experiment-smoke.py` against one task of the live topic. Nothing
# is deployed, tipped, re-baked, sealed, or persisted: the run happens in a
# temp dir on the host, prints the report the guest returned, and is removed.
#
# Usage:
#   proof-metal-smoke.sh --topic tbench --task <one-task-name> --artifact recipe.tar \
#       [--host cortex-metal] [--job evaluate|baseline] [--set k=v]... \
#       [--guest-agent-local target/x86_64-unknown-linux-musl/release/proof-vm-guest-agent | --guest-agent-remote /path] \
#       [--path-prepend /opt/harbor/venv/bin] [--byok-env OPENROUTER_API_KEY] \
#       [--out-dir ./smoke-out] [--keep] [--dry-run]
#
# Preconditions (checked, never assumed):
#   - `ssh -o BatchMode=yes <host> true` works (key in your agent; this box has
#     network to the host). A Cursor cloud VM has TCP reach but no key: the
#     script then prints the exact command the Owner must run and exits 2.
#   - The host holds the topic's pack: <pack-dir>/sha256-<hex>.tar
#     (PROOF_VM_AGENT_EXPERIMENT_PACK_DIR, default /var/lib/proof-vm/packs).
#   - For --job evaluate the topic's miner BYOK variable is exported in THIS
#     shell (default name: the topic's miner_byok). It travels to the host as a
#     0600 file the remote shell sources — never on argv, never in a log.
#   - A guest agent binary for the authoritative `agent` driver: build it
#     static here (`cargo build --release -p proof-vm-guest-agent-bin
#     --target x86_64-unknown-linux-musl`) and pass --guest-agent-local, or
#     name one already on the host with --guest-agent-remote. Without either
#     the driver falls back to `exec` (the adaptor run directly with the
#     derived guest env; same adaptor, no Rust binary).
#
# What it does NOT do: tip, deploy, re-bake, touch retained jails, touch the
# live evaluate slot, reseal, or write any row. It reads the topic document
# from the public gateway (GET) and the pack from the host, read-only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HOST="${PROOF_METAL_HOST:-cortex-metal}"
TOPIC=""
TASK=""
JOB="evaluate"
ARTIFACT=""
SETS=()
GUEST_AGENT_LOCAL=""
GUEST_AGENT_REMOTE=""
PATH_PREPEND=""
BYOK_ENVS=()
OUT_DIR="./smoke-out"
KEEP=0
DRY_RUN=0
CP="${PROOF_CP:-https://gateway.cortex.foundation/challenge/proof}"
PACK_DIR="${PROOF_METAL_PACK_DIR:-/var/lib/proof-vm/packs}"
ADAPTOR_DIR="$ROOT/deploy/guest/runners/rlm_fc_in_guest_harbor"
DRIVER="$ROOT/deploy/scripts/proof-experiment-smoke.py"
SSH_BIN="${PROOF_METAL_SSH:-ssh}"

usage() { sed -n '2,40p' "$0"; }
die() { echo "[metal-smoke] $*" >&2; exit 2; }
log() { echo "[metal-smoke] $*" >&2; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --host) HOST="${2:?}"; shift 2 ;;
    --topic) TOPIC="${2:?}"; shift 2 ;;
    --task) TASK="${2:?}"; shift 2 ;;
    --job) JOB="${2:?}"; shift 2 ;;
    --artifact) ARTIFACT="${2:?}"; shift 2 ;;
    --set) SETS+=("${2:?}"); shift 2 ;;
    --guest-agent-local) GUEST_AGENT_LOCAL="${2:?}"; shift 2 ;;
    --guest-agent-remote) GUEST_AGENT_REMOTE="${2:?}"; shift 2 ;;
    --path-prepend) PATH_PREPEND="${2:?}"; shift 2 ;;
    --byok-env) BYOK_ENVS+=("${2:?}"); shift 2 ;;
    --out-dir) OUT_DIR="${2:?}"; shift 2 ;;
    --cp) CP="${2:?}"; shift 2 ;;
    --pack-dir) PACK_DIR="${2:?}"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown arg: $1 (see --help)" ;;
  esac
done

[[ -n "$TOPIC" ]] || die "--topic is required"
[[ -n "$TASK" ]] || die "--task <one task name> is required: the smoke is exactly one task"
[[ "$TASK" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$ ]] || die "--task must be one task name"
[[ "$JOB" == "evaluate" || "$JOB" == "baseline" ]] || die "--job must be evaluate or baseline"
if [[ "$JOB" == "evaluate" ]]; then
  [[ -n "$ARTIFACT" && -f "$ARTIFACT" ]] || die "--artifact recipe.tar is required for evaluate"
fi
[[ -x "$ADAPTOR_DIR/run" && -f "$DRIVER" ]] || die "run from a cortex checkout: missing $ADAPTOR_DIR/run or $DRIVER"
command -v python3 >/dev/null || die "python3 is required locally"
command -v curl >/dev/null || die "curl is required locally"

# --- 1. SSH reach (fail closed with the Owner command) -----------------------
if ! "$SSH_BIN" -o BatchMode=yes -o ConnectTimeout=10 "$HOST" true 2>/dev/null; then
  cat >&2 <<EOF
[metal-smoke] no SSH access to $HOST from this box (BatchMode key auth failed).
[metal-smoke] This is expected on a Cursor cloud VM. Owner / Architecte: run from a box with the key,
[metal-smoke] on this branch, exactly:

  export ${BYOK_ENVS[0]:-OPENROUTER_API_KEY}=…        # the topic's miner BYOK (evaluate only; never on argv)
  ./deploy/scripts/proof-metal-smoke.sh --host $HOST --topic $TOPIC --task $TASK --job $JOB \\
      ${ARTIFACT:+--artifact $ARTIFACT }${GUEST_AGENT_LOCAL:+--guest-agent-local $GUEST_AGENT_LOCAL }${PATH_PREPEND:+--path-prepend $PATH_PREPEND }$(printf -- '--set %q ' "${SETS[@]}")

[metal-smoke] Expected: "[smoke] run → done" then a JSON summary with primary_value / n_scored / trials
[metal-smoke] for exactly one trial named ${TASK}__1. See docs/runbooks/proof-experiment-smoke.md.
EOF
  exit 2
fi
log "ssh $HOST ok"

# --- 2. Topic document (public GET) → pack digest --------------------------
TMP_LOCAL="$(mktemp -d "${TMPDIR:-/tmp}/proof-metal-smoke-XXXXXX")"
trap 'rm -rf "$TMP_LOCAL"' EXIT
TOPIC_JSON="$TMP_LOCAL/topic.json"
curl -fsS -m 30 "${CP%/}/v1/proof/topics/$TOPIC" -o "$TOPIC_JSON" || die "cannot GET the topic document from $CP"
read -r PACK_DIGEST RUNNER BYOK_NAME <<<"$(python3 - "$TOPIC_JSON" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d = d.get("topic", d)
p = (d.get("constraints") or {}).get("params") or {}
print(p.get("experiment_pack_digest", "-"), p.get("baseline_runner") or p.get("in_guest_benchmark_runner") or "-", p.get("miner_byok", "-"))
PY
)"
[[ "$PACK_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]] || die "topic $TOPIC pins no experiment_pack_digest ($PACK_DIGEST); not an in-guest topic"
[[ "$RUNNER" != "-" ]] || die "topic $TOPIC selects no in-guest runner"
[[ "$RUNNER" == "$(basename "$ADAPTOR_DIR")" ]] || log "warning: topic selects runner $RUNNER; shipping adaptor $(basename "$ADAPTOR_DIR")"
PACK_HEX="${PACK_DIGEST#sha256:}"
REMOTE_PACK="$PACK_DIR/sha256-$PACK_HEX.tar"
log "topic=$TOPIC runner=$RUNNER pack=$REMOTE_PACK task=$TASK job=$JOB"

if [[ "$JOB" == "evaluate" ]]; then
  [[ ${#BYOK_ENVS[@]} -gt 0 ]] || { [[ "$BYOK_NAME" != "-" ]] && BYOK_ENVS=("$BYOK_NAME"); }
  for name in "${BYOK_ENVS[@]}"; do
    [[ -n "${!name:-}" ]] || die "export $name in this shell first (the topic's miner BYOK); it is never passed on argv"
  done
fi

# --- 3. Remote preflight: pack present, python3, docker/harbor hints --------
REMOTE_TMP="$("$SSH_BIN" "$HOST" 'mktemp -d /tmp/proof-smoke-XXXXXX')"
cleanup_remote() {
  if [[ $KEEP -eq 1 ]]; then log "remote work kept at $HOST:$REMOTE_TMP"; else "$SSH_BIN" "$HOST" "rm -rf '$REMOTE_TMP'" || true; fi
}
trap 'cleanup_remote; rm -rf "$TMP_LOCAL"' EXIT
"$SSH_BIN" "$HOST" "test -f '$REMOTE_PACK'" || die "pack $REMOTE_PACK is not on $HOST (PROOF_VM_AGENT_EXPERIMENT_PACK_DIR?)"
"$SSH_BIN" "$HOST" "command -v python3 >/dev/null" || die "python3 missing on $HOST"
if ! "$SSH_BIN" "$HOST" "PATH=${PATH_PREPEND:+$PATH_PREPEND:}\$PATH command -v harbor >/dev/null"; then
  log "warning: 'harbor' not on PATH on $HOST${PATH_PREPEND:+ (with $PATH_PREPEND)}; the adaptor will fail closed at 'harbor is not on PATH' — pass --path-prepend <venv bin>"
fi
if ! "$SSH_BIN" "$HOST" "docker info >/dev/null 2>&1 || test -S /var/run/docker.sock"; then
  log "warning: no reachable docker daemon on $HOST for this user; the adaptor falls back to podman or fails closed"
fi

# --- 4. Ship this checkout's adaptor + driver (+ artefact, + agent) ---------
tar -C "$(dirname "$ADAPTOR_DIR")" -cf - "$(basename "$ADAPTOR_DIR")" | "$SSH_BIN" "$HOST" "tar -C '$REMOTE_TMP' -xf -"
"$SSH_BIN" "$HOST" "cat > '$REMOTE_TMP/proof-experiment-smoke.py' && chmod 0755 '$REMOTE_TMP/proof-experiment-smoke.py'" < "$DRIVER"
"$SSH_BIN" "$HOST" "cat > '$REMOTE_TMP/topic.json'" < "$TOPIC_JSON"
if [[ -n "$ARTIFACT" ]]; then
  "$SSH_BIN" "$HOST" "cat > '$REMOTE_TMP/recipe.tar'" < "$ARTIFACT"
fi
DRIVER_MODE="exec"
REMOTE_AGENT=""
if [[ -n "$GUEST_AGENT_LOCAL" ]]; then
  [[ -x "$GUEST_AGENT_LOCAL" ]] || die "$GUEST_AGENT_LOCAL is not an executable guest agent"
  "$SSH_BIN" "$HOST" "cat > '$REMOTE_TMP/proof-vm-guest-agent' && chmod 0755 '$REMOTE_TMP/proof-vm-guest-agent'" < "$GUEST_AGENT_LOCAL"
  REMOTE_AGENT="$REMOTE_TMP/proof-vm-guest-agent"
  DRIVER_MODE="agent"
elif [[ -n "$GUEST_AGENT_REMOTE" ]]; then
  "$SSH_BIN" "$HOST" "test -x '$GUEST_AGENT_REMOTE'" || die "$GUEST_AGENT_REMOTE is not executable on $HOST"
  REMOTE_AGENT="$GUEST_AGENT_REMOTE"
  DRIVER_MODE="agent"
else
  log "no guest agent binary given: using the exec driver (adaptor run directly; agent driver is the authoritative guest path)"
fi

# --- 5. BYOK as a 0600 file the remote shell sources (never argv) -----------
if [[ "$JOB" == "evaluate" && ${#BYOK_ENVS[@]} -gt 0 ]]; then
  {
    for name in "${BYOK_ENVS[@]}"; do printf '%s=%q\n' "$name" "${!name}"; done
  } | "$SSH_BIN" "$HOST" "umask 077; cat > '$REMOTE_TMP/byok.env'"
fi

# --- 6. Run --------------------------------------------------------------------
REMOTE_ARGS=(
  --topic-json "$REMOTE_TMP/topic.json"
  --job "$JOB"
  --tasks "$TASK"
  --pack-tar "$REMOTE_PACK"
  --runner-dir "$REMOTE_TMP/$(basename "$ADAPTOR_DIR")"
  --driver "$DRIVER_MODE"
  --work-root "$REMOTE_TMP/work"
  --out "$REMOTE_TMP/outcome.json"
)
[[ -n "$ARTIFACT" ]] && REMOTE_ARGS+=(--artifact-tar "$REMOTE_TMP/recipe.tar")
[[ -n "$REMOTE_AGENT" ]] && REMOTE_ARGS+=(--guest-agent "$REMOTE_AGENT")
[[ -n "$PATH_PREPEND" ]] && REMOTE_ARGS+=(--path-prepend "$PATH_PREPEND")
for s in "${SETS[@]}"; do REMOTE_ARGS+=(--set "$s"); done
for name in "${BYOK_ENVS[@]}"; do REMOTE_ARGS+=(--byok-env "$name"); done
[[ $DRY_RUN -eq 1 ]] && REMOTE_ARGS+=(--dry-run)
REMOTE_CMD="$(printf '%q ' python3 "$REMOTE_TMP/proof-experiment-smoke.py" "${REMOTE_ARGS[@]}")"
mkdir -p "$OUT_DIR"
log "running on $HOST (driver=$DRIVER_MODE); log → $OUT_DIR/smoke.log"
set +e
"$SSH_BIN" "$HOST" "set -o pipefail; if [ -f '$REMOTE_TMP/byok.env' ]; then set -a; . '$REMOTE_TMP/byok.env'; set +a; rm -f '$REMOTE_TMP/byok.env'; fi; $REMOTE_CMD" 2>&1 | tee "$OUT_DIR/smoke.log"
RC=${PIPESTATUS[0]}
set -e
if "$SSH_BIN" "$HOST" "test -f '$REMOTE_TMP/outcome.json'"; then
  "$SSH_BIN" "$HOST" "cat '$REMOTE_TMP/outcome.json'" > "$OUT_DIR/outcome.json"
  log "outcome → $OUT_DIR/outcome.json"
fi
if [[ $RC -eq 0 ]]; then
  log "PASS: the guest returned a report for $TOPIC/$TASK (nothing persisted)"
else
  log "FAIL (rc=$RC): read $OUT_DIR/smoke.log; with --keep the remote work root holds the adaptor's harbor.run.log / tasks-filtered"
fi
exit "$RC"
