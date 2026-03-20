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
for arg in "$@"; do
    case "$arg" in
        --dir=*)  INSTALL_DIR="${arg#--dir=}" ;;
        --dir)    shift; INSTALL_DIR="$1" ;;
        --dry-run) DRY_RUN=1 ;;
        --help|-h)
            echo "Usage: install.sh [--dir <path>] [--dry-run]"
            echo "  --dir <path>   Install directory (default: /usr/local/bin or ~/.local/bin)"
            echo "  --dry-run      Print what would be done without installing"
            exit 0 ;;
    esac
done

# ── Dependency check ──────────────────────────────────────────────────────────
for dep in curl uname; do
    if ! command -v "$dep" >/dev/null 2>&1; then
        echo "❌ Required tool not found: $dep"
        exit 1
    fi
done

# ── Detect platform ───────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux)  PLATFORM="unknown-linux-musl"; EXT="tar.gz" ;;
    Darwin) PLATFORM="apple-darwin";       EXT="tar.gz" ;;
    MSYS*|CYGWIN*|MINGW*)
            PLATFORM="pc-windows-msvc";    EXT="zip"    ;;
    *)
        echo "❌ Unsupported OS: $OS"
        echo "   Download a binary manually from: https://github.com/$OWNER/$REPO/releases"
        exit 1 ;;
esac

case "$ARCH" in
    x86_64)       TARGET_ARCH="x86_64"  ;;
    arm64|aarch64) TARGET_ARCH="aarch64" ;;
    *)
        echo "❌ Unsupported architecture: $ARCH"
        echo "   Download a binary manually from: https://github.com/$OWNER/$REPO/releases"
        exit 1 ;;
esac

ASSET="${BINARY}-${TARGET_ARCH}-${PLATFORM}.${EXT}"
echo "  Platform : $OS ($ARCH) → $ASSET"

# ── Resolve install directory ─────────────────────────────────────────────────
if [ -z "$INSTALL_DIR" ]; then
    # Prefer /usr/local/bin if it exists and we have write access (or can sudo).
    # Fall back to ~/.local/bin (no sudo required).
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
echo "🔍 Looking up latest release..."

RELEASE_URL=$(curl -fsSL "https://api.github.com/repos/$OWNER/$REPO/releases/latest" \
    | grep "browser_download_url" \
    | grep "$ASSET" \
    | cut -d '"' -f 4)

if [ -z "$RELEASE_URL" ]; then
    echo "❌ Could not find a release asset matching: $ASSET"
    echo "   Check https://github.com/$OWNER/$REPO/releases for available builds."
    exit 1
fi

VERSION=$(echo "$RELEASE_URL" | sed 's|.*/download/\(v[^/]*\)/.*|\1|')
echo "   Found $VERSION"

# ── Download to a temp directory (cleaned up on exit) ─────────────────────────
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT INT TERM

echo "📥 Downloading..."
curl -fsSL --progress-bar "$RELEASE_URL" -o "$TMPDIR/package"

# ── Extract ───────────────────────────────────────────────────────────────────
case "$EXT" in
    tar.gz) tar -xzf "$TMPDIR/package" -C "$TMPDIR" ;;
    zip)    unzip -q  "$TMPDIR/package" -d "$TMPDIR" ;;
esac

if [ ! -f "$TMPDIR/$BINARY" ] && [ ! -f "$TMPDIR/${BINARY}.exe" ]; then
    echo "❌ Binary not found inside the downloaded archive."
    exit 1
fi

# ── Install ───────────────────────────────────────────────────────────────────
mkdir -p "$INSTALL_DIR"
chmod +x "$TMPDIR/$BINARY" 2>/dev/null || true

if [ -w "$INSTALL_DIR" ]; then
    mv "$TMPDIR/$BINARY" "$INSTALL_DIR/$BINARY"
else
    sudo mv "$TMPDIR/$BINARY" "$INSTALL_DIR/$BINARY"
fi

# ── Verify ────────────────────────────────────────────────────────────────────
if ! command -v "$BINARY" >/dev/null 2>&1; then
    # Binary installed but may not be on PATH
    echo ""
    echo "✔ Velo $VERSION installed to $INSTALL_DIR/$BINARY"
    echo ""
    echo "⚠  $INSTALL_DIR is not on your PATH."
    echo "   Add this to your shell config (~/.bashrc, ~/.zshrc, etc.):"
    echo ""
    echo "     export PATH=\"\$PATH:$INSTALL_DIR\""
    echo ""
else
    INSTALLED_VERSION=$("$BINARY" --version 2>/dev/null | head -1 || echo "")
    echo ""
    echo "✔ $INSTALLED_VERSION installed to $INSTALL_DIR/$BINARY"
    echo ""
    echo "  Run 'velo --help' to get started."
fi