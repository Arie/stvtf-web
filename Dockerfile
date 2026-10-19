# Built with the directory holding both repositories as context (stvmap-web depends on ../stvmap):
#   docker build -f stvmap-web/Dockerfile .
FROM rust:1.93.1-bookworm AS build
WORKDIR /src
COPY stvmap ./stvmap
COPY stvmap-web ./stvmap-web
RUN cd stvmap-web && cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --home /data stvmap && mkdir -p /data && chown stvmap /data
COPY --from=build /src/stvmap-web/target/release/stvmap-web /usr/local/bin/stvmap-web
USER stvmap
WORKDIR /data
EXPOSE 8080
# The token key comes from the environment (STVMAP_TOKEN_KEY); the map cache lives in /data.
ENTRYPOINT ["stvmap-web", "--listen", "0.0.0.0:8080", "--cache", "/data/cache"]
CMD ["--public", "--base-path", "/live"]
