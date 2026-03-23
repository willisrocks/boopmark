#!/bin/sh
set -eu

BOOP_VERSION="${BOOP_VERSION:-latest}"
BOOP_INSTALL_DIR="${BOOP_INSTALL_DIR:-${HOME}/.local/bin}"
BOOP_INSTALL_BASE_URL="${BOOP_INSTALL_BASE_URL:-https://github.com/willisrocks/boopmark/releases}"

main() {
    detect_platform
    construct_url
    create_install_dir
    download_binary
    verify_installation
    echo "boop installed successfully to ${BOOP_INSTALL_DIR}/boop"
    case ":${PATH}:" in
        *":${BOOP_INSTALL_DIR}:"*) ;;
        *) echo "Note: ${BOOP_INSTALL_DIR} is not in your PATH. Add it with:" >&2
           echo "  export PATH=\"${BOOP_INSTALL_DIR}:\$PATH\"" >&2 ;;
    esac
}

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin) OS_TARGET="apple-darwin" ;;
        Linux)  OS_TARGET="unknown-linux-gnu" ;;
        *)      echo "Error: unsupported operating system: $OS" >&2; exit 1 ;;
    esac

    case "$ARCH" in
        x86_64|amd64)  ARCH_TARGET="x86_64" ;;
        aarch64|arm64) ARCH_TARGET="aarch64" ;;
        *)             echo "Error: unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac

    TARGET="${ARCH_TARGET}-${OS_TARGET}"
}

construct_url() {
    BINARY_NAME="boop-${TARGET}"
    if [ "$BOOP_VERSION" = "latest" ]; then
        DOWNLOAD_URL="${BOOP_INSTALL_BASE_URL}/latest/download/${BINARY_NAME}"
    else
        DOWNLOAD_URL="${BOOP_INSTALL_BASE_URL}/download/${BOOP_VERSION}/${BINARY_NAME}"
    fi
}

create_install_dir() {
    if [ ! -d "$BOOP_INSTALL_DIR" ]; then
        if ! mkdir -p "$BOOP_INSTALL_DIR" 2>/dev/null; then
            echo "Error: failed to create install directory ${BOOP_INSTALL_DIR}" >&2
            echo "Try running with sudo or set BOOP_INSTALL_DIR to a writable location." >&2
            exit 1
        fi
    elif [ ! -w "$BOOP_INSTALL_DIR" ]; then
        echo "Error: install directory ${BOOP_INSTALL_DIR} is not writable" >&2
        echo "Try running with sudo or set BOOP_INSTALL_DIR to a writable location." >&2
        exit 1
    fi
}

download_binary() {
    TMPFILE="$(mktemp)"
    trap 'rm -f "$TMPFILE"' EXIT

    if command -v curl >/dev/null 2>&1; then
        if ! curl -fsSL -o "$TMPFILE" "$DOWNLOAD_URL"; then
            echo "Error: failed to download boop from ${DOWNLOAD_URL}" >&2
            exit 1
        fi
    elif command -v wget >/dev/null 2>&1; then
        if ! wget -q -O "$TMPFILE" "$DOWNLOAD_URL"; then
            echo "Error: failed to download boop from ${DOWNLOAD_URL}" >&2
            exit 1
        fi
    else
        echo "Error: neither curl nor wget found. Please install one and try again." >&2
        exit 1
    fi

    STAGING="${BOOP_INSTALL_DIR}/boop.tmp.$$"
    if ! cp "$TMPFILE" "$STAGING"; then
        rm -f "$STAGING"
        echo "Error: failed to copy binary to ${BOOP_INSTALL_DIR}" >&2
        exit 1
    fi
    if ! chmod 755 "$STAGING"; then
        rm -f "$STAGING"
        echo "Error: failed to set executable permissions" >&2
        exit 1
    fi
    if [ "$(uname -s)" = "Darwin" ]; then
        xattr -cr "$STAGING" 2>/dev/null || true
        if ! codesign --force --sign - "$STAGING" 2>/dev/null; then
            echo "Warning: failed to ad-hoc sign binary; Gatekeeper may kill the binary on first run" >&2
        fi
    fi
    if ! mv "$STAGING" "${BOOP_INSTALL_DIR}/boop"; then
        rm -f "$STAGING"
        echo "Error: failed to install binary to ${BOOP_INSTALL_DIR}/boop" >&2
        exit 1
    fi
    rm -f "$TMPFILE"
    trap - EXIT
}

verify_installation() {
    if [ ! -x "${BOOP_INSTALL_DIR}/boop" ]; then
        echo "Error: installation failed — binary not found at ${BOOP_INSTALL_DIR}/boop" >&2
        exit 1
    fi
}

# __BOOP_INSTALL_MAIN__
main
