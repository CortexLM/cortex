# Official Python manifest verified on 2026-09-17; override only with another digest.
ARG PYTHON_IMAGE=python:3.12-slim-bookworm@sha256:782412e85d0f0984994c290652577d4018aff08145c85b262bb63dc0c7522254
FROM ${PYTHON_IMAGE} AS builder

ARG PYTHON_IMAGE
RUN python -c 'import re, sys; assert re.fullmatch(r"(?:docker.io/library/)?python:3\.12(?:\.[0-9]+)?-slim-bookworm@sha256:[0-9a-f]{64}", sys.argv[1]), "digest-pinned Python 3.12 slim Bookworm image required"' "$PYTHON_IMAGE"
ENV UV_LINK_MODE=copy UV_NO_PROGRESS=1 UV_PYTHON_DOWNLOADS=never
RUN pip install --no-cache-dir uv==0.11.17
WORKDIR /build
COPY pyproject.toml uv.lock ./
RUN uv sync --locked --no-dev --group build --no-install-project
COPY src ./src
RUN uv build --wheel --no-build-isolation

FROM builder AS guest-builder
RUN uv export --quiet --locked --no-dev --no-emit-project --output-file /guest.txt \
    && uv venv /opt/cortex \
    && uv pip install --python /opt/cortex/bin/python --require-hashes -r /guest.txt \
    && uv pip install --python /opt/cortex/bin/python --no-deps dist/*.whl

FROM builder AS runtime-builder
RUN uv export --quiet --locked --no-dev --extra chain --no-emit-project --output-file /runtime.txt \
    && uv venv /opt/cortex \
    && uv pip install --python /opt/cortex/bin/python --require-hashes -r /runtime.txt \
    && uv pip install --python /opt/cortex/bin/python --no-deps dist/*.whl

# Export this stage as an ext4 rootfs; it is never the master runtime.
FROM ${PYTHON_IMAGE} AS guest
# Security updates that landed after the base digest was pinned; the publish
# workflow refuses any fixable high or critical finding.
RUN apt-get update \
    && apt-get upgrade -y --no-install-recommends \
    && apt-get install -y --no-install-recommends ca-certificates libsodium23 \
       libseccomp2 bubblewrap util-linux iproute2 tini \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /workspace /proc /sys /dev /run /tmp /etc/proof /opt/proof/runners
COPY --from=guest-builder /opt/cortex /opt/cortex
RUN ln -sf /opt/cortex/bin/proof-guest-init /sbin/init \
    && ln -sf /usr/local/bin/python3 /usr/bin/python3
ENV PATH=/opt/cortex/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin \
    PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
ENTRYPOINT ["/sbin/init"]

FROM ${PYTHON_IMAGE} AS runtime
RUN apt-get update \
    && apt-get upgrade -y --no-install-recommends \
    && apt-get install -y --no-install-recommends ca-certificates libsodium23 openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 cortex \
    && useradd --uid 65532 --gid cortex --home-dir /home/cortex --create-home cortex \
    && install -d -o cortex -g cortex -m 0700 /var/lib/cortex /var/lib/base
COPY --from=runtime-builder /opt/cortex /opt/cortex
# Bittensor's READ_ONLY disables implicit ~/.bittensor directory/log creation.
ENV PATH=/opt/cortex/bin:$PATH PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 READ_ONLY=1
USER 65532:65532
WORKDIR /var/lib/cortex
ENTRYPOINT ["cortex"]
CMD ["--help"]
