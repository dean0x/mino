#!/bin/sh
# Minotaur installer script
# Usage: curl -fsSL https://raw.githubusercontent.com/dean0x/minotaur/main/scripts/install.sh | sh
set -e

REPO="dean0x/minotaur"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="minotaur"

# Colors (disabled if not a terminal)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    BLUE='\033[0;34m'
    NC='\033[0m'
else
    RED=''
    GREEN=''
    YELLOW=''
    BLUE=''
    NC=''
fi

info() { printf "${BLUE}[info]${NC} %s\n" "$1"; }
warn() { printf "${YELLOW}[warn]${NC} %s\n" "$1"; }
error() { printf "${RED}[error]${NC} %s\n" "$1" >&2; exit 1; }
success() { printf "${GREEN}[ok]${NC} %s\n" "$1"; }

# Check for required commands
command -v curl >/dev/null 2>&1 || error "curl is required but not installed"
command -v tar >/dev/null 2>&1 || error "tar is required but not installed"

# Detect OS
OS=$(uname -s)
case "$OS" in
    Darwin) TARGET_OS="apple-darwin" ;;
    Linux)  TARGET_OS="unknown-linux-gnu" ;;
    *)      error "Unsupported operating system: $OS" ;;
esac

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
    x86_64)         TARGET_ARCH="x86_64" ;;
    aarch64|arm64)  TARGET_ARCH="aarch64" ;;
    *)              error "Unsupported architecture: $ARCH" ;;
esac

TARGET="${TARGET_ARCH}-${TARGET_OS}"
info "Detected platform: $TARGET"

# Get latest version from GitHub API
info "Fetching latest release..."
VERSION=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name"' | sed -E 's/.*"([^"]+)".*/\1/')
if [ -z "$VERSION" ]; then
    error "Failed to fetch latest version. Check your internet connection."
fi
info "Latest version: $VERSION"

# Construct download URLs
ARTIFACT_NAME="${BINARY_NAME}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/$VERSION/$ARTIFACT_NAME"
CHECKSUM_URL="https://github.com/$REPO/releases/download/$VERSION/checksums.txt"

# Create temp directory for download
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Download binary
info "Downloading $ARTIFACT_NAME..."
if ! curl -fsSL "$DOWNLOAD_URL" -o "$TMPDIR/$ARTIFACT_NAME"; then
    error "Failed to download binary. Release may not exist for $TARGET."
fi

# Download and verify checksum
info "Verifying checksum..."
if curl -fsSL "$CHECKSUM_URL" -o "$TMPDIR/checksums.txt" 2>/dev/null; then
    EXPECTED_SUM=$(grep "$ARTIFACT_NAME" "$TMPDIR/checksums.txt" | awk '{print $1}')
    if [ -n "$EXPECTED_SUM" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            ACTUAL_SUM=$(sha256sum "$TMPDIR/$ARTIFACT_NAME" | awk '{print $1}')
        elif command -v shasum >/dev/null 2>&1; then
            ACTUAL_SUM=$(shasum -a 256 "$TMPDIR/$ARTIFACT_NAME" | awk '{print $1}')
        else
            warn "No sha256sum or shasum available, skipping checksum verification"
            EXPECTED_SUM=""
        fi

        if [ -n "$EXPECTED_SUM" ]; then
            if [ "$EXPECTED_SUM" != "$ACTUAL_SUM" ]; then
                error "Checksum verification failed!\nExpected: $EXPECTED_SUM\nActual:   $ACTUAL_SUM"
            fi
            success "Checksum verified"
        fi
    else
        warn "Checksum for $ARTIFACT_NAME not found in checksums.txt"
    fi
else
    warn "Could not download checksums.txt, skipping verification"
fi

# Extract binary
info "Extracting..."
tar xzf "$TMPDIR/$ARTIFACT_NAME" -C "$TMPDIR"

# Create install directory if needed
if [ ! -d "$INSTALL_DIR" ]; then
    info "Creating $INSTALL_DIR..."
    mkdir -p "$INSTALL_DIR"
fi

# Install binary
info "Installing to $INSTALL_DIR/$BINARY_NAME..."
mv "$TMPDIR/$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
chmod +x "$INSTALL_DIR/$BINARY_NAME"

success "Installed $BINARY_NAME $VERSION to $INSTALL_DIR/$BINARY_NAME"

# Check if install dir is in PATH
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        warn "$INSTALL_DIR is not in your PATH"
        echo ""
        echo "Add it to your shell profile:"
        echo ""
        echo "  # For bash:"
        echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc"
        echo ""
        echo "  # For zsh:"
        echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc"
        echo ""
        ;;
esac

# Verify installation
if [ -x "$INSTALL_DIR/$BINARY_NAME" ]; then
    echo ""
    success "Installation complete!"
    echo ""
    echo "Run '$BINARY_NAME --help' to get started."
fi
