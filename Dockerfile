# syntax=docker/dockerfile:1.6
#
# Build:
#   docker build -t latch .
#
# Run with env vars (no config file needed):
#   docker volume create latch-data
#   docker run -d --name latch \
#     -e LATCH_RP_ID=latch.example.com \
#     -e LATCH_LISTEN=0.0.0.0:8080 \
#     -v latch-data:/var/lib/latch \
#     -p 8080:8080 latch
#
# Or mount a config.toml:
#   docker run -d --name latch \
#     -v $PWD/latch.toml:/etc/latch/config.toml:ro \
#     -v latch-data:/var/lib/latch \
#     -p 8080:8080 latch

ARG RUST_VERSION=1.83
ARG TARGET=x86_64-unknown-linux-musl
ARG STATE_DIR=/var/lib/latch

# --- builder ---------------------------------------------------------------
FROM rust:${RUST_VERSION}-alpine AS builder
ARG TARGET

RUN apk add --no-cache musl-dev perl make

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN rustup target add ${TARGET} && \
    cargo build --release --target ${TARGET} --features vendored && \
    strip target/${TARGET}/release/latch && \
    mv target/${TARGET}/release/latch /latch

# --- runtime ---------------------------------------------------------------
FROM scratch
ARG STATE_DIR

COPY --from=builder /latch /latch

VOLUME ${STATE_DIR}
EXPOSE 8080

ENTRYPOINT ["/latch"]
CMD ["run"]
