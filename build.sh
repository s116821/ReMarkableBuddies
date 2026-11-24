#!/bin/bash

# Build the ReMarkableBuddies tablet app.

set -e

TARGET="${1:-rm2}"

case "$TARGET" in
    "rm2")
        echo "Building for reMarkable2 (armv7)..."
        cross build --release --target=armv7-unknown-linux-gnueabihf
        echo "Binary: target/armv7-unknown-linux-gnueabihf/release/reader-buddy"
        ;;
    "rmpp")
        echo "Building for reMarkable Paper Pro (aarch64)..."
        cross build --release --target=aarch64-unknown-linux-gnu
        echo "Binary: target/aarch64-unknown-linux-gnu/release/reader-buddy"
        ;;
    "local")
        echo "Building for local system..."
        cargo build --release
        echo "Binary: target/release/reader-buddy"
        ;;
    *)
        echo "Usage: $0 [rm2|rmpp|local]"
        echo "  rm2   - Build for reMarkable2 (armv7)"
        echo "  rmpp  - Build for reMarkable Paper Pro (aarch64)"
        echo "  local - Build for local system"
        exit 1
        ;;
esac

echo "Build complete!"

