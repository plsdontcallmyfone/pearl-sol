#!/usr/bin/env bash
# Run the text-mode commands against the live stack in a pty and render each one's REAL
# ANSI output (black background) to screenshots/cli/inline-<name>.png with Playwright's
# headless_shell (never a desktop browser). Usage, from the repo root, stack up:
#   crates/pearl-cli/tools/inline-screens.sh KEY_FILE
set -euo pipefail
KEY=${1:?key file}
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
BIN=${PEARL_BIN:-$ROOT/crates/pearl-cli/target/release/pearl}
SHELL_BIN=${HEADLESS_SHELL:-$(ls -d "$HOME"/Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-mac/headless_shell 2>/dev/null | tail -1)}
T=$ROOT/crates/pearl-cli/tools
OUT=$ROOT/screenshots/cli
RAW=$(mktemp -d)
PROFILE=$(mktemp -d)
COLS=${COLS:-104}
mkdir -p "$OUT"
cd "$ROOT"
# name | args (after --key)
run() {
  local name=$1; shift
  python3 "$T/capture.py" "$RAW/$name.raw" "$COLS" -- "$BIN" --key "$KEY" "$@"
  python3 "$T/ansi2html.py" "$OUT/inline-$name.html" "$COLS" "pearl $*" "$RAW/$name.raw" >/dev/null
  local rows; rows=$(python3 -c "import sys,re; print(open(sys.argv[1]).read().count(chr(10)))" "$OUT/inline-$name.html")
  local h=$(( rows * 18 + 30 )); local w=$(( COLS * 85 / 10 + 56 ))
  "$SHELL_BIN" --no-sandbox --disable-gpu --hide-scrollbars --user-data-dir="$PROFILE" \
    --window-size=$w,$h --screenshot="$OUT/inline-$name.png" "file://$OUT/inline-$name.html" 2>/dev/null | grep -o "written.*" || true
}
STEPS=${STEPS:-overview balance send jobpost jobshow bridgeout bridgein bridgestatus lienshow status jobs}
for s in $STEPS; do
  case $s in
    overview) run overview ;;
    balance) run balance balance ;;
    send) run send send 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU 1.5 ;;
    jobpost) run job-post job post --pay 5 --fee 1 ;;
    jobshow)
      ID=$("$BIN" --key "$KEY" jobs --limit 1 --json | python3 -c "import json,sys; print(json.load(sys.stdin)['jobs'][0]['job_id'][:12])")
      run job-show job show "$ID" ;;
    bridgeout) run bridge-out bridge out 2 ;;
    bridgein) run bridge-in bridge in 1 ;;
    bridgestatus) run bridge-status bridge status ;;
    lienshow) run lien-show lien show ;;
    status) run status status ;;
    jobs) run jobs jobs --limit 8 ;;
  esac
done
rm -rf "$RAW" "$PROFILE"
