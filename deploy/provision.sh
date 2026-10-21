#!/usr/bin/env bash
# Fresh Ubuntu 24.04 box -> Lode public devnet host (docs/DEVNET.md). Run as root ON the box:
#   ssh root@BOX 'bash -s' < deploy/provision.sh [build]
# Creates user `lode` (services run as it), 4 GB swap, ufw (22, 80, 443, 10640 p2p),
# caddy (https via Let's Encrypt for the sslip.io names), key-only sshd, unattended security updates. With `build` it also installs
# apt build deps; toolchains (rust, solana, anchor, node) go in as `lode` via deploy/toolchain.sh.
# Idempotent. Copies nothing from anywhere: keys arrive later, one file per service.
set -euo pipefail
ROLE="${1:-node}"
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q ufw curl jq lsof debian-keyring debian-archive-keyring apt-transport-https ca-certificates gnupg
if [[ $ROLE == build ]]; then
  apt-get install -y -q build-essential pkg-config libssl-dev libudev-dev clang cmake git protobuf-compiler python3 unzip
fi
id lode >/dev/null 2>&1 || useradd -m -s /bin/bash lode
install -d -o lode -g lode -m 700 /home/lode/.ssh
cp /root/.ssh/authorized_keys /home/lode/.ssh/authorized_keys && chown lode:lode /home/lode/.ssh/authorized_keys && chmod 600 /home/lode/.ssh/authorized_keys
install -d -o lode -g lode -m 755 /opt/lode /opt/lode/bin
install -d -o lode -g lode -m 700 /etc/lode /etc/lode/keys /var/lib/lode
# swap: builds and a node restart (all blocks in memory) can spike
if ! swapon --show | grep -q /swapfile; then
  fallocate -l 4G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
  grep -q '^/swapfile' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi
# caddy from its official apt repo
if ! command -v caddy >/dev/null; then
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
  apt-get update -q && apt-get install -y -q caddy
fi
ufw default deny incoming >/dev/null
ufw default allow outgoing >/dev/null
for p in 22/tcp 80/tcp 443/tcp 10640/tcp; do ufw allow "$p" >/dev/null; done
ufw --force enable >/dev/null
# ssh: key only (00- wins over 50-cloud-init.conf, which sets PasswordAuthentication yes on BitLaunch images)
cat > /etc/ssh/sshd_config.d/00-cadre-hardening.conf <<'SSHD'
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
PermitEmptyPasswords no
X11Forwarding no
MaxAuthTries 3
LoginGraceTime 30
SSHD
sshd -t && systemctl reload ssh.service 2>/dev/null || true
# security updates daily, never an automatic reboot (reboots happen in announced windows)
apt-get install -y -q unattended-upgrades >/dev/null
cat > /etc/apt/apt.conf.d/52cadre-unattended <<'APT'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
Unattended-Upgrade::Automatic-Reboot "false";
Unattended-Upgrade::Remove-Unused-Kernel-Packages "true";
APT
# journald: bounded logs
mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\nSystemMaxUse=2G\n' > /etc/systemd/journald.conf.d/lode.conf
systemctl restart systemd-journald
echo "provisioned ($ROLE): $(lsb_release -ds), $(nproc) cpu, $(free -g | awk '/Mem/{print $2}') GB, ufw: $(ufw status | grep -c ALLOW) rules"
