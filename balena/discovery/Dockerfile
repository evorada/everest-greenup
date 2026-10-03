# syntax=docker/dockerfile:1
# Discovery release for the Legrand Green'Up 058049 (docs/plan.md, "Prepare", step 2).
#
# One image, run twice (side1, side2). It carries:
#   - picocom + trace.sh for hand-driven sessions with a board, logged to /data/traces;
#   - greenup-link (BolzaW, MIT) built from a pinned commit with a small patch so the
#     serial port, the HTTP address and the FM forcing come from the environment.
#
# Build on the Mac:   docker build --platform linux/arm64 -t greenup-discovery .
# Push to the fleet:  balena push gaia_charge/greenup --source balena/discovery

ARG GREENUP_LINK_REPO=https://github.com/BolzaW/greenup-link.git
ARG GREENUP_LINK_REF=edf4cc6e598e3824db670b8db4b76b173504f917

FROM rust:1-bookworm AS build
ARG GREENUP_LINK_REPO
ARG GREENUP_LINK_REF
WORKDIR /src
RUN git init -q . \
    && git remote add origin "${GREENUP_LINK_REPO}" \
    && git fetch -q --depth 1 origin "${GREENUP_LINK_REF}" \
    && git checkout -q FETCH_HEAD
COPY greenup-link-env.patch /tmp/greenup-link-env.patch
RUN git apply --verbose /tmp/greenup-link-env.patch
# No BuildKit cache mounts here: the Balena builders must be able to build this file.
RUN cargo build --release --locked \
    && install -m 0755 target/release/greenup-link /usr/local/bin/greenup-link

FROM balenalib/raspberrypi3-64-ubuntu:noble-run
RUN install_packages picocom usbutils socat curl jq less
# udev inside the container populates /dev/serial/by-id (gate G7).
ENV UDEV=1
ENV GREENUP_HTTP_ADDR=0.0.0.0:8080
COPY --from=build /usr/local/bin/greenup-link /usr/local/bin/greenup-link
COPY run-discovery.sh trace.sh /usr/local/bin/
RUN chmod 0755 /usr/local/bin/run-discovery.sh /usr/local/bin/trace.sh
# greenup-link writes logs/greenup_*.log relative to its working directory.
WORKDIR /data/greenup-link
CMD ["run-discovery.sh"]
