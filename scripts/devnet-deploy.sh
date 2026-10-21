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
#   scripts/devnet-deploy.sh rpc-url [N]       copy keys/devnet/solana-rpc-url (dedicated Solana RPC) to box N (default all), then ctl restart
#   scripts/devnet-deploy.sh relay-key [N]     create the header relay's fee payer on box N (default HEADER_RELAY_BOX), print its pubkey
#   scripts/devnet-deploy.sh svc-key N NAME    create /etc/lode/keys/NAME.json on box N (e.g. gateway), print its pubkey
#   scripts/devnet-deploy.sh harden N          install the systemd sandbox drop-ins (deploy/systemd/hardening) for every cadre-* unit on box N; NO restart (each unit picks them up at its next restart: announced window only)
#   scripts/devnet-deploy.sh health            install/refresh the health check (scripts/devnet-health.sh + cadre-health.timer) on box 0; runs it once
#   scripts/devnet-deploy.sh os N              key-only sshd drop-in + unattended-upgrades (no auto reboot) on box N; reloads sshd only after a test login
#   scripts/devnet-deploy.sh site              ~/cadre-web main -> box 0 (git bundle), npm ci + build for devnet, restart cadre-web
# Only committed files travel to the boxes. The deployer key never leaves the Mac.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
ip_of() { local v="BOX${1}_IP"; echo "${!v}"; }
sshb() { local n=$1 u=$2; shift 2; ssh -i "$HOME/.ssh/lode_devnet$n" -o IdentitiesOnly=yes -o ConnectTimeout=20 "$u@$(ip_of "$n")" "$@"; }
scpb() { local n=$1; shift; scp -q -i "$HOME/.ssh/lode_devnet$n" -o IdentitiesOnly=yes "$@"; }
UNITS0="cadre-node cadre-miner cadre-attester cadre-relayer cadre-settle cadre-storefront cadre-crank cadre-explorer cadre-api cadre-web cadre-worker cadre-gateway"
UNITSN="cadre-node cadre-miner"
# the header relay runs on HEADER_RELAY_BOX (default 1): Solana devnet's public RPC allows about
# 100 calls per 10 s per IP, and box 0's services already use most of box 0's
# HEADER_RELAY_BOX may list several boxes ("1 2"): each relays its lane (height % count) with its own IP
relay_boxes() { echo "${HEADER_RELAY_BOX:-1}"; }
is_relay_box() { local b; for b in $(relay_boxes); do [[ $b == "$1" ]] && return 0; done; return 1; }
relay_lane() { local b i=0 n; n=$(echo $(relay_boxes) | wc -w | tr -d ' '); for b in $(relay_boxes); do [[ $b == "$1" ]] && { echo "$i/$n"; return; }; i=$((i + 1)); done; echo "0/1"; }
units_of() { local u; [[ $1 == 0 ]] && u="$UNITS0" || u="$UNITSN"; is_relay_box "$1" && u="$u cadre-header-relay"; echo "$u"; }

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
    scpb 0 "lode@$BOX0_IP:/opt/lode/bin/agent-node" "lode@$BOX0_IP:/opt/lode/bin/pearl-miner" "lode@$BOX0_IP:/opt/lode/bin/header-relay" "$tmp/"
    for n in 1 2; do
      [[ -n "$(ip_of $n)" ]] || continue
      # copy beside the running binaries, then rename (overwriting a running executable fails: text file busy)
      scpb "$n" "$tmp/agent-node" "lode@$(ip_of $n):/opt/lode/bin/agent-node.new"
      scpb "$n" "$tmp/pearl-miner" "lode@$(ip_of $n):/opt/lode/bin/pearl-miner.new"
      sshb "$n" lode 'cd /opt/lode/bin && chmod 755 agent-node.new pearl-miner.new && mv -f agent-node.new agent-node && mv -f pearl-miner.new pearl-miner && sha256sum agent-node pearl-miner'
      if is_relay_box "$n"; then
        scpb "$n" "$tmp/header-relay" "lode@$(ip_of $n):/opt/lode/bin/header-relay.new"
        sshb "$n" lode 'cd /opt/lode/bin && chmod 755 header-relay.new && mv -f header-relay.new header-relay && sha256sum header-relay'
      fi
    done
    ;;
  install)
    n="$2"; ip="$(ip_of "$n")"; dash="${ip//./-}"
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    sed "s/@IP@/$dash/g" "$ROOT/deploy/Caddyfile.tmpl" > "$tmp/Caddyfile"
    [[ $n == 0 ]] && sed "s/@IP@/$dash/g" "$ROOT/deploy/Caddyfile.box0.tmpl" >> "$tmp/Caddyfile"
    cp "$ROOT/deploy/genesis.json" "$tmp/genesis.json"
    cp "$ROOT/deploy/with-rpc.sh" "$tmp/with-rpc"
    python3 "$ROOT/deploy/node-config.py" "$n" > "$tmp/node.json"
    for u in $(units_of "$n"); do cp "$ROOT/deploy/systemd/$u.service" "$tmp/"; done
    printf 'MINER_THREADS=%s\nMINER_EXTRA=%s\n' "$(eval echo "\${BOX${n}_MINER_THREADS}")" "$( [[ $n == 0 ]] && echo '--deliver --attester http://127.0.0.1:10661' )" > "$tmp/miner.env"
    [[ $n == 0 ]] && printf 'TOKEN_RPC=%s\nBRIDGE_PROGRAM=%s\nSTOREFRONT_PROGRAM=%s\nRESERVE_PROGRAM=%s\nLIGHTCLIENT_PROGRAM=%s\nPUBLIC_URL=https://storefront.%s.sslip.io\nHR_PARALLEL=%s\nHR_CONFIRM_POLL_MS=%s\nHR_SEND_GAP_MS=%s\nREFUND_GRACE_MS=%s\nCADRE_API_PUBLIC_URL=https://api.%s.sslip.io\nEXPLORER_URL=https://explorer.%s.sslip.io\n' \
      "$SOLANA_RPC" "$BRIDGE_PROGRAM" "$STOREFRONT_PROGRAM" "$RESERVE_PROGRAM" "$LIGHTCLIENT_PROGRAM" "$dash" "$HR_PARALLEL" "${HR_CONFIRM_POLL_MS:-50}" "${HR_SEND_GAP_MS:-0}" "$REFUND_GRACE_MS" "$dash" "$dash" > "$tmp/services.env"
    # inference (box 0): the registered model dir (pearl model register writes its manifest.json there), worker threads = half the cores
    [[ $n == 0 ]] && printf 'CADRE_MODEL_DIR=/var/lib/lode/cadre-model\nWORKER_THREADS=%s\nEXPLORER_URL=https://explorer.%s.sslip.io\nGATEWAY_PUBLIC_API_ROOT=https://api.%s.sslip.io/v1/openai\nGATEWAY_EXTRA=%s\n' \
      "${BOX0_WORKER_THREADS:-2}" "$dash" "$dash" "${GATEWAY_EXTRA:-}" > "$tmp/inference.env"
    is_relay_box "$n" && printf 'TOKEN_RPC=%s\nHR_PARALLEL=%s\nHR_CONFIRM_POLL_MS=%s\nHR_SEND_GAP_MS=%s\nHR_LANE=%s\n' \
      "$SOLANA_RPC" "$HR_PARALLEL" "${HR_CONFIRM_POLL_MS:-50}" "${HR_SEND_GAP_MS:-0}" "$(relay_lane "$n")" > "$tmp/header-relay.env"
    scpb "$n" "$tmp"/* "root@$ip:/tmp/"
    sshb "$n" root "set -e; cd /tmp; install -o lode -g lode -m 644 genesis.json /etc/lode/; install -o lode -g lode -m 600 miner.env /etc/lode/; install -o lode -g lode -m 600 node.json /etc/lode/; [ -f services.env ] && install -o lode -g lode -m 600 services.env /etc/lode/ || true; [ -f header-relay.env ] && install -o lode -g lode -m 600 header-relay.env /etc/lode/ || true; [ -f inference.env ] && install -o lode -g lode -m 600 inference.env /etc/lode/ || true
      install -m 644 Caddyfile /etc/caddy/Caddyfile; install -m 755 with-rpc /opt/lode/bin/with-rpc; rm -f /tmp/with-rpc; install -m 644 cadre-*.service /etc/systemd/system/; install -d -o lode -g lode /var/lib/lode/results /var/lib/lode/explorer /var/lib/lode/cadre-api /var/lib/lode/gateway /var/lib/lode/models
      systemctl daemon-reload; systemctl reload caddy || systemctl restart caddy; rm -f /tmp/cadre-*.service /tmp/genesis.json /tmp/node.json /tmp/*.env /tmp/Caddyfile; echo installed on box $n"
    ;;
  rpc-url)
    # the dedicated Solana devnet RPC URL (keys/devnet/solana-rpc-url, carries a provider key): copied
    # over ssh stdin to /etc/lode/solana-rpc-url (0600, lode), read by /opt/lode/bin/with-rpc at service
    # start; never printed. Without the file the services stay on the public RPC.
    f="$ROOT/keys/devnet/solana-rpc-url"
    if [[ ! -s $f ]]; then echo "no keys/devnet/solana-rpc-url: services stay on the public devnet RPC"; exit 0; fi
    for n in ${2:-0 1 2}; do
      sshb "$n" root 'umask 077; cat > /etc/lode/solana-rpc-url.new && chown lode:lode /etc/lode/solana-rpc-url.new && chmod 600 /etc/lode/solana-rpc-url.new && mv -f /etc/lode/solana-rpc-url.new /etc/lode/solana-rpc-url' < "$f"
      echo "box $n: dedicated RPC file installed"
    done
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
  relay-key|svc-key)
    # relay-key [N]: the header relay's Solana fee payer, generated ON the relay box (never copied from the Mac);
    # svc-key N NAME: /etc/lode/keys/NAME.json on box N the same way (gateway: the inference gateway's chain key).
    # Prints only the public key.
    if [[ $1 == svc-key ]]; then n="$2"; kname="${3:?svc-key N NAME}"; [[ $kname =~ ^[a-z0-9-]+$ ]] || { echo "bad key name"; exit 1; }
    else n="${2:-$(relay_boxes | awk '{print $1}')}"; kname=relay; fi
    sshb "$n" root 'set -e; f=/etc/lode/keys/'"$kname"'.json; if [ ! -f "$f" ]; then umask 077; d=$(mktemp -d); openssl genpkey -algorithm ed25519 -out "$d/k.pem"; openssl pkey -in "$d/k.pem" -outform DER -out "$d/k.der"; openssl pkey -in "$d/k.pem" -pubout -outform DER -out "$d/p.der"; python3 -c "import sys,json; s=open(sys.argv[1],\"rb\").read()[-32:]; p=open(sys.argv[2],\"rb\").read()[-32:]; open(sys.argv[3],\"w\").write(json.dumps(list(s+p)))" "$d/k.der" "$d/p.der" "$f"; rm -rf "$d"; chown lode:lode "$f"; chmod 600 "$f"; fi
      python3 -c "import json,sys; k=bytes(json.load(open(sys.argv[1]))[32:]); A=\"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz\"; n=int.from_bytes(k,\"big\"); o=\"\"
while n: n,r=divmod(n,58); o=A[r]+o
print(\"1\"*(len(k)-len(k.lstrip(b\"\\0\")))+o)" "$f"'
    ;;
  harden)
    # systemd sandbox drop-ins (docs/DEVNET.md "Ops hardening"); verified by dry runs, see there.
    n="$2"; tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    cp "$ROOT"/deploy/systemd/hardening/*.conf "$tmp/"; scpb "$n" "$tmp"/*.conf "root@$(ip_of "$n"):/tmp/"
    sshb "$n" root 'set -e; cd /tmp
      for u in $(systemctl list-unit-files --plain --no-legend "cadre-*.service" | awk "{print \$1}" | grep -v "^cadre-health"); do
        d=/etc/systemd/system/$u.d; install -d -m 755 $d; install -m 644 common.conf $d/10-hardening.conf
        case ${u%.service} in cadre-header-relay|cadre-settle|cadre-attester|cadre-gateway) install -m 644 native.conf $d/20-native.conf;; cadre-web) install -m 644 cadre-web.conf $d/20-web.conf;; esac
        echo "$u: $(systemd-analyze security $u 2>/dev/null | tail -1 | sed "s/.*: //") (after restart)"
      done
      rm -f common.conf native.conf cadre-web.conf; systemctl daemon-reload; echo "drop-ins installed; nothing restarted"' ;;
  health)
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    cp "$ROOT/scripts/devnet-health.sh" "$ROOT/deploy/systemd/cadre-health.service" "$ROOT/deploy/systemd/cadre-health.timer" "$tmp/"
    scpb 0 "$tmp"/* "root@$BOX0_IP:/tmp/"
    sshb 0 root 'set -e; cd /tmp; install -m 755 devnet-health.sh /opt/lode/bin/cadre-health; install -m 644 cadre-health.service cadre-health.timer /etc/systemd/system/; rm -f devnet-health.sh cadre-health.service cadre-health.timer
      systemctl daemon-reload; systemctl enable --now cadre-health.timer >/dev/null 2>&1; systemctl start cadre-health.service; cat /var/log/cadre-health/status' ;;
  os)
    n="$2"; ip="$(ip_of "$n")"
    scpb "$n" "$ROOT/deploy/ssh/00-cadre-hardening.conf" "$ROOT/deploy/apt/52cadre-unattended" "root@$ip:/tmp/"
    sshb "$n" root 'set -e; install -m 644 /tmp/00-cadre-hardening.conf /etc/ssh/sshd_config.d/; install -m 644 /tmp/52cadre-unattended /etc/apt/apt.conf.d/; rm -f /tmp/00-cadre-hardening.conf /tmp/52cadre-unattended; sshd -t'
    sshb "$n" root true && sshb "$n" lode true && sshb "$n" root 'systemctl reload ssh.service; sshd -T | grep -Ei "^(passwordauthentication|permitrootlogin) "' ;;
  ctl) n="$2"; act="$3"; shift 3; u="${*:-$(units_of "$n")}"; sshb "$n" root "systemctl $act $u; systemctl --no-pager --plain list-units 'cadre-*' | grep cadre" ;;
  status)
    for n in 0 1 2; do
      ip="$(ip_of $n)"; [[ -n $ip ]] || continue
      echo "== box $n $ip"
      sshb "$n" lode "curl -s -m 3 -X POST http://127.0.0.1:10630 -H 'content-type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getChainInfo\",\"params\":{}}' | python3 -c 'import json,sys; r=json.load(sys.stdin)[\"result\"]; print(\"height\", r[\"height\"], \"final\", r[\"finalized_height\"], \"peers\", r[\"peers\"])' 2>/dev/null || echo 'node not answering'; systemctl --no-pager --plain list-units 'cadre-*' | grep cadre | awk '{print \$1, \$3, \$4}'; uptime"
    done
    ;;
  site)
    # the website (~/cadre-web, committed main only) built on box 0 against the devnet cadre-api;
    # prebuild copies the miner installer to public/ (devnet defaults): the one committed on this
    # checkout's main (sent as ~/site-installer/scripts/install-miner.sh, PEARL_SOL for the
    # prebuild), so the site does not depend on what box 0's ~/pearl-sol build tree is at
    web="${CADRE_WEB:-$HOME/cadre-web}"; dash="${BOX0_IP//./-}"
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    git -C "$web" bundle create "$tmp/web.bundle" main 2>/dev/null
    git -C "$ROOT" show main:scripts/install-miner.sh > "$tmp/install-miner.sh"
    scpb 0 "$tmp/web.bundle" "lode@$BOX0_IP:web.bundle"
    sshb 0 lode "mkdir -p ~/site-installer/scripts"
    scpb 0 "$tmp/install-miner.sh" "lode@$BOX0_IP:site-installer/scripts/install-miner.sh"
    sshb 0 lode "set -e; if [ -d ~/cadre-web/.git ]; then cd ~/cadre-web && git pull -q --ff-only ~/web.bundle main; else git clone -q -b main ~/web.bundle ~/cadre-web; fi
      cd ~/cadre-web && git log --oneline -1 && export PATH=\$HOME/.local/node/bin:\$PATH NEXT_TELEMETRY_DISABLED=1 PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
      npm ci --no-audit --no-fund >/dev/null
      PEARL_SOL=\$HOME/site-installer NEXT_PUBLIC_CADRE_API=https://api.$dash.sslip.io NEXT_PUBLIC_CADRE_NETWORK=devnet NEXT_PUBLIC_CADRE_EXPLORER=https://explorer.$dash.sslip.io npm run build 2>&1 | tail -25"
    sshb 0 root "systemctl enable --now cadre-web >/dev/null 2>&1; systemctl restart cadre-web; sleep 3; systemctl is-active cadre-web"
    ;;
  swarm)
    # swarm [keys|unit]: the house swarm on box 0 (services/swarm, cadre-swarm.service). `keys` copies
    # keys/devnet/swarm/*.json (made by `node services/swarm/src/main.ts keys keys/devnet/swarm`) over ssh
    # stdin to /etc/lode/keys/swarm/ (0700 dir, 0600 files, lode); `unit` installs and enables the unit
    # (start it with `ctl 0 start cadre-swarm`). Never prints a key.
    what="${2:-keys}"
    if [[ $what == keys ]]; then
      d="$ROOT/keys/devnet/swarm"; ls "$d"/*.json >/dev/null 2>&1 || { echo "no keys in keys/devnet/swarm"; exit 1; }
      sshb 0 root 'install -d -m 700 -o lode -g lode /etc/lode/keys/swarm'
      for f in "$d"/*.json; do
        b="$(basename "$f")"; [[ $b =~ ^[a-z0-9_-]+\.json$ ]] || { echo "bad key file name $b"; exit 1; }
        sshb 0 root "umask 077; cat > /etc/lode/keys/swarm/$b.new && chown lode:lode /etc/lode/keys/swarm/$b.new && chmod 600 /etc/lode/keys/swarm/$b.new && mv -f /etc/lode/keys/swarm/$b.new /etc/lode/keys/swarm/$b" < "$f"
      done
      echo "box 0: $(ls "$d"/*.json | wc -l | tr -d ' ') swarm key files installed"
    elif [[ $what == unit ]]; then
      sshb 0 root 'cat > /etc/systemd/system/cadre-swarm.service && systemctl daemon-reload && systemctl enable cadre-swarm' < "$ROOT/deploy/systemd/cadre-swarm.service"
      echo "box 0: cadre-swarm.service installed and enabled (not started)"
    else
      echo "usage: swarm keys|unit"; exit 1
    fi
    ;;
  ssh) n="$2"; shift 2; sshb "$n" lode "$@" ;;
  *) sed -n '2,14p' "$0"; exit 1 ;;
esac
