# ConvWatcher — Build & Deploy

## Building

### Local Development Build

```bash
# Debug build (fast compilation)
cargo build

# Release build (optimized)
cargo build --release

# The binary will be at: target/release/convwatcher
```

### Cross-Compile for ARM64

```bash
# Using cross (recommended)
cargo install cross
cross build --release --target aarch64-unknown-linux-musl

# Using zigbuild
cargo install cargo-zigbuild
cargo zigbuild --release --target aarch64-unknown-linux-musl
```

### Release Profile (`Cargo.toml`)

```toml
[profile.release]
opt-level = 3          # Maximum optimization
lto = true             # Link-time optimization
codegen-units = 1      # Single codegen unit for better optimization
strip = true           # Strip debug symbols
```

## Docker

### Single Architecture Build

```bash
docker build -t convwatcher:latest .
```

### Multi-Architecture Build

```bash
# Using docker buildx
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t ghcr.io/katzzero/convwatcher:latest \
  -t katzzero/convwatcher:latest \
  --push .
```

### Docker Compose

```yaml
version: "3.8"
services:
  convwatcher:
    image: ghcr.io/katzzero/convwatcher:latest
    container_name: convwatcher
    restart: unless-stopped
    volumes:
      - ./config:/app/config:ro
      - ./watch:/app/watch
      - ./output:/app/output
      - ./logs:/app/logs
      - /path/to/extra/watches:/data:ro    # For embedded configs
    devices:
      - /dev/dri:/dev/dri                    # VAAPI hardware acceleration
    environment:
      - RUST_LOG=info
    ports:
      - "8080:8080"
    deploy:
      resources:
        limits:
          memory: 2G
```

### Dockerfile

```dockerfile
# ── Builder Stage ──
FROM rust:1.90-alpine3.22 AS builder
RUN apk add --no-cache musl-dev pkgconfig

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY config/ ./config/

# Build with static linking for musl targets
RUN cargo build --release --target x86_64-unknown-linux-musl

# ── Runtime Stage ──
FROM alpine:3.23

# Install runtime dependencies
RUN apk add --no-cache \
    ffmpeg \
    ghostscript \
    qpdf \
    poppler-utils \
    pandoc \
    py3-pip \
    && pip install --no-cache-dir img2pdf

# Create non-root user
RUN adduser -D -u 1000 convwatcher

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/convwatcher /usr/local/bin/

# Create default directories
RUN mkdir -p /app/config /app/watch /app/output /app/logs \
    && chown -R convwatcher:convwatcher /app

USER convwatcher

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- http://localhost:8080/health || exit 1

EXPOSE 8080

ENTRYPOINT ["convwatcher"]
CMD ["--no-daemon"]
```

## CI/CD (GitHub Actions)

```yaml
# .github/workflows/docker.yml
name: Build and Push Docker Images

on:
  push:
    tags:
      - 'v*.*.*'

jobs:
  docker:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Set up QEMU
        uses: docker/setup-qemu-action@v3

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3

      - name: Login to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Login to Docker Hub
        uses: docker/login-action@v3
        with:
          username: ${{ secrets.DOCKER_USERNAME }}
          password: ${{ secrets.DOCKER_PASSWORD }}

      - name: Extract version
        id: version
        run: echo "VERSION=${GITHUB_REF#refs/tags/v}" >> $GITHUB_OUTPUT

      - name: Build and push
        uses: docker/build-push-action@v5
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          push: true
          tags: |
            ghcr.io/katzzero/convwatcher:latest
            ghcr.io/katzzero/convwatcher:${{ steps.version.outputs.VERSION }}
            katzzero/convwatcher:latest
            katzzero/convwatcher:${{ steps.version.outputs.VERSION }}
```

## Installation Scripts

### Linux (install_linux.sh)

```bash
#!/bin/bash
# Auto-detects distro and installs dependencies

detect_pkg_manager() {
    if command -v apt &>/dev/null; then echo "apt"
    elif command -v dnf &>/dev/null; then echo "dnf"
    elif command -v pacman &>/dev/null; then echo "pacman"
    elif command -v zypper &>/dev/null; then echo "zypper"
    else echo "unknown"; fi
}

install_deps() {
    case $(detect_pkg_manager) in
        apt)
            sudo apt update
            sudo apt install -y ffmpeg ghostscript qpdf poppler-utils pandoc python3-pip
            ;;
        dnf)
            sudo dnf install -y ffmpeg ghostscript qpdf poppler-utils pandoc python3-pip
            ;;
        pacman)
            sudo pacman -S --noconfirm ffmpeg ghostscript qpdf poppler pandoc python-pip
            ;;
        zypper)
            sudo zypper install -y ffmpeg ghostscript qpdf poppler-tools pandoc python3-pip
            ;;
    esac
    pip install --user img2pdf
}

install_deps
echo "Dependencies installed successfully"
```

### macOS (install_macos.sh)

```bash
#!/bin/bash
brew install ffmpeg ghostscript qpdf poppler pandoc
pip3 install img2pdf
echo "Dependencies installed successfully"
```

### Windows (install_windows.ps1)

```powershell
choco install ffmpeg ghostscript qpdf poppler pandoc python
pip install img2pdf
Write-Host "Dependencies installed successfully"
```

## Running

### Basic Usage

```bash
# Foreground with defaults
convwatcher

# Daemon mode (background)
convwatcher --daemon

# Debug logging
convwatcher --level debug

# Custom config file
convwatcher --config /path/to/config.yaml

# Quick single-folder watch
convwatcher --watch /path/to/folder
```

### With Docker

```bash
# Normal run
docker run -d \
  --name convwatcher \
  -v ./config:/app/config:ro \
  -v ./watch:/app/watch \
  -v ./output:/app/output \
  -v ./logs:/app/logs \
  --device /dev/dri:/dev/dri \
  -p 8080:8080 \
  ghcr.io/katzzero/convwatcher:latest

# With embedded configs (scan /data for mainconfig.yaml)
docker run -d \
  --name convwatcher \
  -v ./config:/app/config:ro \
  -v /any/folder/on/host:/data \
  -v ./output:/app/output \
  -v ./logs:/app/logs \
  -p 8080:8080 \
  ghcr.io/katzzero/convwatcher:latest
```

## Monitoring

Once running, open your browser:

```
http://localhost:8080/dashboard
```

Or check health via curl:

```bash
curl http://localhost:8080/health
```
