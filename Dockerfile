FROM node:22.22.0-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build \
    && find dist -type f \( -name '*.js' -o -name '*.css' -o -name '*.html' -o -name '*.svg' -o -name '*.json' \) -exec gzip -k -9 {} +

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
    && useradd --system --uid 10001 --no-create-home stealthnet \
    && install -d -m 700 -o 10001 /var/lib/stealthnet/backups
COPY --from=build /out/stealthnet /usr/local/bin/stealthnet
COPY --from=web /web/dist /usr/share/stealthnet/web
USER 10001
ARG SN_COMMIT=
ARG SN_BUILT_AT=
ARG SN_VERSION=
LABEL org.opencontainers.image.revision=$SN_COMMIT \
      org.opencontainers.image.created=$SN_BUILT_AT \
      org.opencontainers.image.version=$SN_VERSION
ENV SN_BIND=0.0.0.0:8080 \
    SN_WEB_DIR=/usr/share/stealthnet/web \
    SN_BUILD_COMMIT=$SN_COMMIT \
    SN_BUILD_DATE=$SN_BUILT_AT \
    SN_BUILD_VERSION=$SN_VERSION
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1
ENTRYPOINT ["stealthnet"]
CMD ["serve"]
