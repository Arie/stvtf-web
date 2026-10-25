# Built with the directory holding both repositories as context (stvtf-web depends on ../stvtf):
#   docker build -f stvtf-web/Dockerfile .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY stvtf ./stvtf
COPY stvtf-web ./stvtf-web
# The crates and target directory are cache mounts, so a deploy recompiles only what changed;
# the binary is copied out because the cache is not part of the image.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/stvtf-web/target \
    cd stvtf-web && cargo build --release --locked && cp target/release/stvtf-web /usr/local/bin/

FROM debian:bookworm-slim
RUN useradd --system --home /data stvtf && mkdir -p /data && chown stvtf /data
COPY --from=build /usr/local/bin/stvtf-web /usr/local/bin/stvtf-web
COPY stvtf-web/backpack /usr/share/stvtf/backpack
USER stvtf
WORKDIR /data
EXPOSE 8080
# The token key comes from the environment (STVTF_TOKEN_KEY); the map cache lives in /data.
ENTRYPOINT ["stvtf-web", "--listen", "0.0.0.0:8080", "--cache", "/data/cache", "--backpack", "/usr/share/stvtf/backpack"]
CMD ["--public"]
