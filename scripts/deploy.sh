#!/bin/sh
# Deploys a gray tarball to gray.alignment.id (nginx on oracle-new).
# usage: scripts/deploy.sh gray-<channel>-<plat>.tar.gz
#   e.g. scripts/deploy.sh gray-stable-x86_64-linux.tar.gz
# Env: DEPLOY_HOST (default opc@168.110.210.65), DEPLOY_KEY (path to ssh key).
set -eu

TARBALL="${1:?usage: deploy.sh gray-<channel>-<plat>.tar.gz}"
BASE=$(basename "$TARBALL")
case "$BASE" in
    gray-stable-*|gray-beta-*) ;; *) echo "tarball must be named gray-<stable|beta>-<plat>.tar.gz"; exit 1 ;;
esac

HOST="${DEPLOY_HOST:-opc@168.110.210.65}"
KEY="${DEPLOY_KEY:-$HOME/.ssh/oracle-new.key}"
REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)

SSH() { ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes "$HOST" "$@"; }
SCP() { scp -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes "$@"; }

echo "→ deploying $BASE to ${HOST}..."
SCP "$TARBALL" "$HOST:/tmp/$BASE"
SCP "${REPO_ROOT}/dist/install.sh" "$HOST:/tmp/gray-install.sh"
SSH "sudo install -m 644 /tmp/$BASE /var/www/gray/dl/$BASE \
     && sudo install -m 755 /tmp/gray-install.sh /var/www/gray/install.sh \
     && rm -f /tmp/$BASE /tmp/gray-install.sh"
echo "✓ live: https://gray.alignment.id/dl/$BASE"
