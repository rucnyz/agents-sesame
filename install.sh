#!/bin/bash
set -e

# rust-resume (fr-rs) install script
# Usage: curl -fsSL https://rucnyz.github.io/rust-resume/install.sh | bash

REPO="rucnyz/rust-resume"
BINARY="fr-rs"

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

info() { echo -e "${BLUE}[INFO]${NC} $1"; }
success() { echo -e "${GREEN}[OK]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

detect_system() {
    OS=""
    ARCH=""

    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        OS="linux"
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        OS="macos"
    else
        error "Unsupported OS: $OSTYPE"
    fi

    case $(uname -m) in
        x86_64|amd64) ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *) error "Unsupported architecture: $(uname -m)" ;;
    esac

    info "Detected: $OS ($ARCH)"
}

get_target() {
    case "$OS-$ARCH" in
        linux-x86_64)   TARGET="x86_64-unknown-linux-musl" ;;
        linux-aarch64)  TARGET="aarch64-unknown-linux-gnu" ;;
        macos-x86_64)   TARGET="x86_64-apple-darwin" ;;
        macos-aarch64)  TARGET="aarch64-apple-darwin" ;;
        *) error "No pre-built binary for $OS-$ARCH" ;;
    esac
}

install_binary() {
    info "Fetching latest release..."
    VER=$(curl -s "https://api.github.com/repos/$REPO/releases/latest" | grep '"tag_name":' | sed -E 's/.*"tag_name": "([^"]+)".*/\1/')

    if [[ -z "$VER" ]]; then
        error "Failed to fetch latest version. Check your internet connection or try: cargo install --git https://github.com/$REPO"
    fi

    RELEASE_NAME="$BINARY-$VER-$TARGET"
    TARBALL="$RELEASE_NAME.tar.gz"
    URL="https://github.com/$REPO/releases/download/$VER/$TARBALL"

    info "Downloading $BINARY $VER for $TARGET..."
    TMPDIR=$(mktemp -d)
    trap 'rm -rf "$TMPDIR"' EXIT

    if ! curl -fSL -o "$TMPDIR/$TARBALL" "$URL"; then
        error "Download failed. Release may not exist for this platform yet."
    fi

    info "Verifying checksum..."
    if curl -fSL -o "$TMPDIR/$TARBALL.sha256" "$URL.sha256" 2>/dev/null; then
        cd "$TMPDIR"
        if command -v shasum >/dev/null 2>&1; then
            shasum -a 256 -c "$TARBALL.sha256" || error "Checksum verification failed"
        elif command -v sha256sum >/dev/null 2>&1; then
            sha256sum -c "$TARBALL.sha256" || error "Checksum verification failed"
        fi
        cd - >/dev/null
    fi

    info "Extracting..."
    tar -xzf "$TMPDIR/$TARBALL" -C "$TMPDIR"

    # Install to ~/.local/bin (no sudo needed)
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
    mv "$TMPDIR/$RELEASE_NAME/$BINARY" "$INSTALL_DIR/$BINARY"
    chmod +x "$INSTALL_DIR/$BINARY"

    success "$BINARY $VER installed to $INSTALL_DIR/$BINARY"

    # Check if install dir is in PATH
    if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
        echo ""
        info "Add $INSTALL_DIR to your PATH:"
        info "  export PATH=\"$INSTALL_DIR:\$PATH\""
    fi
}

install_cargo() {
    if ! command -v cargo >/dev/null 2>&1; then
        error "cargo not found. Install Rust first: https://rustup.rs"
    fi
    info "Building from source with cargo..."
    cargo install --git "https://github.com/$REPO"
    success "$BINARY installed via cargo!"
}

main() {
    echo "$BINARY install script"
    echo "========================"

    detect_system
    get_target

    # Prefer binary download, fallback to cargo
    install_binary || install_cargo
}

trap 'echo ""; error "Cancelled"' INT
main "$@"
