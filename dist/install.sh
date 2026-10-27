#!/bin/sh

set -eu

CHANNEL="stable"
SYSTEM=0
for arg in "$@"; do
    case "$arg" in
        stable) CHANNEL="stable" ;;
        beta|nightly) CHANNEL="beta" ;;
        --system) SYSTEM=1 ;;
        *) echo "unknown argument '$arg' (use: stable | beta | --system)"; exit 1 ;;
    esac
done

REPO_URL="${GRAY_CDN_URL:-https://gray.alignment.id/dl}"

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

echo "→ verifying checksum..."
if have_cmd curl; then
    curl -fsSL "${REPO_URL}/SHA256SUMS-${CHANNEL}" -o "${TMP}/SHA256SUMS"
elif have_cmd wget; then
    wget -qO "${TMP}/SHA256SUMS" "${REPO_URL}/SHA256SUMS-${CHANNEL}"
else
    echo "need curl or wget to download"; exit 1
fi
if have_cmd sha256sum; then
    ( cd "$TMP" && grep " ${TARBALL}\$" SHA256SUMS | sha256sum -c - )
elif have_cmd shasum; then
    ( cd "$TMP" && grep " ${TARBALL}\$" SHA256SUMS | shasum -a 256 -c - )
else
    echo "need sha256sum or shasum to verify download"; exit 1
fi || { echo "checksum mismatch for ${TARBALL} — refusing to install"; exit 1; }

tar xzf "${TMP}/${TARBALL}" -C "$TMP"

if [ -n "${GRAY_INSTALL_DIR:-}" ]; then
    DEST="${GRAY_INSTALL_DIR}"
elif [ "$SYSTEM" = "1" ] || [ "$(id -u)" = "0" ]; then
    DEST="/usr/local/bin"
else
    DEST="$HOME/.local/bin"
fi
mkdir -p "$DEST"

# An existing gray binary or home dir means this run is an update, not a
# first install — seeding plugins then re-downloads every default plugin
# and resurrects ones the user removed.
INSTALLED_BEFORE=0
if [ -e "${DEST}/gray" ] || [ -d "${GRAY_HOME:-$HOME/.gray}" ]; then
    INSTALLED_BEFORE=1
fi

mv "${TMP}/gray" "${DEST}/gray"
chmod +x "${DEST}/gray"

shadow="$HOME/.cargo/bin/gray"
if [ -e "$shadow" ]; then
    rm -f "$shadow"
    echo "removed stale $shadow (cargo-install shadow)"
fi

INSTALLED_VER=$("${DEST}/gray" --version 2>/dev/null || true)
echo "→ installed to ${DEST}/gray (${INSTALLED_VER:-?}, ${CHANNEL})"

FLAVOR="core"
SEED="${GRAY_INSTALL_PLUGINS:-auto}"
if [ "$SEED" = "auto" ]; then
    if [ "$INSTALLED_BEFORE" = "1" ] && [ -z "${GRAY_PLUGINS:-}" ]; then
        SEED=0
    else
        SEED=1
    fi
fi
if [ "$SEED" != "0" ]; then
    FLAVOR="full"
    DEFAULT_PLUGINS="account memory search gateway permissions"
    [ "$(uname -s)" = Linux ] && DEFAULT_PLUGINS="${DEFAULT_PLUGINS} fsearch"
    PLUGINS="${GRAY_PLUGINS:-$DEFAULT_PLUGINS}"
    FAILED=""
    set -f
    for plugin in $PLUGINS; do
        if [ "$plugin" = "fsearch" ] && {
            [ "$(uname -s)" != "Linux" ] || [ "$(uname -m)" != "x86_64" ]
        }; then
            echo "→ skipped fsearch (Linux x86_64 only)"
            continue
        fi
        if "${DEST}/gray" plugin install --yes "$plugin" >/dev/null 2>&1; then
            echo "→ installed ${plugin}"
            # installing the default set is the consent; grant declared
            # capabilities outright (new builds do this under --yes already;
            # keep it for older binaries, best-effort).
            "${DEST}/gray" plugin capabilities "$plugin" --all >/dev/null 2>&1 || true
        else
            FAILED="${FAILED} ${plugin}"
        fi
    done
    set +f
    if [ -n "$FAILED" ]; then
        echo "  some plugins could not be installed now -- retry later:"
        set -f
        for plugin in $FAILED; do
            echo "    gray plugin install --yes $plugin"
        done
        set +f
    fi
elif [ "$INSTALLED_BEFORE" = "1" ]; then
    FLAVOR="update"
fi

if [ "${GRAY_TELEMETRY:-1}" != "0" ] && [ -z "${GRAY_CDN_URL:-}" ]; then
    VER="${INSTALLED_VER##* }"
    if have_cmd curl; then
        ( curl -fsS -m 3 -X POST -H "Content-Type: application/json" \
            -d "{\"os\":\"${OS}\",\"arch\":\"${ARCH}\",\"channel\":\"${CHANNEL}\",\"version\":\"${VER}\",\"flavor\":\"${FLAVOR}\"}" \
            https://gray.alignment.id/api/telemetry/install >/dev/null 2>&1 || : ) &
    elif have_cmd wget; then
        ( wget -q -T 3 -O /dev/null --post-data \
            "{\"os\":\"${OS}\",\"arch\":\"${ARCH}\",\"channel\":\"${CHANNEL}\",\"version\":\"${VER}\",\"flavor\":\"${FLAVOR}\"}" \
            --header="Content-Type: application/json" \
            https://gray.alignment.id/api/telemetry/install 2>/dev/null || : ) &
    fi
fi

case ":$PATH:" in
    *":${DEST}:"*) ;;
    *)
        echo "⚠ ${DEST} is not in your PATH"
        LOGIN_SHELL="${SHELL:-sh}"
        case "${LOGIN_SHELL##*/}" in
            zsh) SHELL_RC="$HOME/.zshrc" ;;
            bash) SHELL_RC="$HOME/.bashrc" ;;
            fish) echo "  add it:  fish_add_path \"${DEST}\""; exit 0 ;;
            *) SHELL_RC="$HOME/.profile" ;;
        esac
        echo "  add it:  echo 'export PATH=\"${DEST}:\$PATH\"' >> ${SHELL_RC}"
        ;;
esac
