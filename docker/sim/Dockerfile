# syntax=docker/dockerfile:1
# legrand-sim: the fake power board behind TCP, for the Mac SIL compose.
# Build from the repo root:
#   docker build --platform linux/arm64 -f docker/sim/Dockerfile -t legrand-sim .

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bins ./bins
RUN cargo build --release --locked -p legrand-sim \
    && install -m 0755 target/release/legrand-sim /usr/local/bin/legrand-sim

FROM debian:bookworm-slim
COPY --from=build /usr/local/bin/legrand-sim /usr/local/bin/legrand-sim
EXPOSE 5101 5102
ENTRYPOINT ["legrand-sim"]
CMD ["--board", "0.0.0.0:5101", "--control", "0.0.0.0:5102"]
