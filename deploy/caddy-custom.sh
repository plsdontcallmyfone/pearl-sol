#!/usr/bin/env bash
# Run as root on a box: swap the apt caddy for a build that includes the rate limit module
# (github.com/mholt/caddy-ratelimit), following Caddy's documented diversion so apt upgrades do not
# replace it, and give the unit a runtime directory for the admin unix socket. Idempotent.
# The apt binary stays at /usr/bin/caddy.default; `update-alternatives --config caddy` switches back.
set -euo pipefail
if ! caddy list-modules 2>/dev/null | grep -q '^http.handlers.rate_limit$'; then
  tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
  arch="$(dpkg --print-architecture)"
  curl -fsSL -o "$tmp/caddy" "https://caddyserver.com/api/download?os=linux&arch=$arch&p=github.com%2Fmholt%2Fcaddy-ratelimit"
  chmod 755 "$tmp/caddy"
  "$tmp/caddy" list-modules | grep -q '^http.handlers.rate_limit$' || { echo "downloaded caddy lacks rate_limit" >&2; exit 1; }
  [ -e /usr/bin/caddy.default ] || dpkg-divert --divert /usr/bin/caddy.default --rename /usr/bin/caddy
  install -m 755 "$tmp/caddy" /usr/bin/caddy.custom
  update-alternatives --install /usr/bin/caddy caddy /usr/bin/caddy.default 10 >/dev/null
  update-alternatives --install /usr/bin/caddy caddy /usr/bin/caddy.custom 50 >/dev/null
fi
install -d /etc/systemd/system/caddy.service.d
printf '[Service]\nRuntimeDirectory=caddy\nRuntimeDirectoryMode=0750\n' > /etc/systemd/system/caddy.service.d/runtime.conf
systemctl daemon-reload
caddy version
