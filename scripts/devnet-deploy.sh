#!/usr/bin/env bash
# Cadre public devnet operations from the Mac (docs/DEVNET.md). Boxes: deploy/devnet.env.
#   scripts/devnet-deploy.sh sync              committed history -> box 0 (~/pearl-sol, git bundle)
#   scripts/devnet-deploy.sh build             on box 0: binaries + devnet .so + IDLs (deploy/build.sh)
#   scripts/devnet-deploy.sh ship              copy agent-node + pearl-miner from box 0 to boxes 1-2 (via the Mac)
#   scripts/devnet-deploy.sh install N         units, configs, Caddyfile on box N (root)
#   scripts/devnet-deploy.sh keys N            the service keys box N needs (keys/devnet -> /etc/lode/keys, 600)
#   scripts/devnet-deploy.sh ctl N ACTION [UNITS]  systemctl ACTION on box N (default: every cadre-* unit there)
#   scripts/devnet-deploy.sh status            heights, peers, units on every box
#   scripts/devnet-deploy.sh ssh N [CMD]       shell as lode on box N
# Only committed files travel to the boxes. The deployer key never leaves the Mac.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
ip_of() { local v="BOX${1}_IP"; echo "${!v}"; }
sshb() { local n=$1 u=$2; shift 2; ssh -i "$HOME/.ssh/lode_devnet$n" -o IdentitiesOnly=yes -o ConnectTimeout=20 "$u@$(ip_of "$n")" "$@"; }
scpb() { local n=$1; shift; scp -q -i "$HOME/.ssh/lode_devnet$n" -o IdentitiesOnly=yes "$@"; }
UNITS0="cadre-node cadre-miner cadre-attester cadre-relayer cadre-header-relay cadre-storefront cadre-crank cadre-explorer cadre-api"
UNITSN="cadre-node cadre-miner"
units_of() { [[ $1 == 0 ]] && echo "$UNITS0" || echo "$UNITSN"; }

