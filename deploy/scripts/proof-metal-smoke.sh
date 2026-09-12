#!/usr/bin/env bash
# proof-metal-smoke — OPERATOR-RUN single-task in-guest smoke on the KVM host.
#
# Cursor cloud agents have NO metal SSH (owner decision, 2026-09-12); this
# script is for an Owner / Dev box that already holds a key to the host. It
# ships THIS checkout's guest adaptor and the smoke driver to the host,
# resolves the topic's pinned pack there (read-only), and runs
# `proof-experiment-smoke.py` against exactly ONE task of a live topic. It
# prints the report the guest returned plus a `smoke_evidence` block to
# paste into the PR. Nothing is deployed, tipped, re-baked, sealed, or
# persisted; the run happens in a scratch dir the host policy allows and is
# removed unless --keep.
#
# Usage:
#   proof-metal-smoke.sh --topic tbench --task <one-task-name> --artifact recipe.tar \
#       [--host cortex-metal] [--job evaluate|baseline] [--set k=v]... \
#       [--guest-agent-local target/x86_64-unknown-linux-musl/release/proof-vm-guest-agent | --guest-agent-remote /path] \
#       [--path-prepend /path/to/harbor/venv/bin] [--byok-env OPENROUTER_API_KEY] \
#       [--remote-scratch /var/lib/proof/<wd>] [--out-dir ./smoke-out] [--keep] [--dry-run]
#
# OFF LIMITS — this script never does, and you must not use it to:
#   - tip, recreate compose, register backends, re-bake, or re-pin anything
#     (the guest pin stays what it is until an Architecte RE-LOCK);
#   - touch baselines.json / topics.json / proof_sk / hotkeys / BYOK files;
#   - destroy or read a retained jail (/var/lib/proof-vm/retained/...);
#   - kill or compete with a live experiment VM (a paid miner evaluate may be
#     in flight; run this when an experiment slot is free — the agent/exec
#     drivers take no slot but do use the host's Docker and CPU);
#   - write scratch anywhere but under /var/lib/proof/<your-wd>/ on the host
#     (--remote-scratch; default /var/lib/proof/smoke-<user>-<stamp>).
#
# Preconditions (checked, never assumed):
#   - `ssh -o BatchMode=yes <host> true` works from THIS box.
#   - The host holds the topic's pack: <pack-dir>/sha256-<hex>.tar
#     (PROOF_VM_AGENT_EXPERIMENT_PACK_DIR, default /var/lib/proof-vm/packs).
#   - For --job evaluate the topic's miner BYOK variable is exported in THIS
#     shell (default name: the topic's miner_byok). It travels to the host as
#     a 0600 file the remote shell sources and deletes — never argv.
#   - A guest agent binary for the authoritative `agent` driver: build it
#     static (`cargo build --release -p proof-vm-guest-agent-bin
#     --target x86_64-unknown-linux-musl`) and pass --guest-agent-local, or
#     name one already on the host with --guest-agent-remote. Without either
#     the driver falls back to `exec` (adaptor run directly with the derived
#     guest env; same adaptor, no Rust binary).
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
REMOTE_SCRATCH="${PROOF_METAL_SCRATCH:-}"
ADAPTOR_DIR="$ROOT/deploy/guest/runners/rlm_fc_in_guest_harbor"
DRIVER="$ROOT/deploy/scripts/proof-experiment-smoke.py"
SSH_BIN="${PROOF_METAL_SSH:-ssh}"

usage() { sed -n '2,50p' "$0"; }
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
    --remote-scratch) REMOTE_SCRATCH="${2:?}"; shift 2 ;;
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
# Scratch policy: only under /var/lib/proof/<wd>/ on the host. The value is
# interpolated into remote mkdir / rm -rf, so it is held to plain segments
# (no `.`, `..`, empty segment, trailing slash, whitespace, or shell
# metacharacter) here, and resolved with realpath on the host before use.
SCRATCH_ROOT="/var/lib/proof"
if [[ -z "$REMOTE_SCRATCH" ]]; then
  REMOTE_SCRATCH="$SCRATCH_ROOT/smoke-${USER:-op}-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$REMOTE_SCRATCH" =~ ^/var/lib/proof(/[A-Za-z0-9][A-Za-z0-9._-]{0,127})+$ ]]; then
  die "--remote-scratch must be $SCRATCH_ROOT/<plain segments> (no '..', '.', '//', trailing '/', spaces, or metacharacters), got $REMOTE_SCRATCH"
fi
if [[ "$REMOTE_SCRATCH" =~ (^|/)\.\.?(/|$) ]]; then
  die "--remote-scratch must not contain '.' or '..' segments, got $REMOTE_SCRATCH"
