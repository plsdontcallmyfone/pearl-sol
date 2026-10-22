#!/bin/bash
# Read-only RPC load probe for a devnet box (run as root ON the box): agent-node CPU over the
# window (pidstat), getChainInfo latency (one call every 2 s), RPC calls by method and by client
# process (tcpdump on lo + ss), and getNodeStats when the node has it. Used for the 2026-10-22
# job index rollout (HANDOFF item 40). Usage: devnet-rpc-load.sh [seconds, default 300]
set -u
DUR=${1:-300}
RPC=http://127.0.0.1:10630
PID=$(systemctl show -p MainPID --value cadre-node)
T=$(mktemp -d)
echo "date $(date -u +%FT%TZ) node_pid $PID window ${DUR}s"
rpc() { curl -s -m 10 -X POST -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\"}" $RPC; }
rpc getChainInfo | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("height", r["height"], "finalized", r["finalized_height"], "peers", r["peers"], "mempool", r["mempool_size"])'
timeout "$DUR" tcpdump -i lo -nn -A -s 0 -l "tcp dst port 10630" 2>/dev/null | awk '
/ > 127.0.0.1.10630:/ { split($3,a,"."); port=a[5] }
{ while (match($0, /"method": *"[A-Za-z]+"/)) { m=substr($0, RSTART, RLENGTH); gsub(/"method": *|"/,"",m); print port, m; $0=substr($0, RSTART+RLENGTH) } }' > "$T/pm" &
pidstat -p "$PID" -u "$DUR" 1 | tail -1 > "$T/cpu" &
END=$(( $(date +%s) + DUR ))
: > "$T/ss"; : > "$T/lat"; n=0
while [ "$(date +%s)" -lt "$END" ]; do
  ss -tnpH "( dport = :10630 )" >> "$T/ss"
  if [ $((n % 10)) -eq 0 ]; then
    curl -s -o /dev/null -m 10 -w '%{time_total}\n' -X POST -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getChainInfo"}' $RPC >> "$T/lat"
  fi
  n=$((n + 1)); sleep 0.2
done
wait
echo "agent-node CPU (pidstat average: %usr %system %guest %wait %CPU):"; awk '{print "  ", $4, $5, $6, $7, $8}' "$T/cpu"
sort -n "$T/lat" | awk '{a[NR]=$1} END {printf "getChainInfo latency s: n=%d p50 %s p90 %s max %s\n", NR, a[int(NR*0.5)+1], a[int(NR*0.9)+1], a[NR]}'
echo "RPC calls per minute by method:"
awk -v d="$DUR" '{c[$2]++} END {for (m in c) printf "  %7.1f %s\n", c[m]*60/d, m}' "$T/pm" | sort -rn | head -20
echo "RPC calls per minute by client and method (top 20):"
awk -v d="$DUR" 'NR==FNR { for(i=1;i<=NF;i++) if ($i ~ /^127\.0\.0\.1:[0-9]+$/ && $i !~ /:10630$/) { split($i,a,":"); p=a[2] } ; if (match($0,/users:\(\("[^"]+",pid=[0-9]+/)) { u=substr($0,RSTART+9,RLENGTH-9); proc[p]=u }; next }
     { k=(($1 in proc)?proc[$1]:"unmapped") " " $2; c[k]++ } END { for (k in c) printf "  %7.1f %s\n", c[k]*60/d, k }' "$T/ss" "$T/pm" | sort -rn | head -20
for p in $(grep -o 'pid=[0-9]*' "$T/ss" | cut -d= -f2 | sort -u); do echo "  pid $p = $(systemctl status "$p" 2>/dev/null | head -1 | awk '{print $2}')"; done
rpc getNodeStats | python3 -c '
import json,sys
r=json.load(sys.stdin)
if "result" not in r: print("getNodeStats: not on this node build"); sys.exit()
s=r["result"]; print("getNodeStats: uptime_ms", s["uptime_ms"], "state_cache", s["state_cache"], "job_index", s["job_index"], "mempool", s["mempool"])
for m,v in list(s["rpc"].items())[:8]: print("  ", m, v)'
rm -rf "$T"
