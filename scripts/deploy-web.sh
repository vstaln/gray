#!/bin/sh
# Deploys the static site (web/out) to gray.alignment.id (nginx on oracle-new).
# usage: scripts/deploy-web.sh
# Env: DEPLOY_HOST (default opc@168.110.210.65), DEPLOY_KEY (path to ssh key).
#
# The installer artifacts (install.sh, install.ps1, dl/) are served from the
# same docroot and are NOT part of the site build — every rsync below excludes
# them, so `gray update` and the curl|sh installers can never regress.
set -eu

HOST="${DEPLOY_HOST:-opc@168.110.210.65}"
KEY="${DEPLOY_KEY:-$HOME/.ssh/oracle-new.key}"
REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="${REPO_ROOT}/web/out"

SSH() { ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes "$HOST" "$@"; }

if [ ! -d "$OUT" ]; then
    echo "no build at $OUT — run: cd web && pnpm build"
    exit 1
fi

echo "→ deploying web/out to ${HOST}:/var/www/gray/ ..."
rsync -az --delete \
    -e "ssh -i $KEY -o StrictHostKeyChecking=accept-new -o BatchMode=yes" \
    --exclude 'dl/' --exclude 'install.sh' --exclude 'install.ps1' \
    --rsync-path='sudo rsync' \
    "$OUT/" "$HOST:/var/www/gray/"

# smoke-test the artifacts the site must never break
echo "→ verifying installer artifacts still served..."
SSH "head -1 /var/www/gray/install.sh" | grep -q '^#!/bin/sh' \
    || { echo "✗ install.sh missing or corrupted"; exit 1; }
SSH "test -d /var/www/gray/dl" \
    || { echo "✗ dl/ missing"; exit 1; }

echo "✓ live: https://gray.alignment.id"
echo "✓ installers intact: /install.sh /install.ps1 /dl/"
