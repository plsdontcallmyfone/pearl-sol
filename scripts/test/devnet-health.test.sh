#!/usr/bin/env bash
# Tests for section 9 of scripts/devnet-health.sh (journal, OpenRouter, gateway, agents, practice)
# with stub journalctl/systemctl and file:// endpoints; no box, no network:
#   bash scripts/test/devnet-health.test.sh
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; SCRIPT="$HERE/../devnet-health.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fails=0
check() { if eval "$2"; then echo "PASS $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }
mkdir -p "$T/j" "$T/bin"
# stub journalctl: prints $T/j/<unit>.log for -u <unit>
cat > "$T/bin/journalctl" <<EOF
#!/usr/bin/env bash
while [[ \$# -gt 0 ]]; do [[ \$1 == -u ]] && { cat "$T/j/\$2.log" 2>/dev/null; exit 0; }; shift; done
EOF
# stub systemctl: every unit enabled
printf '#!/usr/bin/env bash\nexit 0\n' > "$T/bin/systemctl"
chmod +x "$T/bin/"*
SECRET="sk-or-test-$(date +%s)"
echo "PROVIDERS_ENV_FILE=$T/providers.env" > "$T/desktops.env"
echo "OPENROUTER_API_KEY=$SECRET" > "$T/providers.env"
run() {
  HEALTH_SECTIONS=logs LOG_DIR="$T/log" STATE_DIR="$T/state" JOURNAL="$T/bin/journalctl" SYSTEMCTL="$T/bin/systemctl" \
    DESKTOPS_ENV="$T/desktops.env" OPENROUTER_CREDITS_URL="file://$T/credits.json" GATEWAY_STATS_URL="file://$T/stats.json" \
    bash "$SCRIPT" > "$T/out" 2>&1
}
line() { grep " $1 " "$T/log/health.log" | tail -1; }
task() { echo "{\"t\":\"x\",\"event\":\"task\",\"agent\":\"a$1\",\"ok\":$2,\"error\":\"$3\"}"; }

# --- healthy: few failures, credit left, small backlog, quiet agents, a practice thought
{ for i in $(seq 1 12); do task "$i" true ""; done; task 13 false "timeout"; } > "$T/j/cadre-desktops.log"
echo '{"data":{"total_credits":50,"total_usage":12.5}}' > "$T/credits.json"
echo '{"request_data":{"bytes_waiting":1073741824,"refused_storage":3,"refused_node_storage":0}}' > "$T/stats.json"
echo '{"event":"thought","agent":"x"}' > "$T/j/cadre-agents.log"
echo '{"event":"thought","agent":"still_heron"}' > "$T/j/cadre-practice.log"
run
check "healthy run is OK overall" "grep -q ' OK' $T/log/status"
check "desktop tasks OK with 1 of 13 failed" "line desktop_tasks | grep -q 'OK desktop_tasks 1 of 13'"
check "openrouter shows dollars left" "line openrouter | grep -q 'OK openrouter \\\$37.50 left'"
check "gateway data OK" "line gateway_data | grep -q 'OK gateway_data 1.0 GB'"
check "agents OK" "line agents_inference | grep -q '^.* OK agents_inference 0 inference_hold'"
check "practice OK" "line practice_thought | grep -q 'OK practice_thought 1 thoughts'"

# --- today's outages
{ for i in $(seq 1 6); do task "$i" true ""; done; for i in $(seq 7 20); do task "$i" false "timeout"; done; } > "$T/j/cadre-desktops.log"
run
check "desktops WARN on a high failure share" "line desktop_tasks | grep -q 'WARN desktop_tasks 14 of 20'"
task 21 false 'model 402: {\"error\":{\"message\":\"This request requires more credits\"}}' >> "$T/j/cadre-desktops.log"
echo '{"data":{"total_credits":20,"total_usage":17}}' > "$T/credits.json"
echo '{"request_data":{"bytes_waiting":18253611008,"refused_storage":9,"refused_node_storage":0}}' > "$T/stats.json"
for i in $(seq 1 15); do echo "{\"event\":\"inference_hold\",\"agent\":\"p$((i % 3))\",\"text\":\"paused\"}"; echo "{\"event\":\"practice_error\",\"agent\":\"p$((i % 3))\",\"message\":\"gateway 402: no prepaid balance\"}"; done > "$T/j/cadre-agents.log"
echo '{"event":"tick"}' > "$T/j/cadre-practice.log"
run
check "desktops FAIL on model 402" "line desktop_tasks | grep -q 'FAIL desktop_tasks .* 1 model 402'"
check "openrouter WARN under 5 dollars" "line openrouter | grep -q 'WARN openrouter \\\$3.00 left'"
check "gateway WARN over 16 GB and refused_storage rising" "line gateway_data | grep -q 'WARN gateway_data 17.0 GB .*refused_storage 9 (was 3)'"
check "agents WARN on repeated holds and 402s" "line agents_inference | grep -q 'WARN agents_inference 15 inference_hold + 15 402 lines .* from 3 agents'"
check "practice WARN with no thought" "line practice_thought | grep -q 'WARN practice_thought 0 thoughts'"
check "overall FAIL" "grep -q ' FAIL ' $T/log/status"
echo '{"data":{"total_credits":20,"total_usage":20}}' > "$T/credits.json"
run
check "openrouter FAIL at zero" "line openrouter | grep -q 'FAIL openrouter'"
check "refused_storage not rising since last run: no WARN from it alone" "echo '{\"request_data\":{\"bytes_waiting\":1,\"refused_storage\":9,\"refused_node_storage\":0}}' > $T/stats.json; run; line gateway_data | grep -q 'OK gateway_data'"
check "the OpenRouter key never reaches the log, status or output" "! grep -rq -- '$SECRET' $T/log $T/out $T/state"
check "a missing key is a WARN, not a crash" "echo 'OTHER=1' > $T/providers.env; run; line openrouter | grep -q 'WARN openrouter no OPENROUTER_API_KEY'"
[[ $fails == 0 ]] && echo "ALL PASS" || { echo "FAILED $fails"; cat "$T/out"; tail -20 "$T/log/health.log"; }
exit $fails
