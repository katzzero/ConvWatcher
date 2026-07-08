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

FROM ubuntu:24.04 AS ffmpeg-builder-arm64

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    git \
    wget \
    ca-certificates \
    pkg-config \
    nasm \
    yasm \
    libdrm-dev \
    libva-dev \
    libx264-dev \
    libx265-dev \
    libvpx-dev \
    libaom-dev \
    libmp3lame-dev \
    libopus-dev \
    libvorbis-dev \
    && rm -rf /var/lib/apt/lists/*

RUN git clone --depth 1 --branch jellyfin-mpp https://github.com/nyanmisaka/mpp.git /tmp/mpp && \
    mkdir -p /tmp/mpp/build && \
    cd /tmp/mpp/build && \
    cmake -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_BUILD_TYPE=Release -DBUILD_TEST=OFF -DBUILD_DEC_TEST=OFF -DBUILD_ENC_TEST=OFF .. && \
    make -j$(nproc) && \
    make install && \
    rm -rf /tmp/mpp

RUN git clone --depth 1 --branch 7.1 https://github.com/nyanmisaka/ffmpeg-rockchip.git /tmp/ffmpeg && \
    cd /tmp/ffmpeg && \
    ./configure \
        --prefix=/usr \
        --enable-gpl \
        --enable-version3 \
        --enable-rkmpp \
        --enable-libdrm \
        --enable-libx264 \
        --enable-libx265 \
        --enable-libvpx \
        --enable-libaom \
        --enable-libmp3lame \
        --enable-libopus \
        --enable-libvorbis \
        --enable-nonfree \
        --enable-pthreads \
        --enable-runtime-cpudetect \
        --enable-avfilter \
        --disable-static \
        --enable-shared \
    && \
    make -j$(nproc) && \
    make install && \
    mkdir -p /so-export && \
    find /usr/lib/aarch64-linux-gnu -name '*.so*' -type f -exec cp {} /so-export/ \; && \
    rm -rf /tmp/ffmpeg

FROM ubuntu:24.04 AS runtime-arm64

COPY --from=ffmpeg-builder-arm64 /so-export/*.so* /usr/lib/aarch64-linux-gnu/
COPY --from=ffmpeg-builder-arm64 /usr/bin/ffmpeg /usr/bin/ffmpeg
COPY --from=ffmpeg-builder-arm64 /usr/bin/ffprobe /usr/bin/ffprobe
COPY --from=ffmpeg-builder-arm64 /usr/share/ffmpeg /usr/share/ffmpeg

RUN ldconfig

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    wget curl \
    ghostscript \
    qpdf \
    poppler-utils \
    pandoc \
    python3-pip \
    && pip3 install --no-cache-dir img2pdf \
    && rm -rf /var/lib/apt/lists/*

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
