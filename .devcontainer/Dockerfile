FROM debian:trixie-slim

ARG USERNAME=dev

# OCCT is only for the `occt` comparison in ryukei-bench; the default
# build never links it. Debian's 7.8 packages put a header the meshing
# code needs (NCollection_AliasedArray.hxx) in the visualization package;
# the data exchange package is for reading and writing STEP.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git less pkg-config procps sudo unzip \
        libocct-foundation-dev libocct-modeling-data-dev libocct-modeling-algorithms-dev \
        libocct-visualization-dev libocct-data-exchange-dev libtbb-dev \
    && rm -rf /var/lib/apt/lists/*

# Mesa's software Vulkan (lavapipe): the container has no GPU, and
# ryucad-app's tests draw.
RUN apt-get update \
    && apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1 \
    && rm -rf /var/lib/apt/lists/*

# KiCad 9.0.8 (trixie-backports), only to export ryudfm's ODB++ corpus
# (scripts/fetch-dfm-corpus.sh; crates/ryudfm/tests/corpus/pinned.txt names
# the version). Backports are read from a fixed snapshot.debian.org date so
# the version can't move or vanish; without -t, only kicad comes from
# backports and its dependencies from trixie (with -t, backports' newer
# libcurl blocks later installs). Lato is the font two pinned boards' text
# is set in: without it KiCad substitutes another and their silkscreen
# changes.
RUN echo 'deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/20261028T000000Z trixie-backports main' \
        > /etc/apt/sources.list.d/kicad-backports.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends kicad=9.0.8+dfsg-1~bpo13+1 fonts-lato=2.015-1 \
    && rm -rf /var/lib/apt/lists/* \
    && kicad-cli version

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

# lib3mf, the 3MF Consortium's reference library (BSD-2-Clause), is only
# for ryukei-bench's `formats` references, which read every 3MF ryucad
# writes. Debian packages 1.8, older than 3MF core 1.3, so build a pinned
# release into /usr/local: the library in lib, its C++ binding headers in
# include/lib3mf.
ARG LIB3MF_VERSION=2.5.0
ARG LIB3MF_SHA256=46914f7c7a82ee0839fdd09b339daf621540d5c8d8904b2a0449fd67353dd7cb
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir /tmp/lib3mf && cd /tmp/lib3mf \
    && curl -sSfLo src.zip https://github.com/3MFConsortium/lib3mf/releases/download/v${LIB3MF_VERSION}/lib3mf-${LIB3MF_VERSION}-source-with-submodules.zip \
    && echo "${LIB3MF_SHA256}  src.zip" | sha256sum -c - \
    && unzip -q src.zip \
    && cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DLIB3MF_TESTS=OFF -DCMAKE_INSTALL_PREFIX=/usr/local >/dev/null \
    && cmake --build build -j"$(nproc)" >/dev/null \
    && cmake --install build >/dev/null \
    && mkdir -p /usr/local/include/lib3mf /usr/local/share/doc/lib3mf \
    && cp /usr/local/include/Bindings/Cpp/*.hpp /usr/local/include/lib3mf/ \
    && rm -rf /usr/local/include/Bindings \
    && cp LICENSE /usr/local/share/doc/lib3mf/ \
    && ldconfig && cd / && rm -rf /tmp/lib3mf

# OpenUSD (PyPI's usd-core) and NVIDIA's usd-brep with its BrepArray
# validator are only for ryukei-bench's `usd` references, which open,
# validate and import every USD file ryucad writes into SMLib. usd-brep's
# wheels are CPython 3.12 only and trixie ships 3.13, so a pinned uv
# installs CPython 3.12 and the pinned packages into /opt/usd. usd-brep
# 1.0.1's omniSolid schema is SMLib's above (1.0.0's), unchanged. Its
# validator (usd-validation-nvidia 1.23) takes usd-core below 26.9.
ARG UV_VERSION=0.13.6
ARG USD_CORE_VERSION=26.8
ARG USD_VALIDATION_VERSION=1.23.0
ARG USD_BREP_VERSION=1.0.1
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3-venv \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -m venv /opt/uv \
    && /opt/uv/bin/pip install -q uv==${UV_VERSION} \
    && UV_PYTHON_INSTALL_DIR=/opt/uv/python /opt/uv/bin/uv venv -q -p 3.12 /opt/usd \
    && /opt/uv/bin/uv pip install -q --python /opt/usd \
        usd-core==${USD_CORE_VERSION} "usd-brep[validation]==${USD_BREP_VERSION}" \
        "usd-validation-nvidia[usd]==${USD_VALIDATION_VERSION}" \
    && /opt/usd/bin/python -c "import usd_brep, pxr, brep_validator" \
    && chmod -R a+rX /opt/uv /opt/usd

RUN useradd -m -s /bin/bash ${USERNAME} \
    && echo "${USERNAME} ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/${USERNAME}

USER ${USERNAME}
ENV PATH="/home/${USERNAME}/.cargo/bin:${PATH}"

# Install the toolchain pinned in rust-toolchain.toml so the image matches native builds.
COPY --chown=${USERNAME} rust-toolchain.toml /tmp/toolchain/rust-toolchain.toml
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal \
    && cd /tmp/toolchain && rustup toolchain install \
    && mkdir -p /home/${USERNAME}/.cargo/registry /home/${USERNAME}/.cargo/git
