# syntax=docker/dockerfile:1.7
# Multi-stage Rust build for base service binaries
#
# Source build (default, CI/prod):
#   docker build -f deploy/Dockerfile --target validator -t validator:0.1.0 .
#
# Fast local path (host already ran `cargo build --release -p …`):
#   docker build -f deploy/Dockerfile --target validator \
#     --build-arg BUILD_FROM=prebuilt -t validator:0.1.0 .
#
# Digests only for base images — never floating tags.

ARG DEBIAN_IMAGE=debian@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818
ARG RUST_IMAGE=rust@sha256:e51d0265072d2d9d5d320f6a44dde6b9ef13653b035098febd68cce8fa7c0bc4
ARG BUILD_FROM=source

# -----------------------------------------------------------------------------
# Toolchain: workspace pin is Rust 1.96.0 (rust-toolchain.toml)
# -----------------------------------------------------------------------------
FROM ${RUST_IMAGE} AS rust-base
RUN rustup toolchain install 1.96.0 --profile minimal \
    && rustup default 1.96.0 \
    && rustc --version
WORKDIR /src

# -----------------------------------------------------------------------------
# Source builder — compiles all service bins once
# -----------------------------------------------------------------------------
FROM rust-base AS source-builder
COPY Cargo.toml Cargo.lock rust-toolchain.toml clippy.toml ./
COPY crates ./crates
COPY bins ./bins
COPY xtask ./xtask
# Keep workspace metadata members that Docker context needs
COPY config ./config
# validator-bin/dcap turns on real Intel DCAP quote verification; without it the
# validator parks every attestation instead of verifying it.
RUN cargo build --release \
      --features validator-bin/dcap \
      -p validator-bin \
      -p gateway-bin \
      -p updater-bin \
      -p relearn-challenge-bin \
      -p relearn-t2i-challenge-bin \
      -p relearn-agent-challenge-bin \
      -p relearn-mm-challenge-bin \
      -p bounty-challenge-bin \
      -p proof-challenge-bin \
 && strip target/release/validator \
           target/release/gateway \
           target/release/updater \
           target/release/relearn-challenge \
           target/release/relearn-t2i-challenge \
           target/release/relearn-agent-challenge \
           target/release/relearn-mm-challenge \
           target/release/bounty-challenge \
           target/release/proof-challenge \
 && mkdir -p /out \
 && cp target/release/validator \
       target/release/gateway \
       target/release/updater \
       target/release/relearn-challenge \
       target/release/relearn-t2i-challenge \
       target/release/relearn-agent-challenge \
       target/release/relearn-mm-challenge \
       target/release/bounty-challenge \
       target/release/proof-challenge /out/

# -----------------------------------------------------------------------------
# Prebuilt path — copy host target/release artifacts (local verify only)
# -----------------------------------------------------------------------------
FROM ${DEBIAN_IMAGE} AS prebuilt-builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /out
COPY target/release/validator \
     target/release/gateway \
     target/release/updater \
     target/release/relearn-challenge \
     target/release/relearn-t2i-challenge \
     target/release/relearn-agent-challenge \
     target/release/relearn-mm-challenge \
     target/release/bounty-challenge \
     target/release/proof-challenge \
     /out/

# -----------------------------------------------------------------------------
# Select artifact source
# -----------------------------------------------------------------------------
FROM source-builder AS artifacts-source
FROM prebuilt-builder AS artifacts-prebuilt

FROM artifacts-${BUILD_FROM} AS artifacts

# -----------------------------------------------------------------------------
# Runtime base (non-root)
# -----------------------------------------------------------------------------
FROM ${DEBIAN_IMAGE} AS runtime-base
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 65532 base \
    && useradd --system --uid 65532 --gid base --home-dir /home/base --create-home base \
    && mkdir -p /var/lib/base \
    && chown -R base:base /var/lib/base
USER base
WORKDIR /home/base

FROM runtime-base AS validator
COPY --from=artifacts --chown=base:base /out/validator /usr/local/bin/validator
# Owner-signed trust root for /v1/attest allowlist (todo 34 staging Verified path).
COPY --chown=base:base config /etc/base/config
ENV BASE_TRUST_ROOT_DIR=/etc/base/config
ENTRYPOINT ["/usr/local/bin/validator"]

FROM runtime-base AS gateway
COPY --from=artifacts --chown=base:base /out/gateway /usr/local/bin/gateway
ENTRYPOINT ["/usr/local/bin/gateway"]

FROM runtime-base AS updater
COPY --from=artifacts --chown=base:base /out/updater /usr/local/bin/updater
ENTRYPOINT ["/usr/local/bin/updater"]

FROM runtime-base AS relearn-challenge
# Lium eval uses operator SSH; runtime-base has no ssh client.
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/relearn \
    && chown -R base:base /var/lib/relearn
USER base
COPY --from=artifacts --chown=base:base /out/relearn-challenge /usr/local/bin/relearn-challenge
ENTRYPOINT ["/usr/local/bin/relearn-challenge"]

FROM runtime-base AS relearn-t2i-challenge
# Cosmos3 generation and Q-Judger scoring both run on rented GPUs over SSH.
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/relearn-t2i \
    && chown -R base:base /var/lib/relearn-t2i
USER base
COPY --from=artifacts --chown=base:base /out/relearn-t2i-challenge /usr/local/bin/relearn-t2i-challenge
ENTRYPOINT ["/usr/local/bin/relearn-t2i-challenge"]

FROM runtime-base AS relearn-agent-challenge
# Episode replay and both ablation arms run on rented GPUs over SSH.
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/relearn-agent \
    && chown -R base:base /var/lib/relearn-agent
USER base
COPY --from=artifacts --chown=base:base /out/relearn-agent-challenge /usr/local/bin/relearn-agent-challenge
ENTRYPOINT ["/usr/local/bin/relearn-agent-challenge"]

FROM runtime-base AS relearn-mm-challenge
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/relearn-mm \
    && chown -R base:base /var/lib/relearn-mm
USER base
COPY --from=artifacts --chown=base:base /out/relearn-mm-challenge /usr/local/bin/relearn-mm-challenge
ENTRYPOINT ["/usr/local/bin/relearn-mm-challenge"]

FROM runtime-base AS bounty-challenge
USER root
RUN mkdir -p /var/lib/bounty \
    && chown -R base:base /var/lib/bounty
USER base
COPY --from=artifacts --chown=base:base /out/bounty-challenge /usr/local/bin/bounty-challenge
ENTRYPOINT ["/usr/local/bin/bounty-challenge"]

FROM runtime-base AS proof-challenge
# RLM judge + harness run on rented GPUs over SSH.
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/proof \
    && chown -R base:base /var/lib/proof
USER base
COPY --from=artifacts --chown=base:base /out/proof-challenge /usr/local/bin/proof-challenge
ENTRYPOINT ["/usr/local/bin/proof-challenge"]