fi
case "$REMOTE_SCRATCH" in
  */retained*|*/proof-vm/*) die "--remote-scratch must not point at retained jails or the orchestrator's state" ;;
esac
# Every remote command that names the scratch goes through this one guard:
# the host resolves the path (symlinks and all) and refuses anything that
# does not land strictly under the scratch root.
REMOTE_GUARD="p=\$(realpath -m -- '$REMOTE_SCRATCH') && case \"\$p\" in $SCRATCH_ROOT/?*) ;; *) echo 'scratch resolves outside $SCRATCH_ROOT' >&2; exit 3 ;; esac"

# --- 1. SSH reach (fail closed; this is an operator box's key, never an agent's) ---
if ! "$SSH_BIN" -o BatchMode=yes -o ConnectTimeout=10 "$HOST" true 2>/dev/null; then
  cat >&2 <<EOF
[metal-smoke] no SSH access to $HOST from this box (BatchMode key auth failed).
[metal-smoke] Cursor cloud agents hold no metal key by policy; this smoke is operator-run.
[metal-smoke] Owner / Dev: from a box with the key, on this branch, exactly:

  export ${BYOK_ENVS[0]:-OPENROUTER_API_KEY}=…        # the topic's miner BYOK (evaluate only; never on argv)
  ./deploy/scripts/proof-metal-smoke.sh --host $HOST --topic $TOPIC --task $TASK --job $JOB \\
      ${ARTIFACT:+--artifact $ARTIFACT }${GUEST_AGENT_LOCAL:+--guest-agent-local $GUEST_AGENT_LOCAL }${PATH_PREPEND:+--path-prepend $PATH_PREPEND }$(printf -- '--set %q ' "${SETS[@]}")

[metal-smoke] Expected: "[smoke] run → done", a JSON summary for exactly one trial named ${TASK}__1,
[metal-smoke] and a "smoke_evidence" block to paste into the PR. See docs/runbooks/proof-experiment-smoke.md.
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
log "topic=$TOPIC runner=$RUNNER pack=$REMOTE_PACK task=$TASK job=$JOB scratch=$HOST:$REMOTE_SCRATCH"

if [[ "$JOB" == "evaluate" ]]; then
  [[ ${#BYOK_ENVS[@]} -gt 0 ]] || { [[ "$BYOK_NAME" != "-" ]] && BYOK_ENVS=("$BYOK_NAME"); }
  for name in "${BYOK_ENVS[@]}"; do
    [[ -n "${!name:-}" ]] || die "export $name in this shell first (the topic's miner BYOK); it is never passed on argv"
  done
fi

# --- 3. Remote preflight: scratch, pack present, python3, docker/harbor hints ---
"$SSH_BIN" "$HOST" "test -d $SCRATCH_ROOT && test ! -L $SCRATCH_ROOT" || die "$SCRATCH_ROOT is not a real directory on $HOST; refusing to create scratch elsewhere"
"$SSH_BIN" "$HOST" "$REMOTE_GUARD && test ! -e \"\$p\"" || die "$REMOTE_SCRATCH already exists on $HOST or resolves outside $SCRATCH_ROOT; pick another --remote-scratch"
"$SSH_BIN" "$HOST" "$REMOTE_GUARD && install -d -m 0700 -- \"\$p\"" || die "cannot create $REMOTE_SCRATCH on $HOST"
REMOTE_TMP="$REMOTE_SCRATCH"
cleanup_remote() {
  if [[ $KEEP -eq 1 ]]; then
    log "remote work kept at $HOST:$REMOTE_TMP"
  else
    # Same guard on the way out: never rm -rf a path the host resolves elsewhere.
    "$SSH_BIN" "$HOST" "$REMOTE_GUARD && rm -rf -- \"\$p\"" || log "warning: could not remove $HOST:$REMOTE_TMP (left in place)"
  fi
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
tar -C "$(dirname "$ADAPTOR_DIR")" --exclude='tests' --exclude='__pycache__' -cf - "$(basename "$ADAPTOR_DIR")" \
  | "$SSH_BIN" "$HOST" "tar -C '$REMOTE_TMP' -xf -"
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
  log "PASS: the guest returned a report for $TOPIC/$TASK (nothing persisted). Paste the smoke_evidence block from $OUT_DIR/smoke.log into the PR."
else
  log "FAIL (rc=$RC): read $OUT_DIR/smoke.log; with --keep the remote work root holds the adaptor's harbor.run.log / tasks-filtered"
fi
exit "$RC"
