#!/usr/bin/env bash
# Cadre devnet health check (docs/DEVNET.md "Ops hardening"). Runs on box 0 as `lode` from
# cadre-health.timer every 5 minutes; also fine to run by hand. Writes only local files:
#   $LOG_DIR/health.log   one line per check per run (OK / WARN / FAIL), trimmed to the last 20000 lines
#   $LOG_DIR/status       one line: the time, the overall result, the failing checks
# No external messaging. Reads public endpoints (chain RPC over https, cadre-api, Solana RPC), this box's
# journal and local endpoints, and the OpenRouter credits endpoint (section 9).
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

# HEALTH_SECTIONS=logs runs only section 9 (scripts/test/devnet-health.test.sh); default all
if [[ ${HEALTH_SECTIONS:-all} == all ]]; then
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

# 8. house desktops (only where cadre-desktops runs): one WARN line per unhealthy desktop
#    (stream down or stale, browser host down, memory low, no action for 3 task gaps)
DESKTOPS_URL="${DESKTOPS_URL:-http://127.0.0.1:10720}"
if systemctl is-enabled --quiet cadre-desktops 2>/dev/null; then
  dh="$(curl -s -m 10 "$DESKTOPS_URL/health")"
  if [[ -z $dh ]]; then emit WARN desktops "controller did not answer"; else
    while IFS='|' read -r lvl check msg; do emit "$lvl" "$check" "$msg"; done < <(python3 -c '
import json, sys
d = json.loads(sys.argv[1])
if d.get("capped"): print("OK|desktops|paused at the day cap until 00:00 UTC")
elif not d.get("ok"): print("OK|desktops|stopped by the kill switch")
else:
    bad = [u for u in d.get("unhealthy", []) if u.get("state") == "running"]
    print(("WARN" if bad else "OK") + "|desktops|%d of %d running, %d streams alive, %d unhealthy" % (d.get("running", 0), d.get("desktops", 0), d.get("streams_alive", 0), len(bad)))
    for u in bad[:40]: print("WARN|desk:%s/%s|%s" % (u["agent"], u["role"], "; ".join(u["issues"])[:200]))
' "$dh" 2>/dev/null || echo "WARN|desktops|health answer unreadable")
  fi
fi

fi # sections 1-8

# 9. outages the checks above missed on 2026-10-22/23, each within about 10 minutes (one 5-minute
#    timer run looks back LOG_WINDOW): house desktops failing their tasks or out of model credit,
#    the OpenRouter balance, the gateway's request data backlog, hosted agents stuck on inference
#    payments, and the practice traders thinking. Reads this box's journal as lode (the units run as
#    lode) and local endpoints; the OpenRouter key is read from the desktops' providers file and
#    sent in a header from stdin, never logged and never on a command line.
JOURNAL="${JOURNAL:-journalctl}"
LOG_WINDOW="${LOG_WINDOW:-10min}"
unit_on() { "${SYSTEMCTL:-systemctl}" is-enabled --quiet "$1" 2>/dev/null; }
jlog() { "$JOURNAL" -u "$1" --since "-$2" --no-pager -o cat 2>/dev/null; }
DESK_TASK_MIN="${DESK_TASK_MIN:-10}"                # tasks in the window before a ratio means anything
DESK_FAIL_RATIO="${DESK_FAIL_RATIO:-0.5}"           # WARN at or above this share of failed tasks
MIN_OPENROUTER_USD="${MIN_OPENROUTER_USD:-5}"
OPENROUTER_CREDITS_URL="${OPENROUTER_CREDITS_URL:-https://openrouter.ai/api/v1/credits}"
DESKTOPS_ENV="${DESKTOPS_ENV:-/etc/lode/desktops.env}"
GATEWAY_STATS_URL="${GATEWAY_STATS_URL:-http://127.0.0.1:10664/v1/stats}"
MAX_REQUEST_DATA_GB="${MAX_REQUEST_DATA_GB:-16}"
AGENTS_PAY_MAX="${AGENTS_PAY_MAX:-20}"              # inference_hold + 402 lines per window before WARN
PRACTICE_THOUGHT_MAX_AGE="${PRACTICE_THOUGHT_MAX_AGE:-2h}"

# 9a. desktops: failed task share and model 402 (out of OpenRouter credit)
if unit_on cadre-desktops; then
  r="$(jlog cadre-desktops "$LOG_WINDOW" | python3 -c '
import json, sys
n = bad = m402 = 0
for line in sys.stdin:
    try: e = json.loads(line)
    except Exception:
        m402 += "model 402" in line; continue
    if e.get("event") != "task": continue
    n += 1; bad += e.get("ok") is False
    m402 += "model 402" in str(e.get("error", ""))
print(n, bad, m402)' 2>/dev/null)"
  read -r n bad m402 <<<"${r:-0 0 0}"
  lvl=OK
  (( m402 > 0 )) && lvl=FAIL
  if [[ $lvl == OK ]] && (( n >= DESK_TASK_MIN )) && python3 -c "import sys; sys.exit(0 if $bad / $n >= $DESK_FAIL_RATIO else 1)"; then lvl=WARN; fi
  emit "$lvl" desktop_tasks "$bad of $n tasks failed in $LOG_WINDOW, $m402 model 402 (out of model credit)"
fi

# 9b. OpenRouter balance (the desktops' model provider)
pf="$(sed -n 's/^PROVIDERS_ENV_FILE=//p' "$DESKTOPS_ENV" 2>/dev/null | tail -1 | tr -d "\"'")"
if [[ -n $pf ]]; then
  key="$(sed -n 's/^OPENROUTER_API_KEY=//p' "$pf" 2>/dev/null | tail -1 | tr -d "\"'")"
  if [[ -z $key ]]; then emit WARN openrouter "no OPENROUTER_API_KEY readable in the providers file"; else
    left="$(printf 'Authorization: Bearer %s\n' "$key" | curl -s -m 15 -H @- "$OPENROUTER_CREDITS_URL" | python3 -c '
