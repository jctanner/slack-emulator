# Single-image build: React UI → embedded into the Rust binary → slim runtime.

FROM docker.io/library/node:22-trixie-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

FROM docker.io/library/rust:1-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY --from=web /src/web/dist web/dist
RUN cargo build --release --locked --bin slack-emu

FROM docker.io/library/debian:trixie-slim
# CA certificates: apps' HTTPS request URLs (and the HTTP client needs some to start).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data slack-emu \
    && mkdir -p /data && chown slack-emu /data
COPY --from=build /src/target/release/slack-emu /usr/local/bin/slack-emu
ENV BIND_ADDR=0.0.0.0:8080 \
    DATABASE_URL=sqlite:///data/slack-emu.db
VOLUME /data
EXPOSE 8080
USER slack-emu
CMD ["slack-emu"]
