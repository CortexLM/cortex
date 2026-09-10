#!/bin/bash
# Shared helpers for rlm_fc_in_guest_harbor. Sourced by run / inspect / tests.
# Never print secret values.

_PROOF_HARBOR_ADAPTOR_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROOF_RESOLVE_AGENT="${_PROOF_HARBOR_ADAPTOR_DIR}/resolve_agent.py"
PROOF_RESOLVE_HARNESS="${_PROOF_HARBOR_ADAPTOR_DIR}/resolve_harness.py"
PROOF_FILTER_TASKS="${_PROOF_HARBOR_ADAPTOR_DIR}/harness/filter_tasks.py"
PROOF_REWRITE_NETWORK="${_PROOF_HARBOR_ADAPTOR_DIR}/harness/rewrite_network.py"
PROOF_ENSURE_VERIFIER="${_PROOF_HARBOR_ADAPTOR_DIR}/harness/ensure_verifier.py"
PROOF_PYTHON_AGENT="${_PROOF_HARBOR_ADAPTOR_DIR}/harness/proof_python_agent.py"

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
#
# Evaluate always has a staging dir (guest sets PROOF_MINER_ENV_DIR; if it
# did not, we create one and copy any already-exported value). Fail closed
# only when the key is still missing after that — never because the dir
# env var was unset.
proof_stage_miner_env_dir() {
    if [ -n "${PROOF_MINER_ENV_DIR:-}" ]; then
        mkdir -p "$PROOF_MINER_ENV_DIR"
        chmod 0700 "$PROOF_MINER_ENV_DIR" 2>/dev/null || true
        return 0
    fi
    if [ -n "${PROOF_SECRETS_DIR:-}" ]; then
        export PROOF_MINER_ENV_DIR="$PROOF_SECRETS_DIR/miner"
    else
        : "${PROOF_WORK_DIR:?PROOF_WORK_DIR is required to stage miner BYOK}"
        export PROOF_MINER_ENV_DIR="$PROOF_WORK_DIR/miner-env"
    fi
    mkdir -p "$PROOF_MINER_ENV_DIR"
    chmod 0700 "$PROOF_MINER_ENV_DIR"
}

