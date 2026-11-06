# Build context is the repo root (see compose.yaml).
FROM docker.io/library/rust:1-bookworm AS build
WORKDIR /src

# The workspace lists src-tauri as a member; cargo needs its manifest (and the
# targets it names) to exist, but we never build it, so stub it out.
COPY Cargo.toml Cargo.lock ./
COPY src-tauri/Cargo.toml src-tauri/build.rs src-tauri/
RUN mkdir src-tauri/src && touch src-tauri/src/lib.rs src-tauri/src/main.rs
COPY crates crates
# The workspace's release profile is fat-LTO, which is slow and memory-hungry
# (the podman VM defaults to 2GiB); a probe doesn't need it.
ENV CARGO_PROFILE_RELEASE_LTO=off \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    CARGO_BUILD_JOBS=4
RUN cargo build --release --locked -p e2e-latency

FROM docker.io/library/debian:bookworm-slim
COPY --from=build /src/target/release/e2e-latency /usr/local/bin/e2e-latency
ENTRYPOINT ["e2e-latency"]
