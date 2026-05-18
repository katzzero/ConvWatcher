FROM rust:1.90-alpine3.22 AS builder
RUN apk add --no-cache musl-dev pkgconfig

WORKDIR /app
COPY Cargo.toml ./
COPY src/ ./src/
COPY config/ ./config/

RUN cargo build --release --target x86_64-unknown-linux-musl

FROM alpine:3.23

RUN apk add --no-cache \
    ffmpeg \
    ghostscript \
    qpdf \
    poppler-utils \
    pandoc \
    py3-pip \
    wget \
    && pip install --no-cache-dir img2pdf \
    && rm -rf /var/cache/apk/*

RUN adduser -D -u 1000 convwatcher

WORKDIR /app

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/convwatcher /usr/local/bin/

RUN mkdir -p /app/config /app/watch /app/output /app/logs \
    && chown -R convwatcher:convwatcher /app

USER convwatcher

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- http://localhost:8080/health || exit 1

EXPOSE 8080

ENTRYPOINT ["convwatcher"]
CMD ["--no-daemon"]
