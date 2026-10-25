FROM debian:trixie-slim

ARG USERNAME=dev

# OCCT is only for the `occt` comparison in ryukei-bench; the default
# build never links it. Debian's 7.8 packages put a header the meshing
# code needs (NCollection_AliasedArray.hxx) in the visualization package;
# the data exchange package is for reading and writing STEP.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git less pkg-config procps sudo \
        libocct-foundation-dev libocct-modeling-data-dev libocct-modeling-algorithms-dev \
        libocct-visualization-dev libocct-data-exchange-dev libtbb-dev \
    && rm -rf /var/lib/apt/lists/*

# Mesa's software Vulkan (lavapipe): the container has no GPU, and
# ryucad-app's tests draw.
RUN apt-get update \
    && apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1 \
    && rm -rf /var/lib/apt/lists/*

# SMLib, the kernel in NVIDIA's USD BRep (Apache-2.0), is only for the
# `smlib` comparison in ryukei-bench. Its kernel builds on its own (no
# OpenUSD, no Packman): compile it at a pinned commit into
# /usr/local/lib/libsmlib.so with headers in /usr/local/include/smlib.
ARG USD_BREP_COMMIT=c9c5979f04cea8c38213004d4f04df0dcfb40d05
RUN mkdir /tmp/usd-brep && cd /tmp/usd-brep \
    && git init -q && git remote add origin https://github.com/NVIDIA-Omniverse/usd-brep \
    && git fetch -q --depth 1 origin ${USD_BREP_COMMIT} && git checkout -q FETCH_HEAD \
    && mkdir obj && ls source/SMLib/src/*.cpp | xargs -P "$(nproc)" -I{} sh -c \
        'g++ -std=c++17 -O2 -fPIC -w -Isource/SMLib/inc -c {} -o obj/$(basename {} .cpp).o' \
    && g++ -shared -o /usr/local/lib/libsmlib.so obj/*.o -ltbb -ltbbmalloc -Wl,--no-undefined \
    && cp -r source/SMLib/inc /usr/local/include/smlib \
    && mkdir -p /usr/local/share/doc/smlib && cp LICENSE THIRD_PARTY_NOTICES.md /usr/local/share/doc/smlib/ \
    && ldconfig && cd / && rm -rf /tmp/usd-brep

RUN useradd -m -s /bin/bash ${USERNAME} \
    && echo "${USERNAME} ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/${USERNAME}

USER ${USERNAME}
ENV PATH="/home/${USERNAME}/.cargo/bin:${PATH}"

# Install the toolchain pinned in rust-toolchain.toml so the image matches native builds.
COPY --chown=${USERNAME} rust-toolchain.toml /tmp/toolchain/rust-toolchain.toml
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal \
    && cd /tmp/toolchain && rustup toolchain install \
    && mkdir -p /home/${USERNAME}/.cargo/registry /home/${USERNAME}/.cargo/git
