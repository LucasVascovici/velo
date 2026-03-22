#!/bin/sh
# Velo installer
# Usage: curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh
#        curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh -s -- --dir ~/.local/bin
#        curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh -s -- --dry-run
set -e

OWNER="LucasVascovici"
REPO="velo"
BINARY="velo"

# ── Parse flags ───────────────────────────────────────────────────────────────
INSTALL_DIR=""
DRY_RUN=0
while [ $# -gt 0 ]; do
    case "$1" in
        --dir=*) INSTALL_DIR="${1#--dir=}" ;;
        --dir)   shift; INSTALL_DIR="$1"   ;;
        --dry-run) DRY_RUN=1               ;;
        --help|-h)
            echo "Usage: install.sh [--dir <path>] [--dry-run]"
            echo "  --dir <path>   Install directory (default: /usr/local/bin or ~/.local/bin)"
            echo "  --dry-run      Show what would happen without making any changes"
            exit 0 ;;
    esac
    shift
done

# ── Dependency check ──────────────────────────────────────────────────────────
for dep in curl uname; do
    if ! command -v "$dep" >/dev/null 2>&1; then
        echo "error: required tool not found: $dep"
        exit 1
    fi
done

# ── Detect platform and map to release asset name ────────────────────────────
# Release asset names (from .github/workflows/release.yml):
#   velo-x86_64-linux.tar.gz
#   velo-aarch64-linux.tar.gz
#   velo-x86_64-macos.tar.gz
#   velo-aarch64-macos.tar.gz
#   velo-x86_64-windows.zip

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$ARCH" in
    x86_64)        ASSET_ARCH="x86_64"  ;;
    arm64|aarch64) ASSET_ARCH="aarch64" ;;
    *)
        echo "error: unsupported architecture: $ARCH"
        echo "  Download manually: https://github.com/$OWNER/$REPO/releases"
        exit 1 ;;
esac

case "$OS" in
    Linux)
        ASSET="${BINARY}-${ASSET_ARCH}-linux.tar.gz"
        EXT="tar.gz"
        ;;
    Darwin)
        ASSET="${BINARY}-${ASSET_ARCH}-macos.tar.gz"
        EXT="tar.gz"
        ;;
    MSYS*|CYGWIN*|MINGW*)
        # Only x86_64 is available for Windows — ARM64 runs via emulation
        ASSET="${BINARY}-x86_64-windows.zip"
        ASSET_ARCH="x86_64"
        EXT="zip"
        ;;
    *)
        echo "error: unsupported OS: $OS"
        echo "  Download manually: https://github.com/$OWNER/$REPO/releases"
        exit 1 ;;
esac

echo "  Platform : $OS ($ARCH)"
echo "  Asset    : $ASSET"

# ── Resolve install directory ─────────────────────────────────────────────────
if [ -z "$INSTALL_DIR" ]; then
    if [ -d "/usr/local/bin" ] && [ -w "/usr/local/bin" ]; then
        INSTALL_DIR="/usr/local/bin"
    elif command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
        INSTALL_DIR="/usr/local/bin"
    else
        INSTALL_DIR="$HOME/.local/bin"
    fi
fi

echo "  Install  : $INSTALL_DIR/$BINARY"

if [ "$DRY_RUN" = "1" ]; then
    echo ""
    echo "  (dry-run — no changes made)"
    exit 0
fi

# ── Fetch latest release URL ──────────────────────────────────────────────────
echo ""
echo "Looking up latest release..."

RELEASE_JSON=$(curl -fsSL "https://api.github.com/repos/$OWNER/$REPO/releases/latest")

# Extract the download URL for our asset
RELEASE_URL=$(printf '%s' "$RELEASE_JSON" \
    | grep "browser_download_url" \
    | grep "\"$ASSET\"" \
    | cut -d '"' -f 4)

if [ -z "$RELEASE_URL" ]; then
    # Fallback: less strict grep (handles minor naming differences)
    RELEASE_URL=$(printf '%s' "$RELEASE_JSON" \
        | grep "browser_download_url" \
        | grep "$ASSET" \
        | cut -d '"' -f 4 \
        | head -1)
fi

if [ -z "$RELEASE_URL" ]; then
    echo "error: no release asset found matching '$ASSET'"
    echo ""
    echo "Available assets:"
    printf '%s' "$RELEASE_JSON" \
        | grep "browser_download_url" \
        | cut -d '"' -f 4 \
        | while read -r url; do
            echo "  $(basename "$url")"
          done
    echo ""
    echo "  Download manually: https://github.com/$OWNER/$REPO/releases"
    exit 1
fi

VERSION=$(printf '%s' "$RELEASE_URL" | sed 's|.*/download/\(v[^/]*\)/.*|\1|')
echo "  Version  : $VERSION"

# ── Download ──────────────────────────────────────────────────────────────────
# Use a unique temp dir that is always cleaned up on exit
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT INT TERM

echo ""
echo "Downloading $ASSET..."
curl -fsSL --progress-bar "$RELEASE_URL" -o "$WORK_DIR/package.$EXT"

# ── Extract ───────────────────────────────────────────────────────────────────
case "$EXT" in
    tar.gz)
        tar -xzf "$WORK_DIR/package.$EXT" -C "$WORK_DIR"
        ;;
    zip)
        # unzip may not be available on minimal systems; try tar first (BSDs)
        if command -v unzip >/dev/null 2>&1; then
            unzip -q "$WORK_DIR/package.$EXT" -d "$WORK_DIR"
        else
            echo "error: 'unzip' not found — install it and retry"
            exit 1
        fi
        ;;
esac

# Find the extracted binary (handles both 'velo' and 'velo.exe')
EXTRACTED=""
for name in "$BINARY" "${BINARY}.exe"; do
    if [ -f "$WORK_DIR/$name" ]; then
        EXTRACTED="$WORK_DIR/$name"
        break
    fi
done

if [ -z "$EXTRACTED" ]; then
    echo "error: binary not found in archive"
    echo "  Contents of archive:"
    ls -la "$WORK_DIR/"
    exit 1
fi

# ── Install ───────────────────────────────────────────────────────────────────
mkdir -p "$INSTALL_DIR"

chmod +x "$EXTRACTED" 2>/dev/null || true

DEST="$INSTALL_DIR/$BINARY"

# Replace any existing installation cleanly
if [ -f "$DEST" ]; then
    OLD_VERSION=$("$DEST" --version 2>/dev/null | head -1 || echo "unknown")
    echo "  Replacing existing installation ($OLD_VERSION)"
fi

if [ -w "$INSTALL_DIR" ]; then
    cp "$EXTRACTED" "$DEST"
else
    sudo cp "$EXTRACTED" "$DEST"
fi

# ── Verify ────────────────────────────────────────────────────────────────────
echo ""
if command -v "$BINARY" >/dev/null 2>&1; then
    INSTALLED=$("$BINARY" --version 2>/dev/null | head -1 || echo "velo $VERSION")
    echo "  $INSTALLED"
    echo "  installed to $DEST"
    echo ""
    echo "  Run 'velo --help' to get started."
else
    echo "  Installed to $DEST"
    echo ""
    echo "  WARNING: $INSTALL_DIR is not on your PATH."
    echo "  Add this line to your shell config (~/.bashrc, ~/.zshrc, etc.):"
    echo ""
    echo "    export PATH=\"\$PATH:$INSTALL_DIR\""
    echo ""
    echo "  Then restart your shell or run: source ~/.bashrc"
fi