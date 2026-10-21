#!/usr/bin/env bash
# Cadre devnet health check (docs/DEVNET.md "Ops hardening"). Runs on box 0 as `lode` from
# cadre-health.timer every 5 minutes; also fine to run by hand. Writes only local files:
#   $LOG_DIR/health.log   one line per check per run (OK / WARN / FAIL), trimmed to the last 20000 lines
#   $LOG_DIR/status       one line: the time, the overall result, the failing checks
# No external messaging. Reads public endpoints only (chain RPC over https, cadre-api, Solana RPC).
# Optional overrides in /etc/lode/health.env (NODES, API, RELAY_PAYERS, SITES, thresholds).
set -uo pipefail
LOG_DIR="${LOGS_DIRECTORY:-${LOG_DIR:-/var/log/cadre-health}}"
STATE_DIR="${STATE_DIRECTORY:-${STATE_DIR:-/var/lib/cadre-health}}"
mkdir -p "$LOG_DIR" "$STATE_DIR"
NODES="${NODES:-https://rpc.64-95-12-116.sslip.io https://rpc.64-95-13-213.sslip.io https://rpc.64-95-12-22.sslip.io}"
API="${API:-https://api.64-95-12-116.sslip.io/v1}"
# header-relay fee payers (public keys, docs/DEVNET.md "Keys on the boxes")
RELAY_PAYERS="${RELAY_PAYERS:-XQUUCs5tZaJrf4VMuKJRmUbA2gEV2M7uKWTQiSTe6Cy E66ekRjfD9tjquu5F4qyRHt3y7AZhM9hsXY63KV4Ug7L}"
SITES="${SITES:-rpc.64-95-12-116.sslip.io rpc.64-95-13-213.sslip.io rpc.64-95-12-22.sslip.io api.64-95-12-116.sslip.io explorer.64-95-12-116.sslip.io storefront.64-95-12-116.sslip.io relayer.64-95-12-116.sslip.io cadre.64-95-12-116.sslip.io}"
MAX_HEIGHT_SPREAD="${MAX_HEIGHT_SPREAD:-3}"      # blocks between the highest and lowest node
MAX_LC_BEHIND_FINAL="${MAX_LC_BEHIND_FINAL:-120}" # light client final vs chain tip, blocks
MIN_RELAY_SOL="${MIN_RELAY_SOL:-1}"               # relay payer balance floor, SOL
MIN_DISK_FREE_PCT="${MIN_DISK_FREE_PCT:-10}"
MIN_CERT_DAYS="${MIN_CERT_DAYS:-14}"              # caddy renews at 30 days left; under 14 means renewals fail
SOLANA_RPC="${SOLANA_RPC:-$(cat /etc/lode/solana-rpc-url 2>/dev/null || echo https://api.devnet.solana.com)}"

now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
fails=()
emit() { # level check message
  echo "$now $1 $2 $3" >> "$LOG_DIR/health.log"
  [[ $1 != OK ]] && fails+=("$2:$1")
}
rpc() { curl -s -m 10 -X POST "$1" -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":${3:-{\}}}"; }

# 1. node heights: every node answers, heights agree, the tip moved since the last run
heights=()
for u in $NODES; do
  r="$(rpc "$u" getChainInfo | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r["height"], r["finalized_height"], r["peers"])' 2>/dev/null)"
  name="${u#https://}"; name="${name#rpc.}"; name="${name%.sslip.io}"
  if [[ -z $r ]]; then emit FAIL "node:$name" "no answer"; continue; fi
  read -r h f p <<<"$r"; heights+=("$h")
  lvl=OK; [[ $p -lt 1 ]] && lvl=WARN
  emit "$lvl" "node:$name" "height $h final $f peers $p"
