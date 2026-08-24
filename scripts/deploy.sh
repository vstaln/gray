#!/bin/sh
# Deploys gray artifacts to gray.alignment.id (nginx on oracle-new).
# usage: scripts/deploy.sh <channel>            (uses pre-built tarball)
#        scripts/deploy.sh <channel> <tarball>  (deploys this tarball)
# Env: DEPLOY_HOST (default opc@168.110.210.65), DEPLOY_KEY (path to ssh key).
set -eu

CHANNEL="${1:?usage: deploy.sh <stable|beta> [tarball]}"
[ "$CHANNEL" = "stable" ] || [ "$CHANNEL" = "beta" ] || { echo "channel must be stable|beta"; exit 1; }
HOST="${DEPLOY_HOST:-opc@168.110.210.65}"
KEY="${DEPLOY_KEY:-$HOME/.ssh/ssh-key-2025-08-25.key}"
REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)

SSH() { ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes "$HOST" "$@"; }
SCP() { scp -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes "$@"; }

if [ $# -ge 2 ]; then TARBALL=$2; else
  TARBALL="${REPO_ROOT}/gray-${CHANNEL}-x86_64-linux.tar.gz"
  tar czf "$TARBALL" -C "${REPO_ROOT}/target/x86_64-unknown-linux-musl/release" gray
fi

echo "→ deploying ${TARBALL} to ${HOST} (${CHANNEL})..."
SCP "$TARBALL" "$HOST:/tmp/gray-${CHANNEL}.tar.gz"
SCP "${REPO_ROOT}/dist/install.sh" "$HOST:/tmp/gray-install.sh"
SSH "sudo install -m 644 /tmp/gray-${CHANNEL}.tar.gz /var/www/gray/dl/gray-${CHANNEL}-x86_64-linux.tar.gz \
     && sudo install -m 755 /tmp/gray-install.sh /var/www/gray/install.sh \
     && rm -f /tmp/gray-${CHANNEL}.tar.gz /tmp/gray-install.sh"
echo "✓ live: https://gray.alignment.id/dl/gray-${CHANNEL}-x86_64-linux.tar.gz"
