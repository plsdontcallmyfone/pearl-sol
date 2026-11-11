#!/usr/bin/env bash
# One-time setup of the off-box backup copy (READINESS-PLAN B11), run from the Mac by the owner:
#   bash deploy/backup-push-setup.sh
# What it does:
#   1. box 0: makes an ssh key /root/.ssh/backup_push (no passphrase) if there is none.
#   2. box 2: makes a login "cadrebackup" whose only allowed key is that one, only from box 0's
#      address, with a forced command that stores the incoming stream under
#      /var/backups/cadre-box0/in (keeps the last 7) and prints its sha256. No shell, no forwarding.
#   3. box 0: installs deploy/backup-push.sh and hooks it after the daily cadre-backup.service.
#   4. pushes the newest backup once and lists the result on box 2.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
k0() { ssh -o ConnectTimeout=8 -i "$HOME/.ssh/lode_devnet0" "root@$BOX0_IP" "$@"; }
k2() { ssh -o ConnectTimeout=8 -i "$HOME/.ssh/lode_devnet2" "root@$BOX2_IP" "$@"; }

PUB="$(k0 'test -f /root/.ssh/backup_push || ssh-keygen -q -t ed25519 -N "" -C cadre-backup-push -f /root/.ssh/backup_push; cat /root/.ssh/backup_push.pub')"
echo "box 0 key type: $(echo "$PUB" | cut -d' ' -f1)"

k2 'set -e
id cadrebackup >/dev/null 2>&1 || useradd -r -m -d /var/backups/cadre-box0 -s /bin/sh cadrebackup
install -d -m 700 -o cadrebackup -g cadrebackup /var/backups/cadre-box0/in /var/backups/cadre-box0/.ssh
cat > /var/backups/cadre-box0/store.sh <<"EOS"
#!/bin/sh
umask 077
f="$HOME/in/cadre-box0-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
head -c 2147483648 > "$f"
ls -1t "$HOME"/in/*.tar.gz | tail -n +8 | xargs -r rm -f
sha256sum "$f" | cut -d" " -f1
EOS
chown root:root /var/backups/cadre-box0/store.sh; chmod 755 /var/backups/cadre-box0/store.sh
chmod 750 /var/backups/cadre-box0'

printf 'command="/var/backups/cadre-box0/store.sh",restrict,from="%s" %s\n' "$BOX0_IP" "$PUB" | k2 'cat > /var/backups/cadre-box0/.ssh/authorized_keys
chown cadrebackup:cadrebackup /var/backups/cadre-box0/.ssh/authorized_keys; chmod 600 /var/backups/cadre-box0/.ssh/authorized_keys; echo "box 2 ready"'

scp -q -i "$HOME/.ssh/lode_devnet0" "$ROOT/deploy/backup-push.sh" "root@$BOX0_IP:/usr/local/sbin/cadre-backup-push.sh"
k0 "set -e
echo 'BACKUP_PUSH_HOST=cadrebackup@$BOX2_IP' > /etc/lode/backup-push.env; chmod 640 /etc/lode/backup-push.env
mkdir -p /etc/systemd/system/cadre-backup.service.d
printf '[Service]\nEnvironmentFile=-/etc/lode/backup-push.env\nExecStartPost=/usr/local/sbin/cadre-backup-push.sh\n' > /etc/systemd/system/cadre-backup.service.d/push.conf
systemctl daemon-reload
BACKUP_PUSH_HOST=cadrebackup@$BOX2_IP /usr/local/sbin/cadre-backup-push.sh"
k2 'ls -la /var/backups/cadre-box0/in | tail -2'