done
if [[ ${#heights[@]} -gt 0 ]]; then
  hi="$(printf '%s\n' "${heights[@]}" | sort -n | tail -1)"; lo="$(printf '%s\n' "${heights[@]}" | sort -n | head -1)"
  lvl=OK; (( hi - lo > MAX_HEIGHT_SPREAD )) && lvl=WARN
  emit "$lvl" agree "spread $((hi - lo)) (max $MAX_HEIGHT_SPREAD)"
  prev="$(cat "$STATE_DIR/last_height" 2>/dev/null || echo 0)"
  lvl=OK; (( hi <= prev )) && lvl=FAIL
  emit "$lvl" advancing "tip $hi, last run $prev"
  echo "$hi" > "$STATE_DIR/last_height"
fi

# 2. light client lag (cadre-api summary)
lc="$(curl -s -m 15 "$API/summary" | python3 -c 'import json,sys; d=json.load(sys.stdin)["light_client_lag"]; print(d["behind_final"], d["final_height"], d["chain_height"])' 2>/dev/null)"
if [[ -z $lc ]]; then emit FAIL light_client "cadre-api summary did not answer"; else
  read -r bf lf ch <<<"$lc"; lvl=OK; (( bf > MAX_LC_BEHIND_FINAL )) && lvl=WARN
  emit "$lvl" light_client "final $lf chain $ch behind $bf (max $MAX_LC_BEHIND_FINAL)"
fi

# 3. relay payer balances (Solana devnet)
for k in $RELAY_PAYERS; do
  lam="$(curl -s -m 10 -X POST "$SOLANA_RPC" -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBalance\",\"params\":[\"$k\"]}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["value"])' 2>/dev/null)"
  if [[ -z $lam ]]; then emit WARN "relay_sol:${k:0:6}" "balance read failed"; continue; fi
  sol="$(python3 -c "print(f'{$lam/1e9:.4f}')")"; lvl=OK
  python3 -c "import sys; sys.exit(0 if $lam/1e9 >= $MIN_RELAY_SOL else 1)" || lvl=WARN
  emit "$lvl" "relay_sol:${k:0:6}" "$sol SOL (min $MIN_RELAY_SOL)"
done

# 4. local units (enabled cadre-* units must be active)
for u in $(systemctl list-unit-files --plain --no-legend 'cadre-*.service' | awk '$2=="enabled"{print $1}'); do
  s="$(systemctl is-active "$u")"; lvl=OK; [[ $s != active ]] && lvl=FAIL
  [[ $lvl != OK ]] && emit "$lvl" "unit:${u%.service}" "$s"
done
emit OK units "checked $(systemctl list-unit-files --plain --no-legend 'cadre-*.service' | awk '$2=="enabled"' | wc -l) enabled cadre units"

# 5. disk
free_pct="$(df -P / | awk 'NR==2{gsub("%","",$5); print 100-$5}')"
lvl=OK; (( free_pct < MIN_DISK_FREE_PCT )) && lvl=WARN
emit "$lvl" disk "${free_pct}% free on /"

# 6. certificates
for s in $SITES; do
  end="$(echo | timeout 10 openssl s_client -servername "$s" -connect "$s:443" 2>/dev/null | openssl x509 -noout -enddate 2>/dev/null | cut -d= -f2)"
  if [[ -z $end ]]; then emit FAIL "cert:${s%.sslip.io}" "no certificate read"; continue; fi
  days=$(( ($(date -d "$end" +%s) - $(date +%s)) / 86400 )); lvl=OK; (( days < MIN_CERT_DAYS )) && lvl=WARN
  emit "$lvl" "cert:${s%.sslip.io}" "$days days left"
done

# 7. the Cadre CLI's checks when its binary is installed (docs/CLI.md "status"): finalized-hash
# agreement, programs, services' own health, verifiable AI, agents and desktops, bridge invariant,
# settle, box-local services (--local). Each line is `OK|WARN|FAIL cli:<check> <detail>`.
CADRE_CLI="${CADRE_CLI:-/opt/lode/bin/cadre}"
if [[ -x $CADRE_CLI ]]; then
  while read -r lvl name msg; do
    [[ $lvl == OK || $lvl == WARN || $lvl == FAIL ]] && emit "$lvl" "$name" "$msg"
  done < <(timeout 90 "$CADRE_CLI" status --health --local --network devnet 2>/dev/null)
fi

overall=OK; for f in "${fails[@]:-}"; do [[ $f == *:FAIL ]] && overall=FAIL; [[ $f == *:WARN && $overall == OK ]] && overall=WARN; done
echo "$now $overall ${fails[*]:-}" > "$LOG_DIR/status"
tail -n 20000 "$LOG_DIR/health.log" > "$LOG_DIR/health.log.tmp" && mv "$LOG_DIR/health.log.tmp" "$LOG_DIR/health.log"
echo "$now $overall ${fails[*]:-}"
