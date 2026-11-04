#!/usr/bin/env bash
# Hourly devnet watcher for the Mac (launchd com.cadre.bitlaunch-watch). The three devnet boxes
# were deleted on 2026-11-01 because the prepaid BitLaunch balance ran out and nothing noticed;
# this checks the runway, that every devnet box still exists, and that the public names answer.
# Alerts are macOS notifications; every run appends one line to ~/Library/Logs/cadre-bitlaunch-watch.log.
#   scripts/bitlaunch-watch.sh          one check now
#   scripts/bitlaunch-watch.sh --test   fire a test notification
set -uo pipefail
TOKEN_FILE="$HOME/.config/bitlaunch/token"
LOG="$HOME/Library/Logs/cadre-bitlaunch-watch.log"
WARN_DAYS=14
FAIL_DAYS=5
BOXES="lode-devnet-0 lode-devnet-1 lode-devnet-2"
DOMAIN="${DEVNET_DOMAIN:-devnet.cadre.ag}"
URLS="https://$DOMAIN/ https://api.$DOMAIN/v1/summary https://rpc.$DOMAIN/"

notify() { osascript -e "display notification \"$2\" with title \"Cadre devnet: $1\" sound name \"Basso\"" >/dev/null 2>&1 || true; }
log() { echo "$(date -u +%FT%TZ) $*" >> "$LOG"; }

if [[ ${1:-} == --test ]]; then notify "test" "the watcher can reach you"; log "test notification"; exit 0; fi
[[ -r $TOKEN_FILE ]] || { notify "watcher broken" "no BitLaunch token at $TOKEN_FILE"; log "FAIL no token"; exit 1; }

api() { curl -s -m 20 -H "Authorization: Bearer $(cat "$TOKEN_FILE")" "https://app.bitlaunch.io/api/$1"; }
user="$(api user)"; servers="$(api servers)"
read -r balance hourly missing < <(python3 -c '
import json, sys
u = json.loads(sys.argv[1]); s = json.loads(sys.argv[2]); want = sys.argv[3].split()
names = {x["name"] for x in s}
# balance and rates are in thousandths of a dollar (rate per hour)
print(u.get("balance", 0), sum(x.get("rate", 0) for x in s), ",".join(n for n in want if n not in names) or "none")
' "$user" "$servers" "$BOXES" 2>/dev/null || echo "0 0 parse-error")

status=OK; msgs=()
if [[ $hourly -gt 0 ]]; then
  days=$(( balance / hourly / 24 ))
  if (( days < FAIL_DAYS )); then status=FAIL; msgs+=("balance \$$((balance / 1000)) lasts about $days days: top up now")
  elif (( days < WARN_DAYS )); then status=WARN; msgs+=("balance \$$((balance / 1000)) lasts about $days days")
  fi
else
  days=unknown; status=FAIL; msgs+=("could not read the BitLaunch account")
fi
[[ $missing != none ]] && { status=FAIL; msgs+=("server(s) gone: $missing"); }
for u in $URLS; do
  code=$(curl -s -o /dev/null -m 15 -w '%{http_code}' "$u")
  [[ $code =~ ^(2|3|4)[0-9][0-9]$ ]] || { [[ $status == OK ]] && status=WARN; msgs+=("$u answered $code"); }
done

log "$status balance=$balance hourly=$hourly days=$days missing=$missing ${msgs[*]:-}"
[[ $status != OK ]] && notify "$status" "${msgs[*]}"
exit 0
