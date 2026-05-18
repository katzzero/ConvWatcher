#!/bin/bash
set -e

docker buildx create --name convwatcher-builder --use 2>/dev/null || docker buildx use convwatcher-builder

docker buildx build \
    --platform linux/amd64,linux/arm64 \
    -t ghcr.io/katzzero/convwatcher:latest \
    --load \
    .

echo "Multi-arch Docker image built successfully."
