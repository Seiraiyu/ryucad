FROM debian:trixie-slim

ARG USERNAME=dev

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git less pkg-config procps sudo \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/bash ${USERNAME} \
    && echo "${USERNAME} ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/${USERNAME}

USER ${USERNAME}
ENV PATH="/home/${USERNAME}/.cargo/bin:${PATH}"

# Install the toolchain pinned in rust-toolchain.toml so the image matches native builds.
COPY --chown=${USERNAME} rust-toolchain.toml /tmp/toolchain/rust-toolchain.toml
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal \
    && cd /tmp/toolchain && rustup toolchain install \
    && mkdir -p /home/${USERNAME}/.cargo/registry /home/${USERNAME}/.cargo/git
