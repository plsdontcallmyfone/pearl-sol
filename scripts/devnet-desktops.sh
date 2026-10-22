#!/usr/bin/env bash
# House desktops on devnet box 0 (services/swarm/desktop, cadre-desktops.service).
#   scripts/devnet-desktops.sh secrets     E2B + OpenRouter key files -> /etc/lode/keys/desktops (0600, over ssh
#                                          stdin, never printed); the stream token secret (made on the box);
#                                          /etc/lode/desktops.env with the paths and the knobs below
#   scripts/devnet-desktops.sh wallet      pack ~/pearl-wallet/dist (built from dev, no source maps) -> /var/lib/lode/desktops/wallet.tgz
#   scripts/devnet-desktops.sh code        this checkout's committed main -> box 0 ~/cadre-desktops (its own clone, not
#                                          ~/pearl-sol), then npm ci in services/swarm/desktop
#   scripts/devnet-desktops.sh unit        install + enable cadre-desktops.service (not started)
#   scripts/devnet-desktops.sh status      the controller's /status (spend, measured cost, desktops)
#   scripts/devnet-desktops.sh stop        KILL SWITCH: STOP flag + pause every house desktop (billing stops)
#   scripts/devnet-desktops.sh resume      remove the STOP flag (then `devnet-deploy.sh ctl 0 restart cadre-desktops`)
# Knobs (env when running `secrets`; defaults = Phase A, docs/DESKTOPS-AT-SCALE.md 9): DESK_AGENTS (30),
# DESK_TWO_DESK_AGENTS (6), DESK_USD_PER_DAY (64), DESK_SOCIAL (1), DESK_BROWSE_GAP_S / DESK_WORK_GAP_S (1200),
# stream DESK_FPS (15) DESK_KBPS (800) DESK_SEG_S (1) DESK_STILL_S (1); the desktop agents' own swarm budget
# (their share of the house's 16384 rows a day: all 30 = 16384). cadre-swarm must run SWARM_AGENTS=30-DESK_AGENTS
# (/etc/lode/swarm.env) so no persona is driven twice. Sources: E2B_ENV_FILE (~/.config/lineage/e2b.env),
# PROVIDERS_ENV_FILE (~/.config/lineage/providers.env, OPENROUTER_API_KEY).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
sshb() { local u=$1; shift; ssh -i "$HOME/.ssh/lode_devnet0" -o IdentitiesOnly=yes -o ConnectTimeout=20 "$u@$BOX0_IP" "$@"; }
put() { # put <local file> <remote path> (root writes it 0600 lode)
  sshb root "umask 077; cat > '$2.new' && chown lode:lode '$2.new' && chmod 600 '$2.new' && mv -f '$2.new' '$2'" < "$1"
}
case "${1:-}" in
  secrets)
    e2b="${E2B_ENV_FILE:-$HOME/.config/lineage/e2b.env}"; prov="${PROVIDERS_ENV_FILE:-$HOME/.config/lineage/providers.env}"
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    grep -E '^E2B_API_KEY=' "$e2b" > "$tmp/e2b.env"
    grep -E '^OPENROUTER_API_KEY=' "$prov" > "$tmp/providers.env"
    sshb root 'install -d -m 700 -o lode -g lode /etc/lode/keys/desktops /var/lib/lode/desktops'
    put "$tmp/e2b.env" /etc/lode/keys/desktops/e2b.env
    put "$tmp/providers.env" /etc/lode/keys/desktops/providers.env
    sshb root 'test -s /etc/lode/keys/desktops-stream.key || (umask 077; head -c 32 /dev/urandom > /etc/lode/keys/desktops-stream.key; chown lode:lode /etc/lode/keys/desktops-stream.key)'
    cat > "$tmp/desktops.env" <<ENV
