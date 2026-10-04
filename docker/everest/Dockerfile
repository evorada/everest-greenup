# syntax=docker/dockerfile:1
# EVerest for the Pi 3 (arm64) with Rust module support (docs/plan.md, "Prepare", step 3).
#
#   build:   upstream EVerest at a pinned SHA on the upstream build-kit image, configured
#            with EVEREST_ENABLE_RS_SUPPORT=ON and only the modules we need, plus our
#            RsGreenUpEvse module and its crates copied into the tree.
#   runtime: Debian 12 slim (same glibc as the build-kit) with /opt/everest and our configs;
#            the build fails if a binary misses a shared library or `manager --check` rejects
#            the spike config.
#
# Build from the repo root:
#   docker build --platform linux/arm64 -f docker/everest/Dockerfile -t everest-greenup .

ARG BUILD_KIT_TAG=v1.6.0
ARG EVEREST_REPO=https://github.com/EVerest/everest-core.git
# upstream main, 2026-10-02. Move to the 2026.10.0 tag once it exists.
ARG EVEREST_REF=4afd42e07ea500f80ae485e5f3aa5340c887ae01
# Upstream's Rust pin (MODULE.bazel); everestrs needs >= 1.86.
ARG RUST_VERSION=1.86.0
# Must match the `cxx` crate version in lib/everest/framework/everestrs/everestrs/Cargo.toml,
# otherwise CMake builds its own copy at configure time.
ARG CXXBRIDGE_VERSION=1.0.194
ARG CMAKE_VERSION=3.31.10
ARG EVEREST_MODULES="API;Auth;DummyTokenProvider;DummyTokenProviderManual;DummyTokenValidator;EnergyManager;EnergyNode;RsExample;RsExampleUser;RsGreenUpEvse;Store"
# Libraries no module above needs. ocpp_module_common does not configure without the OCPP
# modules (it links everest::ocpp_conversions), and the rest only cost build time.
# Drop ocpp and ocpp_module_common from this list when OCPP201 joins EVEREST_MODULES.
ARG EVEREST_EXCLUDE_LIBS="ocpp;ocpp_module_common;iso15118;cbv2g;slac;ieee2030_1_1;can_dpm1000"
# gcc 12 at -O3 on arm64 reports false positives in std::vector code (lib/everest/io/src/mdns/mdns.cpp,
# -Warray-bounds), and lib/everest/io builds with -Werror. Upstream CI never sets a build type, so
# it does not see them. Keep the warnings, stop them from failing the build.
ARG CXX_WARNING_FLAGS="-Wno-error=array-bounds -Wno-error=stringop-overflow -Wno-error=stringop-overread -Wno-error=maybe-uninitialized -Wno-error=restrict"

FROM ghcr.io/everest/everest-ci/build-kit-base:${BUILD_KIT_TAG} AS build
ARG EVEREST_REPO
ARG EVEREST_REF
ARG RUST_VERSION
ARG CXXBRIDGE_VERSION
ARG CMAKE_VERSION
ARG EVEREST_MODULES
ARG EVEREST_EXCLUDE_LIBS
ARG CXX_WARNING_FLAGS
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH} \
    CPM_SOURCE_CACHE=/cache/cpm \
    CCACHE_DIR=/cache/ccache

# sdbus-c++ needs libsystemd-dev (same as upstream's .ci/build-kit/docker/Dockerfile).
# The Rust module glue in cmake/everest-generate.cmake uses $<LIST:JOIN,...>, which needs
# CMake >= 3.27; the build-kit ships 3.25, so a newer 3.x goes in front of it on PATH.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libsystemd-dev \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -m pip install --break-system-packages --no-cache-dir "cmake==${CMAKE_VERSION}" \
    && cmake --version

RUN curl -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" --component rustfmt \
    && cargo install cxxbridge-cmd --version "${CXXBRIDGE_VERSION}" \
    && rm -rf "${CARGO_HOME}/registry"

WORKDIR /workspace
RUN git init -q source \
    && cd source \
    && git remote add origin "${EVEREST_REPO}" \
    && git fetch -q --depth 1 origin "${EVEREST_REF}" \
    && git checkout -q FETCH_HEAD

