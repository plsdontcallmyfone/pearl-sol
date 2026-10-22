#!/usr/bin/env bash
# Render the dashboard from the live stack into screenshots/cli/live-<tab>.{html,txt,png}.
# Headless Chromium (Playwright's headless_shell) only; never a desktop browser.
# Usage: crates/cadre-cli/screenshot.sh KEY_FILE   (from the repo root, stack up)
set -euo pipefail
KEY=${1:?key file}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${CADRE_BIN:-$ROOT/crates/cadre-cli/target/debug/cadre}
SHELL_BIN=${HEADLESS_SHELL:-$(ls -d "$HOME"/Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-mac/headless_shell 2>/dev/null | tail -1)}
OUT=$ROOT/screenshots/cli
PROFILE=$(mktemp -d)
mkdir -p "$OUT"
for t in jobs mining activity bridge agent; do
  "$BIN" --key "$KEY" snapshot "$OUT/live-$t.html" --tab "$t" >/dev/null
  "$SHELL_BIN" --no-sandbox --disable-gpu --hide-scrollbars --user-data-dir="$PROFILE" \
    --window-size=1210,808 --screenshot="$OUT/live-$t.png" "file://$OUT/live-$t.html" 2>/dev/null | grep -o "written.*" || true
done
rm -rf "$PROFILE"