proof_load_inference_key() {
    local byok_name="${PROOF_PARAM_MINER_BYOK:-}"
    local byok_file=""
    if [ -n "$byok_name" ]; then
        proof_stage_miner_env_dir
        byok_file="$PROOF_MINER_ENV_DIR/$byok_name"
        if [ ! -r "$byok_file" ]; then
            # Indirect expansion: the guest may have exported the value
            # without writing the file. Stage it; never print it.
            if [ -n "${!byok_name:-}" ]; then
                printf '%s' "${!byok_name}" > "$byok_file"
                chmod 0600 "$byok_file"
            fi
        fi
        if [ -r "$byok_file" ]; then
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

# Filter pack tasks to those that typically finish under max_task_duration_s
# (default 3600). Copies into $PROOF_WORK_DIR/tasks-filtered and points
# PROOF_TASKS at that tree. The pack is never mutated.
proof_filter_tasks() {
    : "${PROOF_TASKS:?proof_require_tasks first}"
    : "${PROOF_WORK_DIR:?PROOF_WORK_DIR is required}"
    local dest="$PROOF_WORK_DIR/tasks-filtered"
    local extra=()
    if [ -n "${PROOF_PARAM_TASK_FILTER:-}" ]; then
        extra+=(--filter-rel "$PROOF_PARAM_TASK_FILTER")
    fi
    if [ "${PROOF_PARAM_EXCLUDE_UNKNOWN_DURATION:-}" = "true" ]; then
        extra+=(--drop-unknown)
    fi
    python3 "$PROOF_FILTER_TASKS" \
        --tasks-dir "$PROOF_TASKS" \
        --dest-dir "$dest" \
        --pack-dir "${PROOF_PACK_DIR:?}" \
        --max-duration-s "${PROOF_PARAM_MAX_TASK_DURATION_S:-3600}" \
        ${extra[@]+"${extra[@]}"} \
        || proof_die "task duration filter failed"
    PROOF_TASKS="$dest"
    export PROOF_TASKS
}

# Agents must reach the internet (OpenRouter / BYOK). Harbor Docker env
# rejects network_mode=no-network on this guest; rewrite the filtered copy.
proof_enable_agent_network() {
    : "${PROOF_TASKS:?proof_filter_tasks first}"
    python3 "$PROOF_REWRITE_NETWORK" --tasks-dir "$PROOF_TASKS" --mode public \
        || proof_die "failed to enable agent network on the filtered task copy"
}

# Harbor verifier execs pytest inside the task environment image. n15 x0017
# biped + cad scored 0 because that image had no pytest. Patch the copy.
proof_ensure_verifier() {
    : "${PROOF_TASKS:?proof_filter_tasks first}"
    python3 "$PROOF_ENSURE_VERIFIER" --tasks-dir "$PROOF_TASKS" \
        || proof_die "failed to ensure pytest in verifier/environment images"
}

# Resolve the miner harness. Custom Python is primary; Harbor BaseAgent,
# harness.json, and run.sh are also accepted. Evaluate never falls back to
# terminus-2 when an artefact was staged.
proof_select_harness() {
    local job="${PROOF_JOB:?PROOF_JOB is required}"
    local resolved
    resolved="$(python3 "$PROOF_RESOLVE_HARNESS")" || proof_die "harness resolve failed"
    local kind import_path pythonpath wrapper source entry builtin
    kind="$(printf '%s\n' "$resolved" | sed -n 's/^kind=//p' | head -n1)"
    import_path="$(printf '%s\n' "$resolved" | sed -n 's/^import_path=//p' | head -n1)"
    pythonpath="$(printf '%s\n' "$resolved" | sed -n 's/^pythonpath=//p' | head -n1)"
    wrapper="$(printf '%s\n' "$resolved" | sed -n 's/^wrapper=//p' | head -n1)"
    source="$(printf '%s\n' "$resolved" | sed -n 's/^source=//p' | head -n1)"
    entry="$(printf '%s\n' "$resolved" | sed -n 's/^entry=//p' | head -n1)"
    builtin="$(printf '%s\n' "$resolved" | sed -n 's/^builtin=//p' | head -n1)"
    [ -n "$kind" ] || proof_die "harness resolve printed no kind"

    export PROOF_HARNESS_KIND="$kind"
    export PROOF_HARNESS_ENTRY="$entry"
    export PROOF_HARBOR_AGENT_SOURCE="$source"
    export PROOF_HARNESS_WRAPPER="$wrapper"

    case "$kind" in
        python)
            [ -n "$import_path" ] || proof_die "python harness missing import_path"
            [ -f "$PROOF_PYTHON_AGENT" ] || proof_die "missing custom Python wrapper $PROOF_PYTHON_AGENT"
            # Wrapper lives next to the adaptor; miner code stays the artefact parent.
            export PYTHONPATH="${_PROOF_HARBOR_ADAPTOR_DIR}/harness:${pythonpath}"
            export PROOF_MINER_AGENT_IMPORT="$import_path"
            export PROOF_MINER_AGENT_ROOT="${pythonpath}"
            export PROOF_HARBOR_AGENT_ARG="proof_python_agent:ProofPythonAgent"
            echo "rlm_fc_in_guest_harbor: custom Python harness $import_path -> -a proof_python_agent:ProofPythonAgent" >&2
            ;;
        harbor)
            [ -n "$import_path" ] || proof_die "harbor harness missing import_path"
            export PYTHONPATH="$pythonpath"
            export PROOF_HARBOR_AGENT_ARG="$import_path"
            echo "rlm_fc_in_guest_harbor: miner Harbor agent -> -a $import_path" >&2
            ;;
        script)
            [ -n "$entry" ] || proof_die "script harness missing entry"
            export PROOF_HARBOR_AGENT_ARG=""
            echo "rlm_fc_in_guest_harbor: script harness $entry (not terminus-2)" >&2
            ;;
        builtin)
            [ -n "$builtin" ] || proof_die "builtin harness missing name"
            [ "$job" = evaluate ] && [ "$source" = topic ] && \
                proof_die "evaluate -a must be a miner harness, not built-in $builtin"
            export PROOF_HARBOR_AGENT_ARG="$builtin"
            echo "rlm_fc_in_guest_harbor: $source using built-in -a $builtin" >&2
            ;;
        *)
            proof_die "unknown harness kind $kind"
            ;;
    esac
    printf '%s\n' "${PROOF_HARBOR_AGENT_ARG}"
}

