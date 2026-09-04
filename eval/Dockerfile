# syntax=docker/dockerfile:1.7
#
# Proof eval image (contract layer). Cortex pins the *digest* of the scoring
# build (Dockerfile.scoring) in config/proof-pin.toml; a floating tag is
# never the pin. Do not pin this contract-only digest.
#
#   docker build -f eval/Dockerfile --build-arg WITH_RUNTIME=0 -t proof-eval:contract .
#
# No secret, holdout item, teacher endpoint, or Modal reference is baked in.

ARG BASE_IMAGE=python:3.12-slim-bookworm@sha256:0f5b26b9518d002b6173fd61daad821fa340635ebfec5bba471013f9ca114579
FROM ${BASE_IMAGE}

ARG WITH_RUNTIME=0
ARG TORCH_INDEX_URL=""
ARG PROOF_GIT_SHA="unknown"

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    HF_HUB_DISABLE_TELEMETRY=1 \
    TRANSFORMERS_VERBOSITY=error \
    TOKENIZERS_PARALLELISM=false \
    PROOF_GIT_SHA=${PROOF_GIT_SHA}

RUN set -eux; \
    apt-get update; \
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates openssh-server tini iproute2; \
    rm -rf /var/lib/apt/lists/*; \
    mkdir -p /run/sshd /root/.ssh /opt/proof-eval/baselines; \
    chmod 700 /root/.ssh

WORKDIR /opt/proof-eval
COPY eval/pyproject.toml ./
COPY eval/src ./src
COPY eval/baselines ./baselines
COPY eval/install-cli.sh /tmp/install-cli.sh
COPY eval/bin/proof-eval /tmp/proof-eval-launcher
COPY eval/src/proof_eval/baked_proxies.json /opt/proof-eval/baked_proxies.json
COPY eval/baselines/adamw.py /opt/proof-eval/baselines/adamw.py

RUN set -eux; \
    if command -v pip >/dev/null 2>&1; then pip_cmd="pip"; else pip_cmd="python3 -m pip"; fi; \
    if [ -n "${TORCH_INDEX_URL}" ]; then \
        $pip_cmd install --extra-index-url "${TORCH_INDEX_URL}" ".[runtime]"; \
    elif [ "${WITH_RUNTIME}" = "1" ]; then \
        $pip_cmd install ".[runtime]"; \
    else \
        $pip_cmd install "."; \
    fi; \
    sh /tmp/install-cli.sh; \
    rm -f /tmp/install-cli.sh /tmp/proof-eval-launcher

COPY eval/bin/proof-eval /usr/bin/proof-eval
COPY eval/entrypoint.sh /usr/bin/proof-eval-entrypoint
RUN set -eux; \
    chmod 0755 /usr/bin/proof-eval /usr/bin/proof-eval-entrypoint; \
    test -f /usr/bin/proof-eval; \
    test ! -L /usr/bin/proof-eval; \
    test -x /usr/bin/proof-eval; \
    env -i PATH=/usr/bin:/bin /usr/bin/proof-eval --help >/dev/null

WORKDIR /tmp/proof_eval
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/bin/proof-eval-entrypoint"]
CMD ["serve"]
