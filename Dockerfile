# syntax=docker/dockerfile:1

ARG TARGETARCH

FROM rust:1.95-alpine AS builder

RUN apk add --no-cache musl-dev pkgconfig

ARG TARGETARCH

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/

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

FROM ubuntu:24.04 AS runtime-arm64

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    wget curl gnupg lsb-release \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /usr/share/keyrings && \
    wget -O- https://repo.jellyfin.org/jellyfin-team.gpg.key | gpg --dearmor -o /usr/share/keyrings/jellyfin.gpg && \
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/jellyfin.gpg] https://repo.jellyfin.org/ubuntu $(lsb_release -cs) main" > /etc/apt/sources.list.d/jellyfin.list

RUN apt-get update && apt-get install -y --no-install-recommends \
    jellyfin-ffmpeg7 \
    ghostscript \
    qpdf \
    poppler-utils \
    pandoc \
    python3-pip \
    && pip3 install --no-cache-dir img2pdf \
    && rm -rf /var/lib/apt/lists/*

RUN ln -sf /usr/lib/jellyfin-ffmpeg/ffmpeg /usr/local/bin/ffmpeg && \
    ln -sf /usr/lib/jellyfin-ffmpeg/ffprobe /usr/local/bin/ffprobe

FROM alpine:3.21 AS runtime-amd64

RUN apk add --no-cache \
    ffmpeg \
    ghostscript \
    qpdf \
    poppler-utils \
    pandoc \
    py3-pip \
    wget \
    curl \
    mesa-va-gallium \
    libva \
    && pip install --no-cache-dir img2pdf --break-system-packages \
    && rm -rf /var/cache/apk/*

FROM runtime-${TARGETARCH}

WORKDIR /app

COPY --from=builder /convwatcher /usr/local/bin/convwatcher

RUN mkdir -p /app/config /app/inputs /app/outputs /app/logs

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

EXPOSE 8080

ENTRYPOINT ["convwatcher"]
CMD ["--no-daemon"]