# Local fixes to upstream, kept as patches so they are easy to upstream and to drop (see README).
COPY docker/everest/patches /workspace/patches
RUN cd source && for p in /workspace/patches/*.patch; do git apply --verbose "$p"; done

# Our module goes into the tree like any other; its crates come along as a workspace of
# their own so their `workspace = true` keys resolve. The module's relative crate paths are
# rewritten to that location.
COPY Cargo.toml Cargo.lock /workspace/source/greenup/
COPY crates /workspace/source/greenup/crates
COPY bins /workspace/source/greenup/bins
COPY module/RsGreenUpEvse /workspace/source/modules/RsGreenUpEvse
RUN sed -i 's#path = "../../crates/#path = "/workspace/source/greenup/crates/#'       /workspace/source/modules/RsGreenUpEvse/Cargo.toml     && echo 'ev_add_module(RsGreenUpEvse)' >> /workspace/source/modules/CMakeLists.txt

RUN --mount=type=cache,target=/cache/cpm \
    --mount=type=cache,target=/cache/ccache \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cmake -S source -B build -G Ninja \
      -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_CXX_FLAGS="${CXX_WARNING_FLAGS}" \
      -DCMAKE_INSTALL_PREFIX=/opt/everest \
      -DEVC_ENABLE_CCACHE=ON \
      -DEVEREST_ENABLE_RS_SUPPORT=ON \
      -DEVEREST_ENABLE_PY_SUPPORT=OFF \
      -DEVEREST_ENABLE_JS_SUPPORT=OFF \
      -DEVEREST_INCLUDE_MODULES="${EVEREST_MODULES}" \
      -DEVEREST_EXCLUDE_LIBS="${EVEREST_EXCLUDE_LIBS}" \
      -DEVEREST_BUILD_APPLICATIONS=OFF \
      -DEVEREST_ENABLE_RUN_SCRIPT_GENERATION=OFF \
      -DISO15118_2_GENERATE_AND_INSTALL_CERTIFICATES=OFF \
      -DBUILD_TESTING=OFF \
      2>&1 | tee /workspace/configure.log

# EVerest generates the Rust workspace at configure time without a lockfile, so
# cargo resolves every dependency fresh; the MSRV-aware fallback keeps it from
# picking releases that need a newer rustc than RUST_VERSION (uuid 1.27 wants 1.89).
ENV CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback
RUN --mount=type=cache,target=/cache/cpm \
    --mount=type=cache,target=/cache/ccache \
    --mount=type=cache,target=/usr/local/cargo/registry \
    ninja -C build \
    && ninja -C build install/strip \
    && ls -R /opt/everest/libexec/everest/modules | head -50

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      ca-certificates \
      libboost-chrono1.74.0 \
      libboost-filesystem1.74.0 \
      libboost-log1.74.0 \
      libboost-program-options1.74.0 \
      libboost-thread1.74.0 \
      libcap2 \
      libcurl4 \
      libsqlite3-0 \
      libssl3 \
      libsystemd0 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /opt/everest /opt/everest
COPY config/*.yaml /opt/everest/etc/everest/
ENV PATH=/opt/everest/bin:${PATH}
# The installed binaries carry no RPATH; libframework and friends live in /opt/everest/lib.
RUN echo /opt/everest/lib > /etc/ld.so.conf.d/everest.conf && ldconfig

# Every ELF file must resolve its shared libraries.
RUN missing=$(find /opt/everest -type f \( -perm -u+x -o -name '*.so*' \) -exec ldd {} \; 2>/dev/null \
      | grep 'not found' | sort -u); \
    if [ -n "$missing" ]; then echo "missing shared libraries:"; echo "$missing"; exit 1; fi

RUN manager --check --config /opt/everest/etc/everest/config-spike-rust.yaml \
    && manager --check --config /opt/everest/etc/everest/config-greenup-sil.yaml \
    && test -x /opt/everest/libexec/everest/modules/RsGreenUpEvse/RsGreenUpEvse

ENTRYPOINT ["/opt/everest/bin/manager"]
