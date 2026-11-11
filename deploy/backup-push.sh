#!/usr/bin/env bash
# Off-box copy of the newest backup (READINESS-PLAN B11): streams it to BACKUP_PUSH_HOST over a
# write-only ssh key (the far side has a forced command that only stores the stream and keeps 7).
# Run as root on box 0 after deploy/backup.sh; /etc/lode/backup-push.env: BACKUP_PUSH_HOST=user@ip.
set -euo pipefail
[[ -n "${BACKUP_PUSH_HOST:-}" ]] || { echo "BACKUP_PUSH_HOST not set: backup stays on this box"; exit 0; }
f="$(ls -1t /var/backups/cadre/cadre-*.tar.gz | head -1)"
want="$(sha256sum "$f" | cut -d' ' -f1)"
got="$(ssh -i /root/.ssh/backup_push -o BatchMode=yes -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new "$BACKUP_PUSH_HOST" < "$f")"
[[ $got == "$want" ]] || { echo "off-box copy FAILED: far side reported '$got'"; exit 1; }
echo "off-box copy ok: $(basename "$f") sha256 $want"