import json, sys
d = json.load(sys.stdin)["data"]
print("%.2f" % (float(d["total_credits"]) - float(d["total_usage"])))' 2>/dev/null)"
    if [[ -z $left ]]; then emit WARN openrouter "credits endpoint did not answer"; else
      lvl=OK; python3 -c "import sys; sys.exit(0 if $left >= $MIN_OPENROUTER_USD else 1)" || lvl=WARN
      python3 -c "import sys; sys.exit(0 if $left > 0 else 1)" || lvl=FAIL
      emit "$lvl" openrouter "\$$left left (warn under \$$MIN_OPENROUTER_USD)"
    fi
  fi
  unset key
fi

# 9c. gateway request data: backlog over MAX_REQUEST_DATA_GB, or refused_storage rising since the last run
if unit_on cadre-gateway; then
  gs="$(curl -s -m 10 "$GATEWAY_STATS_URL" | python3 -c '
import json, sys
r = json.load(sys.stdin)["request_data"]
print(int(r["bytes_waiting"]), int(r.get("refused_storage", 0)), int(r.get("refused_node_storage", 0)))' 2>/dev/null)"
  if [[ -z $gs ]]; then emit WARN gateway_data "gateway stats did not answer"; else
    read -r waiting refused refused_node <<<"$gs"
    prev="$(cat "$STATE_DIR/gateway_refused" 2>/dev/null || echo "$refused $refused_node")"; read -r p1 p2 <<<"$prev"
    echo "$refused $refused_node" > "$STATE_DIR/gateway_refused"
    lvl=OK; (( waiting > MAX_REQUEST_DATA_GB * 1024 * 1024 * 1024 )) && lvl=WARN
    (( refused > p1 || refused_node > p2 )) && lvl=WARN
    emit "$lvl" gateway_data "$(python3 -c "print(f'{$waiting/2**30:.1f}')") GB waiting (max $MAX_REQUEST_DATA_GB), refused_storage $refused (was $p1), refused_node_storage $refused_node (was $p2)"
  fi
fi

# 9d. hosted agents: repeated inference holds and 402s (paid inference refused)
if unit_on cadre-agents; then
  r="$(jlog cadre-agents "$LOG_WINDOW" | python3 -c '
import json, sys
hold = p402 = 0; agents = set()
for line in sys.stdin:
    try: e = json.loads(line)
    except Exception: e = {}
    h = e.get("event") == "inference_hold"
    p = " 402" in line or "402:" in line
    if h or p: agents.add(e.get("agent", "?"))
    hold += h; p402 += p and not h
print(hold, p402, len(agents))' 2>/dev/null)"
  read -r hold p402 who <<<"${r:-0 0 0}"
  lvl=OK; (( hold + p402 > AGENTS_PAY_MAX )) && lvl=WARN
  emit "$lvl" agents_inference "$hold inference_hold + $p402 402 lines in $LOG_WINDOW from $who agents (max $AGENTS_PAY_MAX)"
fi

# 9e. practice traders: at least one paid thought in PRACTICE_THOUGHT_MAX_AGE
if unit_on cadre-practice; then
  n="$(jlog cadre-practice "$PRACTICE_THOUGHT_MAX_AGE" | grep -c '"event":"thought"')"
  lvl=OK; (( n == 0 )) && lvl=WARN
  emit "$lvl" practice_thought "$n thoughts in the last $PRACTICE_THOUGHT_MAX_AGE"
fi

overall=OK; for f in "${fails[@]:-}"; do [[ $f == *:FAIL ]] && overall=FAIL; [[ $f == *:WARN && $overall == OK ]] && overall=WARN; done
echo "$now $overall ${fails[*]:-}" > "$LOG_DIR/status"
tail -n 20000 "$LOG_DIR/health.log" > "$LOG_DIR/health.log.tmp" && mv "$LOG_DIR/health.log.tmp" "$LOG_DIR/health.log"
echo "$now $overall ${fails[*]:-}"
