#!/usr/bin/env bash
# Deploy cadre-api to the TEST box (64.95.13.106) from the Mac, read-only toward the stack:
#   services/cadre-api/deploy/install-test-box.sh [sync|install|restart|status]
#   sync     bundle main -> ~/cadre-api on the box (its own clone; ~/pearl-sol is untouched)
#   install  sync + root: apt caddy, ufw allow 443/tcp, Caddyfile, systemd unit; then start
#   restart  sync + restart the unit
#   status   unit status + /v1/ready through https
# Never the devnet boxes, never instance-backend (64.94.84.126).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
HOST="${VPS_HOST:-64.95.13.106}"
[ "$HOST" = "64.94.84.126" ] && { echo "refusing: instance-backend is not ours" >&2; exit 1; }
KEY="${VPS_KEY:-$HOME/.ssh/pearl_vps}"
SSH=(ssh -i "$KEY" -o IdentitiesOnly=yes -o ConnectTimeout=20)
NAME="cadre-api.$(echo "$HOST" | tr . -).sslip.io"
D="$ROOT/services/cadre-api/deploy"

sync() {
  local tmp; tmp="$(mktemp -d)"
  git -C "$ROOT" bundle create "$tmp/cadre-api.bundle" main 2>/dev/null
  scp -q -i "$KEY" -o IdentitiesOnly=yes "$tmp/cadre-api.bundle" "pearl@$HOST:cadre-api.bundle"
  rm -rf "$tmp"
  "${SSH[@]}" "pearl@$HOST" 'set -e; if [ -d ~/cadre-api/.git ]; then cd ~/cadre-api && git fetch -q ~/cadre-api.bundle main && git reset -q --hard FETCH_HEAD; else git clone -q -b main ~/cadre-api.bundle ~/cadre-api; fi; cd ~/cadre-api && git log --oneline -1'
}

case "${1:-status}" in
  sync) sync ;;
  install)
    sync
    scp -q -i "$KEY" -o IdentitiesOnly=yes "$D/test-box.env" "pearl@$HOST:cadre-api.env"
    sed "s/@HOST@/$NAME/" "$D/Caddyfile.tmpl" | "${SSH[@]}" "root@$HOST" 'cat > /tmp/Caddyfile.cadre'
    scp -q -i "$KEY" -o IdentitiesOnly=yes "$D/cadre-api.service" "root@$HOST:/etc/systemd/system/cadre-api.service"
    "${SSH[@]}" "root@$HOST" 'set -e
      command -v caddy >/dev/null || { DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq caddy; }
      mkdir -p /etc/caddy /var/log/caddy && chown caddy:caddy /var/log/caddy
      install -m 644 /tmp/Caddyfile.cadre /etc/caddy/Caddyfile
      ufw allow 443/tcp comment cadre-api >/dev/null
      systemctl daemon-reload
      systemctl enable --now cadre-api
      systemctl restart cadre-api
      caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
      systemctl enable caddy >/dev/null 2>&1; systemctl restart caddy
      ufw status | sed -n 1,12p'
    ;;
  restart) sync; "${SSH[@]}" "root@$HOST" 'systemctl restart cadre-api && systemctl is-active cadre-api' ;;
  status)
    "${SSH[@]}" "root@$HOST" 'systemctl is-active cadre-api caddy; journalctl -u cadre-api -n 5 --no-pager'
    curl -s -m 15 "https://$NAME/v1/ready" | head -c 600; echo
    ;;
  *) sed -n 2,9p "$0"; exit 1 ;;
esac
