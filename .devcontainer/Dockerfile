FROM debian:trixie-slim

ARG USERNAME=dev

# OCCT is only for the `occt` comparison in ryukei-bench; the default
# build never links it. Debian's 7.8 packages put a header the meshing
# code needs (NCollection_AliasedArray.hxx) in the visualization package.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git less pkg-config procps sudo \
        libocct-foundation-dev libocct-modeling-data-dev libocct-modeling-algorithms-dev \
        libocct-visualization-dev \
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
