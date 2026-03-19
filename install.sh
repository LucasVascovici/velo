#!/bin/sh
set -e

# Configuration
OWNER="LucasVascovici"
REPO="velo"
BINARY_NAME="velo"

# Detect OS
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
    linux*)  PLATFORM="unknown-linux-musl" ;;
    darwin*) PLATFORM="apple-darwin" ;;
    msys*|cygwin*|mingw*) PLATFORM="pc-windows-msvc" ;;
    *) echo "Unsupported OS: $OS"; exit 1 ;;
esac

case "$ARCH" in
    x86_64) TARGET_ARCH="x86_64" ;;
    arm64|aarch64) TARGET_ARCH="aarch64" ;;
    *) echo "Unsupported Architecture: $ARCH"; exit 1 ;;
esac

# Construct the expected asset name (adjust based on your release action output)
ASSET_NAME="${BINARY_NAME}-${TARGET_ARCH}-${PLATFORM}.tar.gz"
if [ "$OS" = "windows" ]; then ASSET_NAME="${BINARY_NAME}-${TARGET_ARCH}-${PLATFORM}.zip"; fi

echo "🚀 Finding latest release for $OS ($ARCH)..."
LATEST_RELEASE_URL=$(curl -s https://api.github.com/repos/$OWNER/$REPO/releases/latest | grep "browser_download_url" | grep "$ASSET_NAME" | cut -d '"' -f 4)

if [ -z "$LATEST_RELEASE_URL" ]; then
    echo "❌ Error: Could not find a release matching your system."
    exit 1
fi

echo "📥 Downloading $BINARY_NAME..."
curl -L "$LATEST_RELEASE_URL" -o "velo_package"

# Extract and Install
if [ "$OS" = "windows" ]; then
    unzip velo_package
else
    tar -xzf velo_package
fi

chmod +x $BINARY_NAME
sudo mv $BINARY_NAME /usr/local/bin/

echo "✨ Velo installed successfully! Type 'velo help' to get started."