#!/usr/bin/env bash
# Outside watcher (READINESS-PLAN D1): runs on a box OTHER than box 0 and checks that box 0's public
# endpoints answer, so a dead box 0 (whose own health check dies with it) still raises an alert.
# /etc/lode/watch.env: WATCH_URLS (space separated, each must return 2xx), ALERT_NTFY_URL, ALERT_REPEAT_S.
set -uo pipefail
STATE_DIR="${STATE_DIRECTORY:-/var/lib/cadre-watch}"; mkdir -p "$STATE_DIR"
[[ -n "${WATCH_URLS:-}" && -n "${ALERT_NTFY_URL:-}" ]] || { echo "WATCH_URLS or ALERT_NTFY_URL not set"; exit 0; }
bad=()
for u in $WATCH_URLS; do
  code="$(curl -s -o /dev/null -m 20 -w '%{http_code}' "$u" || true)"
  [[ $code == 2* ]] || bad+=("${u#https://}:$code")
done
cur="${bad[*]:-}"
last="$(cat "$STATE_DIR/alert-last" 2>/dev/null || true)"
prev="$(cat "$STATE_DIR/alert-prev" 2>/dev/null || true)"
age=$(( $(date +%s) - $(stat -c %Y "$STATE_DIR/alert-last" 2>/dev/null || echo 0) ))
msg=""
if [[ $cur != "$last" ]]; then
  if [[ -z $cur ]]; then msg="Recovered: box 0 endpoints answer again"
  elif [[ $cur == "$prev" ]]; then msg="Not answering: $cur"; fi
elif [[ -n $cur ]] && (( age >= ${ALERT_REPEAT_S:-21600} )); then msg="Still not answering: $cur"
fi
printf '%s' "$cur" > "$STATE_DIR/alert-prev"
if [[ -n $msg ]] && curl -fsS -m 15 -H "Title: Cadre devnet watcher ($(hostname))" -d "$msg" "$ALERT_NTFY_URL" >/dev/null 2>&1; then
  printf '%s' "$cur" > "$STATE_DIR/alert-last"
fi
echo "$(date -u +%FT%TZ) ${cur:-OK}"
