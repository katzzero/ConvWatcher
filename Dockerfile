FROM rust:1.95-alpine AS builder

RUN apk add --no-cache musl-dev pkgconfig

ARG TARGETARCH

WORKDIR /app

COPY Cargo.toml Cargo.lock ./

RUN if [ "$TARGETARCH" = "arm64" ]; then \
        rustup target add aarch64-unknown-linux-musl; \
    fi

RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN if [ "$TARGETARCH" = "arm64" ]; then \
        T=aarch64-unknown-linux-musl; \
    else \
        T=x86_64-unknown-linux-musl; \
    fi && \
    cargo build --release --target "$T"

RUN rm -rf src

COPY src/ ./src/
COPY config/ ./config/

RUN if [ "$TARGETARCH" = "arm64" ]; then \
        T=aarch64-unknown-linux-musl; \
    else \
        T=x86_64-unknown-linux-musl; \
    fi && \
    cargo build --release --target "$T"

FROM alpine:3.21

ARG TARGETARCH

RUN apk add --no-cache \
    ffmpeg \
    ghostscript \
    qpdf \
    poppler-utils \
    pandoc \
    py3-pip \
    wget \
    curl \
    && pip install --no-cache-dir img2pdf --break-system-packages \
    && rm -rf /var/cache/apk/*

RUN adduser -D -u 1000 convwatcher

WORKDIR /app

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/convwatcher /usr/local/bin/ 2>/dev/null || true
COPY --from=builder /app/target/aarch64-unknown-linux-musl/release/convwatcher /usr/local/bin/ 2>/dev/null || true

RUN mkdir -p /app/config /app/inputs /app/outputs /app/logs \
    && chown -R convwatcher:convwatcher /app

USER convwatcher

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

EXPOSE 8080

ENTRYPOINT ["convwatcher"]
CMD ["--no-daemon"]
