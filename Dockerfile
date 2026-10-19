# Built with the directory holding both repositories as context (stvtf-web depends on ../stvtf):
#   docker build -f stvtf-web/Dockerfile .
FROM rust:1.93.1-bookworm AS build
WORKDIR /src
COPY stvtf ./stvtf
COPY stvtf-web ./stvtf-web
RUN cd stvtf-web && cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --home /data stvtf && mkdir -p /data && chown stvtf /data
COPY --from=build /src/stvtf-web/target/release/stvtf-web /usr/local/bin/stvtf-web
USER stvtf
WORKDIR /data
EXPOSE 8080
# The token key comes from the environment (STVTF_TOKEN_KEY); the map cache lives in /data.
ENTRYPOINT ["stvtf-web", "--listen", "0.0.0.0:8080", "--cache", "/data/cache"]
CMD ["--public"]
