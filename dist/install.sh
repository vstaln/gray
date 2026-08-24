#!/bin/sh
# gray installer — https://gray.alignment.id
# curl -fsSL https://gray.alignment.id/install.sh | sh
set -eu

VERSION="0.0.1"
REPO_URL="https://gray.alignment.id/dl"

have_cmd() { command -v "$1" >/dev/null 2>&1; }

ARCH=$(uname -m)
case "$ARCH" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) echo "note: aarch64 build not packaged yet — building from source instead (see github.com/vstaln/gray)"; exit 1 ;;
    *) echo "unsupported architecture: $ARCH"; exit 1 ;;
esac

if [ "$(uname -s)" != "Linux" ]; then
    echo "this script is for Linux/WSL. on Windows, run install.ps1 in PowerShell instead."
    exit 1
fi

TARBALL="gray-${VERSION}-${ARCH}-linux.tar.gz"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "→ downloading gray v${VERSION} (${ARCH})..."
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

echo "→ installed to ${DEST}/gray"

case ":$PATH:" in
    *":${DEST}:"*) ;;
    *)
        echo "⚠ ${DEST} is not in your PATH"
        SHELL_RC="$HOME/.bashrc"
        [ -n "${ZSH_VERSION:-}" ] && SHELL_RC="$HOME/.zshrc"
        echo "  add it:  echo 'export PATH=\"${DEST}:\$PATH\"' >> ${SHELL_RC}"
        ;;
esac

cat <<EOF

  gray v${VERSION} installed.

  get an API key (openrouter.ai or deepseek.com or any OpenAI-compatible), then:

      export GRAY_API_KEY=sk-or-...
      export GRAY_MODEL=anthropic/claude-sonnet-4     # or deepseek/deepseek-chat etc.
      gray

  one-shot:   gray --print "explain this repo"
  docs:       https://github.com/vstaln/gray

EOF
