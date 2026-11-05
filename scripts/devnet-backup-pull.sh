#!/usr/bin/env bash
# Pull the newest box backup (deploy/backup.sh, run daily by cadre-backup.timer on each box) to the
# Mac, so a deleted box still leaves its configs and service state here. Run by hand, or daily from a
# launchd job the owner installs (not installed by default).
# Lands in ~/pearl-sol/keys/devnet/rebuild-2026-11/box-backups (gitignored, 0600), keeps 14 per box.
set -uo pipefail
D="$HOME/pearl-sol/keys/devnet/rebuild-2026-11/box-backups"; LOG="$HOME/Library/Logs/cadre-backup-pull.log"
install -d -m 700 "$D"; i=0
for ip in 64.95.12.36 64.94.84.44 64.95.12.24; do
  K="$HOME/.ssh/lode_devnet$i"
  f=$(ssh -o BatchMode=yes -o ConnectTimeout=15 -i "$K" "root@$ip" 'ls -1t /var/backups/cadre/cadre-*.tar.gz 2>/dev/null | head -1')
  if [[ -n $f ]] && scp -q -o BatchMode=yes -i "$K" "root@$ip:$f" "$D/box$i-$(basename "$f")"; then
    chmod 600 "$D"/box$i-*; echo "$(date -u +%FT%TZ) OK box$i $(basename "$f")" >> "$LOG"
  else
    echo "$(date -u +%FT%TZ) FAIL box$i" >> "$LOG"
    osascript -e "display notification \"backup pull failed for box $i\" with title \"Cadre devnet\"" >/dev/null 2>&1 || true
  fi
  ls -1t "$D"/box$i-*.tar.gz 2>/dev/null | tail -n +15 | xargs rm -f
  i=$((i+1))
done
