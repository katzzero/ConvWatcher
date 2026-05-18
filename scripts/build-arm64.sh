#!/bin/bash
set -e

TARGET="aarch64-unknown-linux-musl"

if command -v cross &>/dev/null; then
    echo "Building with cross for $TARGET..."
    cross build --release --target "$TARGET"
elif command -v cargo-zigbuild &>/dev/null; then
    echo "Building with zigbuild for $TARGET..."
    cargo zigbuild --release --target "$TARGET"
else
    echo "Neither 'cross' nor 'cargo-zigbuild' found."
    echo "Install one of them:"
    echo "  cargo install cross"
    echo "  cargo install cargo-zigbuild"
    exit 1
fi

echo "Build complete: target/$TARGET/release/convwatcher"
