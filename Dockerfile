FROM rust:1.95-alpine AS builder

RUN apk add --no-cache musl-dev pkgconfig

ARG TARGETARCH

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY config/ ./config/

RUN if [ "$TARGETARCH" = "arm64" ]; then \
        rustup target add aarch64-unknown-linux-musl; \
    fi

RUN if [ "$TARGETARCH" = "arm64" ]; then \
        T=aarch64-unknown-linux-musl; \
    else \
        T=x86_64-unknown-linux-musl; \
    fi && \
    cargo build --release --target "$T" && \
    cp target/"$T"/release/convwatcher /convwatcher

FROM alpine:3.21

RUN apk add --no-cache \
    ffmpeg \
    ghostscript \
    qpdf \
    poppler-utils \
    pandoc \
    py3-pip \
    wget \
    curl \
    su-exec \
    && pip install --no-cache-dir img2pdf --break-system-packages \
    && rm -rf /var/cache/apk/*

RUN adduser -D -u 1000 convwatcher

WORKDIR /app

COPY --from=builder /convwatcher /usr/local/bin/convwatcher

RUN mkdir -p /app/config /app/inputs /app/outputs /app/logs \
    && chown -R convwatcher:convwatcher /app /usr/local/bin/convwatcher

COPY docker-entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

EXPOSE 8080

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["convwatcher", "--no-daemon"]
