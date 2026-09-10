#!/bin/bash
# Shared helpers for rlm_fc_in_guest_harbor. Sourced by run / inspect / tests.
# Never print secret values.

_PROOF_HARBOR_ADAPTOR_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROOF_RESOLVE_AGENT="${_PROOF_HARBOR_ADAPTOR_DIR}/resolve_agent.py"

proof_die() {
    echo "rlm_fc_in_guest_harbor: $*" >&2
    exit 2
}

proof_relative_ok() {
    case "$1" in
        "" | /* | *..*) return 1 ;;
        *) return 0 ;;
    esac
}

proof_require_tasks() {
    : "${PROOF_PACK_DIR:?PROOF_PACK_DIR is required}"
    local tasks_rel="${PROOF_PARAM_TASKS_DIR:?constraints.params.tasks_dir is required}"
    proof_relative_ok "$tasks_rel" || proof_die "tasks_dir must be a plain relative path (no / or ..): $tasks_rel"
    PROOF_TASKS="$PROOF_PACK_DIR/$tasks_rel"
    [ -d "$PROOF_TASKS" ] || proof_die "no tasks directory $PROOF_TASKS in the staged pack"
}

# Writable scratch on the experiment disk (rootfs is read-only).
proof_writable_scratch() {
    : "${PROOF_WORK_DIR:?PROOF_WORK_DIR is required}"
    export TMPDIR="${PROOF_WORK_DIR}/tmp"
    export TMP="$TMPDIR"
    export TEMP="$TMPDIR"
    mkdir -p "$TMPDIR" "$PROOF_WORK_DIR/cache" "$PROOF_WORK_DIR/var-tmp" "$PROOF_WORK_DIR/home"
    export XDG_CACHE_HOME="${PROOF_WORK_DIR}/cache"
    if [ ! -w /var/tmp ] 2>/dev/null; then
        echo "rlm_fc_in_guest_harbor: /var/tmp is not writable (read-only rootfs); using TMPDIR=$TMPDIR" >&2
    fi
}

# Miner BYOK wins on evaluate. Owner key is only for baseline / operator-paid
# jobs when miner_byok is unset (or baseline with no miner file staged).
proof_load_inference_key() {
    local byok_name="${PROOF_PARAM_MINER_BYOK:-}"
    local byok_file=""
    if [ -n "$byok_name" ]; then
        [ -n "${PROOF_MINER_ENV_DIR:-}" ] || {
            [ "${PROOF_JOB:-}" = evaluate ] && proof_die "evaluate: miner_byok=$byok_name but PROOF_MINER_ENV_DIR is unset"
        }
        if [ -n "${PROOF_MINER_ENV_DIR:-}" ]; then
            byok_file="$PROOF_MINER_ENV_DIR/$byok_name"
        fi
        if [ -n "$byok_file" ] && [ -r "$byok_file" ]; then
            export "$byok_name"="$(tr -d '\n' < "$byok_file")"
            return 0
        fi
        if [ "${PROOF_JOB:-}" = evaluate ]; then
            proof_die "evaluate: miner did not supply $byok_name at $byok_file; refusing owner-key fallback"
        fi
        # baseline: fall through to owner key when the miner file is absent.
    fi
    if [ -n "${PROOF_PARAM_INFERENCE_KEY_FILE:-}" ]; then
        : "${PROOF_PARAM_INFERENCE_KEY_ENV:?inference_key_env is required with inference_key_file}"
        : "${PROOF_SECRETS_DIR:?PROOF_SECRETS_DIR is required to read the owner key}"
        local owner_file="$PROOF_SECRETS_DIR/$PROOF_PARAM_INFERENCE_KEY_FILE"
        [ -r "$owner_file" ] || proof_die "owner key file not readable: $PROOF_PARAM_INFERENCE_KEY_FILE"
        export "$PROOF_PARAM_INFERENCE_KEY_ENV"="$(tr -d '\n' < "$owner_file")"
    fi
}

proof_is_harbor_agent_dir() {
    local d="${1:-}"
    [ -n "$d" ] && [ -d "$d" ] || return 1
    python3 "$PROOF_RESOLVE_AGENT" --dir "$d" --check
}

# Prints the Harbor -a argument. Exports PROOF_HARBOR_AGENT_SOURCE and
# PYTHONPATH when the artefact supplies a custom agent. Never falls back to
# the topic built-in on evaluate when an artefact was staged.
proof_select_harbor_agent() {
    local job="${PROOF_JOB:?PROOF_JOB is required}"
    local art="${PROOF_ARTIFACT_DIR:-}"
    local chosen=""
    local source=""
    local resolved=""

    _use_artefact_agent() {
        local dir="$1"
        local label="$2"
        resolved="$(python3 "$PROOF_RESOLVE_AGENT" --dir "$dir")" || return 1
        local import_path pythonpath
        import_path="$(printf '%s\n' "$resolved" | sed -n 's/^import_path=//p' | head -n1)"
        pythonpath="$(printf '%s\n' "$resolved" | sed -n 's/^pythonpath=//p' | head -n1)"
        [ -n "$import_path" ] || return 1
        if [ -n "$pythonpath" ]; then
            # Evaluate import env is the staged artefact parent only.
            # Inherited PYTHONPATH would let a miner import_path name a
            # module that lives outside the artefact.
            export PYTHONPATH="$pythonpath"
        fi
        chosen="$import_path"
        source="$label"
        echo "rlm_fc_in_guest_harbor: using miner Harbor agent at $dir -> -a $import_path" >&2
        return 0
    }

    if [ -n "$art" ]; then
        if proof_is_harbor_agent_dir "$art/agent"; then
            _use_artefact_agent "$art/agent" "artifact_dir/agent" || proof_die "failed to resolve $art/agent"
        elif proof_is_harbor_agent_dir "$art/recipe/agent"; then
            _use_artefact_agent "$art/recipe/agent" "artifact_dir/recipe/agent" || proof_die "failed to resolve $art/recipe/agent"
        elif [ "$job" = evaluate ]; then
            if [ -f "$art/recipe/run.sh" ]; then
                proof_die "evaluate requires a Harbor agent directory at \$PROOF_ARTIFACT_DIR/agent or \$PROOF_ARTIFACT_DIR/recipe/agent (Harbor -a takes module:Class, not a path). recipe/run.sh is a classic marker only — refusing to ignore the staged artefact or wrap it as terminus-2"
            fi
            proof_die "evaluate staged an artefact at $art but found no Harbor agent directory at agent/ or recipe/agent; refusing topic agent fallback (that was the scoring gap)"
        fi
    elif [ "$job" = evaluate ]; then
        proof_die "evaluate requires PROOF_ARTIFACT_DIR"
    fi

    if [ -z "$chosen" ]; then
        if [ -n "$art" ]; then
            proof_die "artefact staged at $art has no Harbor agent at agent/ or recipe/agent; topic agent fallback is only for baseline with PROOF_ARTIFACT_DIR unset"
        fi
        [ "$job" = evaluate ] && proof_die "evaluate requires a miner Harbor agent (module:Class)"
        : "${PROOF_PARAM_HARBOR_AGENT:?constraints.params.harbor_agent is required for baseline without a miner agent dir}"
        chosen="$PROOF_PARAM_HARBOR_AGENT"
        source="topic"
        echo "rlm_fc_in_guest_harbor: baseline using topic agent -a $chosen" >&2
    fi

    if [ "$job" = evaluate ] && [[ "$chosen" != *:* ]]; then
        proof_die "evaluate -a must be a miner import path (module:Class), not built-in $chosen"
    fi

    export PROOF_HARBOR_AGENT_SOURCE="$source"
    export PROOF_HARBOR_AGENT_ARG="$chosen"
    printf '%s\n' "$chosen"
}

proof_start_podman() {
    if [ "${PROOF_HARNESS_SKIP_PODMAN:-}" = 1 ]; then
        return 0
    fi
    command -v podman >/dev/null 2>&1 || proof_die "podman is not on PATH (bake --with-podman)"
    export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
    mkdir -p "$XDG_RUNTIME_DIR/podman"
    local sock="$XDG_RUNTIME_DIR/podman/podman.sock"
    podman system service --time=0 "unix://$sock" > "${PROOF_WORK_DIR}/podman-service.log" 2>&1 &
    PROOF_PODMAN_PID=$!
    trap 'kill "$PROOF_PODMAN_PID" 2>/dev/null || true' EXIT
    local i
    for i in $(seq 1 50); do
        if [ -S "$sock" ]; then
            export DOCKER_HOST="unix://$sock"
            return 0
        fi
        sleep 0.1
    done
    proof_die "podman API socket did not appear at $sock"
}
