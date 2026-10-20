#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
tar cz crates Cargo.toml Cargo.lock | ssh maid 'rm -rf gray/crates && tar xz -C gray'
ssh maid 'cd gray && "$HOME/.cargo/bin/cargo" build --release -p gray'
src="/tmp/gray-maid.$$"
scp -q maid:gray/target/release/gray "$src"
dst="$HOME/.local/bin/gray"
tmp="$dst.tmp.$$"
trap 'rm -f "$tmp" "$src"' EXIT
cp "$src" "$tmp"
chmod 755 "$tmp"
mv "$tmp" "$dst"
trap - EXIT
echo "built on maid -> installed $dst"
shadow="$HOME/.cargo/bin/gray"
if [ -e "$shadow" ]; then
    rm -f "$shadow"
    echo "removed stale $shadow (cargo-install shadow; $dst is the only gray)"
fi