# Prints the Harbor -a argument. Kept for adaptor unit tests.
proof_select_harbor_agent() {
    proof_select_harness
}

# Prefer a live Docker daemon (rootful overlay / host-tools). Fall back to
# the podman API socket only when Docker is absent. Never alias compose.
proof_docker_ok() {
    command -v docker >/dev/null 2>&1 || return 1
    docker info >/dev/null 2>&1
}

proof_start_podman() {
    command -v podman >/dev/null 2>&1 || proof_die "podman is not on PATH (bake --with-podman) and docker is not usable"
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

proof_start_container_runtime() {
    if [ "${PROOF_HARNESS_SKIP_PODMAN:-}" = 1 ] || [ "${PROOF_HARNESS_SKIP_RUNTIME:-}" = 1 ]; then
        return 0
    fi
    # Rootful docker.sock from init / operator overlay. Native overlay, no fuse.
    local sock=""
    if [ -S /var/run/docker.sock ]; then
        sock=/var/run/docker.sock
    elif [ -S /run/docker.sock ]; then
        sock=/run/docker.sock
    fi
    if [ -n "$sock" ]; then
        export DOCKER_HOST="unix://$sock"
        # Socket can exist before the daemon answers; wait rather than
        # immediately falling through to podman (Harbor env start timeouts).
        local i
        for i in $(seq 1 30); do
            if proof_docker_ok; then
                echo "rlm_fc_in_guest_harbor: using docker daemon at $DOCKER_HOST" >&2
                export PROOF_CONTAINER_RUNTIME=docker
                return 0
            fi
            sleep 1
        done
        echo "rlm_fc_in_guest_harbor: docker socket at $sock never became ready" >&2
        unset DOCKER_HOST
    fi
    if [ -n "${DOCKER_HOST:-}" ] && proof_docker_ok; then
        echo "rlm_fc_in_guest_harbor: using docker via DOCKER_HOST=$DOCKER_HOST" >&2
        export PROOF_CONTAINER_RUNTIME=docker
        return 0
    fi
    echo "rlm_fc_in_guest_harbor: docker not ready; falling back to podman API socket" >&2
    proof_start_podman
    export PROOF_CONTAINER_RUNTIME=podman
}

proof_run_script_harness() {
    local art="${PROOF_ARTIFACT_DIR:?script harness requires PROOF_ARTIFACT_DIR}"
    local rel="${PROOF_HARNESS_ENTRY:?}"
    case "$rel" in
        "" | /* | *..*) proof_die "script harness entry must be a relative artefact path: $rel" ;;
    esac
    local path="$art/$rel"
    [ -f "$path" ] || proof_die "script harness missing $path"
    echo "rlm_fc_in_guest_harbor: exec miner script $rel (not wrapping terminus-2)" >&2
    (
        cd "$art"
        case "$path" in
            *.sh) bash "$path" ;;
            *.py) python3 "$path" ;;
            *)
                if [ -x "$path" ]; then
                    "$path"
                else
                    proof_die "script harness $rel is not executable"
                fi
                ;;
        esac
    )
}
