#!/bin/sh
# gray installer — https://gray.alignment.id
# stable:  curl -fsSL https://gray.alignment.id/install.sh | sh
# beta:    curl -fsSL https://gray.alignment.id/install.sh | sh -s -- beta
set -eu

CHANNEL="stable"
case "${1:-}" in
    ""|stable) CHANNEL="stable" ;;
    beta|nightly) CHANNEL="beta" ;;
    *) echo "unknown channel '$1' (use: stable | beta)"; exit 1 ;;
esac

REPO_URL="https://gray.alignment.id/dl"

have_cmd() { command -v "$1" >/dev/null 2>&1; }

ARCH=$(uname -m)
OS=$(uname -s)
case "$OS" in
    Linux) OS="linux" ;;
    Darwin) OS="darwin" ;;
    *) echo "this script is for Linux/macOS. on Windows, run install.ps1 in PowerShell instead."; exit 1 ;;
esac
case "$ARCH" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) echo "unsupported architecture: $ARCH"; exit 1 ;;
esac

TARBALL="gray-${CHANNEL}-${ARCH}-${OS}.tar.gz"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "→ downloading gray (${CHANNEL} channel, ${ARCH} ${OS})..."
if have_cmd curl; then
    curl -fsSL "${REPO_URL}/${TARBALL}" -o "${TMP}/${TARBALL}"
elif have_cmd wget; then
    wget -qO "${TMP}/${TARBALL}" "${REPO_URL}/${TARBALL}"
else
    echo "need curl or wget to download"; exit 1
fi

tar xzf "${TMP}/${TARBALL}" -C "$TMP"

# pick install dir: ~/.local/bin preferred, /usr/local/bin with sudo fallback
if [ -w "/usr/local/bin" ] || [ "$(id -u)" = "0" ]; then
    DEST="/usr/local/bin"
else
    DEST="$HOME/.local/bin"
    mkdir -p "$DEST"
fi

mv "${TMP}/gray" "${DEST}/gray"
chmod +x "${DEST}/gray"

echo "→ installed to ${DEST}/gray ($(${DEST}/gray --version 2>/dev/null || echo '?'), ${CHANNEL})"

case ":$PATH:" in
    *":${DEST}:"*) ;;
    *)
        echo "⚠ ${DEST} is not in your PATH"
        SHELL_RC="$HOME/.bashrc"
        [ -n "${ZSH_VERSION:-}" ] && SHELL_RC="$HOME/.zshrc"
        echo "  add it:  echo 'export PATH=\"${DEST}:\$PATH\"' >> ${SHELL_RC}"
        ;;
esac
