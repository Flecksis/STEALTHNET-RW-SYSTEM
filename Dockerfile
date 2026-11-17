FROM rust:1.94.0-slim-bookworm AS build
ENV RUSTUP_TOOLCHAIN=1.94.0
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p sn-app \
    && install -D target/release/stealthnet /out/stealthnet

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/STEALTHNET-APP/STEALTHNET-RW-SYSTEM" \
      org.opencontainers.image.description="STEALTHNET RW SYSTEM" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home stealthnet
COPY --from=build /out/stealthnet /usr/local/bin/stealthnet
USER 10001
ENV SN_BIND=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1
ENTRYPOINT ["stealthnet"]
CMD ["serve"]