E2B_ENV_FILE=/etc/lode/keys/desktops/e2b.env
PROVIDERS_ENV_FILE=/etc/lode/keys/desktops/providers.env
DESK_AGENTS=${DESK_AGENTS:-30}
DESK_TWO_DESK_AGENTS=${DESK_TWO_DESK_AGENTS:-6}
DESK_USD_PER_DAY=${DESK_USD_PER_DAY:-64}
DESK_FPS=${DESK_FPS:-15}
DESK_KBPS=${DESK_KBPS:-800}
DESK_SEG_S=${DESK_SEG_S:-1}
DESK_STILL_S=${DESK_STILL_S:-1}
DESK_WORK_TEMPLATE=${DESK_WORK_TEMPLATE:-cadre-desktop-1536}
DESK_WORK_MEM_MB=${DESK_WORK_MEM_MB:-1536}
DESK_SOCIAL=${DESK_SOCIAL:-1}
DESK_BROWSE_GAP_S=${DESK_BROWSE_GAP_S:-1200}
DESK_WORK_GAP_S=${DESK_WORK_GAP_S:-1200}
DESK_SITE_URL=https://cadre.${BOX0_IP//./-}.sslip.io
DESK_EXPLORER_URL=https://cadre.${BOX0_IP//./-}.sslip.io/explorer
SWARM_ROWS_PER_DAY=${SWARM_ROWS_PER_DAY:-16384}
SWARM_TOPUP_ROWS=8192
SWARM_TOPUP_ROUND_SECS=10800
SWARM_TOPUP_LAMPORTS_PER_DAY=${SWARM_TOPUP_LAMPORTS_PER_DAY:-500000000}
SWARM_TRANSFER_PER_DAY=${SWARM_TRANSFER_PER_DAY:-2000000000}
ENV
    put "$tmp/desktops.env" /etc/lode/desktops.env
    echo "box 0: desktop secrets, stream key and /etc/lode/desktops.env installed"
    ;;
  wallet)
    dist="${WALLET_DIST:-$HOME/pearl-wallet/dist}"; tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    tar -C "$dist" --exclude='*.map' -czf "$tmp/wallet.tgz" .
    sshb lode 'cat > /var/lib/lode/desktops/wallet.tgz' < "$tmp/wallet.tgz"
    echo "box 0: wallet.tgz $(wc -c < "$tmp/wallet.tgz" | tr -d ' ') bytes (from $(git -C "$dist/.." log --oneline -1 2>/dev/null || echo "$dist"))"
    ;;
  code)
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    git -C "$ROOT" bundle create "$tmp/desk.bundle" main 2>/dev/null
    sshb lode 'cat > ~/desk.bundle' < "$tmp/desk.bundle"
    sshb lode 'set -e; if [ -d ~/cadre-desktops/.git ]; then cd ~/cadre-desktops && git pull -q --ff-only ~/desk.bundle main; else git clone -q -b main ~/desk.bundle ~/cadre-desktops; fi
      cd ~/cadre-desktops && git log --oneline -1 && cd services/swarm/desktop && PATH=$HOME/.local/node/bin:$PATH npm ci --silent && echo deps ok' ;;
  unit)
    sshb root 'cat > /etc/systemd/system/cadre-desktops.service && systemctl daemon-reload && systemctl enable cadre-desktops' < "$ROOT/deploy/systemd/cadre-desktops.service"
    echo "box 0: cadre-desktops.service installed and enabled (not started)"
    ;;
  status) sshb lode 'curl -s 127.0.0.1:10720/status' | python3 -c 'import json,sys; d=json.load(sys.stdin); d["agents"]=[{k:a[k] for k in ("handle","desktops","spend_today")} for a in d["agents"]]; print(json.dumps(d, indent=1))' ;;
  stop) sshb lode 'cd ~/cadre-desktops/services/swarm/desktop && set -a && . /etc/lode/desktops.env && set +a && CADRE_AGENTS_ENV=/etc/lode/desktops.env DESK_KILL_FILE=/var/lib/lode/desktops/STOP ~/.local/node/bin/node src/main.ts stop' ;;
  resume) sshb lode 'rm -f /var/lib/lode/desktops/STOP && echo "STOP flag removed"' ;;
  *) sed -n 2,16p "$0"; exit 1 ;;
esac