case "${1:-}" in
  sync)
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    git -C "$ROOT" bundle create "$tmp/repo.bundle" main 2>/dev/null
    scpb 0 "$tmp/repo.bundle" "lode@$BOX0_IP:repo.bundle"
    sshb 0 lode 'if [ -d ~/pearl-sol/.git ]; then cd ~/pearl-sol && git pull -q --ff-only ~/repo.bundle main; else git clone -q -b main ~/repo.bundle ~/pearl-sol; fi; cd ~/pearl-sol && git log --oneline -1'
    ;;
  build) sshb 0 lode 'bash -lc "cd ~/pearl-sol && bash deploy/build.sh"' ;;
  ship)
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    scpb 0 "lode@$BOX0_IP:/opt/lode/bin/agent-node" "lode@$BOX0_IP:/opt/lode/bin/pearl-miner" "$tmp/"
    for n in 1 2; do
      [[ -n "$(ip_of $n)" ]] || continue
      # copy beside the running binaries, then rename (overwriting a running executable fails: text file busy)
      scpb "$n" "$tmp/agent-node" "lode@$(ip_of $n):/opt/lode/bin/agent-node.new"
      scpb "$n" "$tmp/pearl-miner" "lode@$(ip_of $n):/opt/lode/bin/pearl-miner.new"
      sshb "$n" lode 'cd /opt/lode/bin && chmod 755 agent-node.new pearl-miner.new && mv -f agent-node.new agent-node && mv -f pearl-miner.new pearl-miner && sha256sum agent-node pearl-miner'
    done
    ;;
  install)
    n="$2"; ip="$(ip_of "$n")"; dash="${ip//./-}"
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    sed "s/@IP@/$dash/g" "$ROOT/deploy/Caddyfile.tmpl" > "$tmp/Caddyfile"
    [[ $n == 0 ]] && sed "s/@IP@/$dash/g" "$ROOT/deploy/Caddyfile.box0.tmpl" >> "$tmp/Caddyfile"
    cp "$ROOT/deploy/genesis.json" "$tmp/genesis.json"
    python3 "$ROOT/deploy/node-config.py" "$n" > "$tmp/node.json"
    for u in $(units_of "$n"); do cp "$ROOT/deploy/systemd/$u.service" "$tmp/"; done
    printf 'MINER_THREADS=%s\nMINER_EXTRA=%s\n' "$(eval echo "\${BOX${n}_MINER_THREADS}")" "$( [[ $n == 0 ]] && echo '--deliver --attester http://127.0.0.1:10661' )" > "$tmp/miner.env"
    [[ $n == 0 ]] && printf 'TOKEN_RPC=%s\nBRIDGE_PROGRAM=%s\nSTOREFRONT_PROGRAM=%s\nRESERVE_PROGRAM=%s\nLIGHTCLIENT_PROGRAM=%s\nPUBLIC_URL=https://storefront.%s.sslip.io\nHR_PARALLEL=%s\nHR_CONFIRM_POLL_MS=%s\nHR_SEND_GAP_MS=%s\nREFUND_GRACE_MS=%s\nCADRE_API_PUBLIC_URL=https://api.%s.sslip.io\nEXPLORER_URL=https://explorer.%s.sslip.io\n' \
      "$SOLANA_RPC" "$BRIDGE_PROGRAM" "$STOREFRONT_PROGRAM" "$RESERVE_PROGRAM" "$LIGHTCLIENT_PROGRAM" "$dash" "$HR_PARALLEL" "${HR_CONFIRM_POLL_MS:-50}" "${HR_SEND_GAP_MS:-0}" "$REFUND_GRACE_MS" "$dash" "$dash" > "$tmp/services.env"
    scpb "$n" "$tmp"/* "root@$ip:/tmp/"
    sshb "$n" root "set -e; cd /tmp; install -o lode -g lode -m 644 genesis.json miner.env /etc/lode/; install -o lode -g lode -m 600 node.json /etc/lode/; [ -f services.env ] && install -o lode -g lode -m 644 services.env /etc/lode/ || true
      install -m 644 Caddyfile /etc/caddy/Caddyfile; install -m 644 cadre-*.service /etc/systemd/system/; install -d -o lode -g lode /var/lib/lode/results /var/lib/lode/explorer /var/lib/lode/cadre-api
      systemctl daemon-reload; systemctl reload caddy || systemctl restart caddy; rm -f /tmp/cadre-*.service /tmp/genesis.json /tmp/node.json /tmp/*.env /tmp/Caddyfile; echo installed on box $n"
    ;;
  keys)
    n="$2"; ip="$(ip_of "$n")"
    files=("miner$n:miner")
    [[ $n == 0 ]] && files+=("attester:attester" "ops:ops" "storefront-wallet:storefront-wallet")
    for f in "${files[@]}"; do
      src="$ROOT/keys/devnet/${f%%:*}-keypair.json"
      sshb "$n" root "umask 077; cat > /etc/lode/keys/${f##*:}.json && chown lode:lode /etc/lode/keys/${f##*:}.json && chmod 600 /etc/lode/keys/${f##*:}.json" < "$src"
    done
    echo "box $n keys: ${files[*]}"
    ;;
  ctl) n="$2"; act="$3"; shift 3; u="${*:-$(units_of "$n")}"; sshb "$n" root "systemctl $act $u; systemctl --no-pager --plain list-units 'cadre-*' | grep cadre" ;;
  status)
    for n in 0 1 2; do
      ip="$(ip_of $n)"; [[ -n $ip ]] || continue
      echo "== box $n $ip"
      sshb "$n" lode "curl -s -m 3 -X POST http://127.0.0.1:10630 -H 'content-type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getChainInfo\",\"params\":{}}' | python3 -c 'import json,sys; r=json.load(sys.stdin)[\"result\"]; print(\"height\", r[\"height\"], \"final\", r[\"finalized_height\"], \"peers\", r[\"peers\"])' 2>/dev/null || echo 'node not answering'; systemctl --no-pager --plain list-units 'cadre-*' | grep cadre | awk '{print \$1, \$3, \$4}'; uptime"
    done
    ;;
  ssh) n="$2"; shift 2; sshb "$n" lode "$@" ;;
  *) sed -n '2,12p' "$0"; exit 1 ;;
esac
