FROM ubuntu:24.04@sha256:fe81560990fae86dc4743202b2cb4d3df98d0bcf949173cecef15ce1127ef852

ARG DEBIAN_FRONTEND=noninteractive
ARG SOURCE_REPOSITORY=https://github.com/DemonBigj781/code-mod
ARG RUST_TOOLCHAIN=1.93.0

ENV CARGO_HOME=/opt/cargo \
    RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:$PATH

# Package revisions intentionally follow the security repositories configured by
# the digest-pinned Ubuntu base image.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        build-essential \
        bzip2 \
        ca-certificates \
        clang \
        cmake \
        curl \
        file \
        git \
        jq \
        keyutils \
        lld \
        make \
        ninja-build \
        patch \
        pkg-config \
        python3 \
        python3-venv \
        rsync \
        sudo \
        tar \
        unzip \
        xz-utils \
        zip \
        zstd \
    && rm -rf /var/lib/apt/lists/*

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN" \
    && rustc --version \
    && cargo --version

LABEL org.opencontainers.image.source="$SOURCE_REPOSITORY"
LABEL org.opencontainers.image.description="Linux x86-64 build environment for Cosmopolitan Code APE artifacts"

WORKDIR /workspace
