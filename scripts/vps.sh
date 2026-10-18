#!/usr/bin/env bash
# Test VPS helper (docs/VPS.md). Box: BitLaunch "pearl-sol-test", user pearl.
#   scripts/vps.sh sync     send committed history (git bundle of the current branch) and fast-forward ~/pearl-sol there
#   scripts/vps.sh ssh      shell on the box as pearl
#   scripts/vps.sh tunnel   forward the box's ports to this machine, remote 106xx -> local 206xx (foreground; Ctrl-C ends it)
#   scripts/vps.sh demo     run scripts/demo.sh on the box with DEMO_KEEP_UP=1 (stack stays up, log ~/demo.log)
#   scripts/vps.sh down     stop the box's demo stack (scripts/demo.sh down, PIDs only)
# Env: VPS_HOST (default 64.95.13.106), VPS_KEY (default ~/.ssh/pearl_vps),
#      VPS_PORT_OFFSET (default 10000: local port = remote port + offset; 0 maps 1:1).
# Only committed files travel; uncommitted work and ignored dirs (target/, node_modules/, .demo/) never do.
# The default offset keeps the tunnel clear of a local demo stack on 10600-10669.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${VPS_HOST:-64.95.13.106}"
KEY="${VPS_KEY:-$HOME/.ssh/pearl_vps}"
SSH=(ssh -i "$KEY" -o IdentitiesOnly=yes)
TARGET="pearl@$HOST"

case "${1:-}" in
  sync)
    branch="$(git -C "$ROOT" rev-parse --abbrev-ref HEAD)"
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    git -C "$ROOT" bundle create "$tmp/pearl-sol.bundle" "$branch" 2>/dev/null
    scp -q -i "$KEY" -o IdentitiesOnly=yes "$tmp/pearl-sol.bundle" "$TARGET:pearl-sol.bundle"
    "${SSH[@]}" "$TARGET" "cd ~/pearl-sol && git checkout -q $branch && git pull -q --ff-only ~/pearl-sol.bundle $branch && git log --oneline -1"
    ;;
  ssh) shift; exec "${SSH[@]}" "$TARGET" "$@" ;;
  tunnel)
    off="${VPS_PORT_OFFSET:-10000}" fw=()
    for p in 10600 10601 10630 10631 10632 10634 10660 10661 10662 10663; do
      lp=$((p + off))
      if lsof -ti :"$lp" >/dev/null 2>&1; then echo "local port $lp is in use, refusing" >&2; exit 1; fi
      fw+=(-L "127.0.0.1:$lp:127.0.0.1:$p")
    done
    echo "tunnel to $HOST: remote 10600 10601 10630-10634 10660-10663 -> local +$off (Ctrl-C to end)"
    echo "  pearl --rpc http://127.0.0.1:$((10630 + off)) --sol-rpc http://127.0.0.1:$((10600 + off)) --pool http://127.0.0.1:$((10663 + off)) ..."
    exec "${SSH[@]}" -N "${fw[@]}" "$TARGET"
    ;;
  demo)
    # detached on the box (the kept-up stack outlives this ssh session), then follow the log to the end
    "${SSH[@]}" -n "$TARGET" 'cd ~/pearl-sol && setsid nohup bash -c "DEMO_KEEP_UP=1 bash scripts/demo.sh; echo DEMO_EXIT=\$?" > ~/demo.log 2>&1 < /dev/null & echo "demo started on the box, log ~/demo.log"'
    "${SSH[@]}" "$TARGET" 'tail -n +1 -f ~/demo.log | sed "/^DEMO_EXIT=/q"'
    ;;
  down) "${SSH[@]}" "$TARGET" 'cd ~/pearl-sol && bash scripts/demo.sh down' ;;
  *) sed -n '2,10p' "$0"; exit 1 ;;
esac
