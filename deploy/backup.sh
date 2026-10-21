#!/usr/bin/env bash
# Cadre devnet box backup (docs/DEVNET.md "Ops hardening: backups"). Run as root ON the box:
#   bash deploy/backup.sh            -> /var/backups/cadre/cadre-<host>-<UTC time>.tar.gz (0600), keeps the last 7
# Contents: /etc/lode WITHOUT keys/ (configs, env files, node.json; node.json holds the p2p seed, so the
# tarball stays 0600 root and on the box), service state under /var/lib/lode WITHOUT the chain data,
# model files, gateway request scratch and results (the chain resyncs from peers; models are re-uploadable),
# the systemd units + drop-ins and the Caddyfile. Keys under /etc/lode/keys are NOT included: the owner
# copies them offline by hand (docs/DEVNET.md), nothing here uploads anything anywhere.
set -euo pipefail
out=/var/backups/cadre; install -d -m 700 "$out"
f="$out/cadre-$(hostname)-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
umask 077
tar -czf "$f" --ignore-failed-read \
  --exclude=/etc/lode/keys \
  --exclude=/var/lib/lode/chain --exclude=/var/lib/lode/cadre-model --exclude=/var/lib/lode/models \
  --exclude=/var/lib/lode/gateway/requests --exclude=/var/lib/lode/results \
  --exclude='/var/lib/lode/header-relay-work*.json' --exclude='/var/lib/lode/swarm/keys' \
  /etc/lode /var/lib/lode /etc/systemd/system/cadre-* /etc/caddy/Caddyfile 2>/dev/null
chmod 600 "$f"
ls -1t "$out"/cadre-*.tar.gz | tail -n +8 | xargs -r rm -f
echo "$f $(du -h "$f" | cut -f1)"
tar -tzf "$f" | { grep -cE '(^|/)keys/|keypair|work.*\.json$' || true; } | sed 's/^/key-like entries (must be 0): /'
