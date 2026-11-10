#!/usr/bin/env bash
# Cadre public devnet operations from the Mac (docs/DEVNET.md). Boxes: deploy/devnet.env.
#   scripts/devnet-deploy.sh sync              pushed main -> box 0 (~/pearl-sol, git bundle); refuses if local main != origin/main
#   scripts/devnet-deploy.sh build             on box 0: binaries + devnet .so + IDLs (deploy/build.sh)
#   scripts/devnet-deploy.sh ship              copy agent-node + pearl-miner from box 0 to boxes 1-2 (via the Mac)
#   scripts/devnet-deploy.sh install N         units, configs, Caddyfile on box N (root)
#   scripts/devnet-deploy.sh caddy N           https front only: rate limit build, Caddyfile, sslip names (root)
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
#   scripts/devnet-deploy.sh site              ~/cadre-web origin/main -> box 0 (git bundle), build in a new release dir, keep old chunks, switch ~/cadre-web-current, restart cadre-web
#   scripts/devnet-deploy.sh genesis-stamp [MS]  set deploy/genesis.json timestamp_ms (default now) for a new chain; commit it before install
#   scripts/devnet-deploy.sh venv              on box 0: the inference worker's CPU venv ~/cadre-venv (torch cpu, transformers, datasets ...)
#   scripts/devnet-deploy.sh checkpoint        on box 0: download the pinned HF checkpoint (MODEL_CHECKPOINT @ MODEL_REVISION) for tools/cadre/quantize.py
#   scripts/devnet-deploy.sh relayer-cursor    on box 0: write /var/lib/lode/relayer.json with RELAYER_DEPOSIT_CURSOR before cadre-relayer first starts (refuses if it exists)
# sync ships pushed history only (origin/main); install copies this checkout's deploy/ files. The deployer key never leaves the Mac.
# Box IPs and hostnames: deploy/devnet.env (BOXn_IP, DEVNET_DOMAIN); docs/REBUILD-2026-11.md "Hostnames".
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
ip_of() { local v="BOX${1}_IP"; echo "${!v:-}"; }
# BOXn_IP stays TBD_BOXn_IP until the box is bought: refuse with a clear message instead of an ssh error
need_box() {
  local n ip
  for n in "$@"; do
    ip="$(ip_of "$n")"
    if [[ -z $ip || $ip == TBD_* ]]; then
      echo "deploy/devnet.env: BOX${n}_IP is still a placeholder (${ip:-empty}); set it once box $n is bought (docs/REBUILD-2026-11.md Phase A)" >&2
      exit 1
    fi
  done
}
[[ -n ${DEVNET_DOMAIN:-} ]] || { echo "deploy/devnet.env: DEVNET_DOMAIN is not set" >&2; exit 1; }
# public names (docs/REBUILD-2026-11.md "Hostnames"): box 0 node rpc.<domain>, box N node rpcN.<domain>
rpc_host() { if [[ $1 == 0 ]]; then echo rpc; else echo "rpc$1"; fi; }
sshb() { local n=$1 u=$2; shift 2; need_box "$n"; ssh -i "$HOME/.ssh/lode_devnet$n" -o IdentitiesOnly=yes -o ConnectTimeout=20 "$u@$(ip_of "$n")" "$@"; }
scpb() { local n=$1; shift; need_box "$n"; scp -q -i "$HOME/.ssh/lode_devnet$n" -o IdentitiesOnly=yes "$@"; }
UNITS0="cadre-node cadre-miner cadre-attester cadre-relayer cadre-settle cadre-storefront cadre-crank cadre-explorer cadre-api cadre-web cadre-worker cadre-gateway"
UNITSN="cadre-node cadre-miner"
# the header relay runs on HEADER_RELAY_BOX (default 1): Solana devnet's public RPC allows about
# 100 calls per 10 s per IP, and box 0's services already use most of box 0's
# HEADER_RELAY_BOX may list several boxes ("1 2"): each relays its lane (height % count) with its own IP
relay_boxes() { echo "${HEADER_RELAY_BOX:-1}"; }
is_relay_box() { local b; for b in $(relay_boxes); do [[ $b == "$1" ]] && return 0; done; return 1; }
relay_lane() { local b i=0 n; n=$(echo $(relay_boxes) | wc -w | tr -d ' '); for b in $(relay_boxes); do [[ $b == "$1" ]] && { echo "$i/$n"; return; }; i=$((i + 1)); done; echo "0/1"; }
# Only PUSHED history goes to the boxes: the bundle is origin/main (after a fetch), never a local
# branch. Refuse when this checkout's HEAD is not exactly origin/main (another session's unpushed
# commit, or one of yours), so what you are looking at is what ships. Works from ~/pearl-sol on main
# and from a worktree whose branch was merged and pushed (its HEAD then equals origin/main; else
# `git checkout --detach origin/main` there). Several sessions share ~/pearl-sol; 2026-10-21 a sync
# and a site deploy almost shipped another session's unpushed commit. ALLOW_UNPUSHED_MAIN=1 skips the
# HEAD check (origin/main still ships; say why in your CLAUDE.md row).
pushed_main() {
  local repo=$1 l o
  git -C "$repo" fetch -q origin main 2>/dev/null || { echo "$repo: git fetch origin main failed" >&2; exit 1; }
  l="$(git -C "$repo" rev-parse HEAD)"; o="$(git -C "$repo" rev-parse origin/main)"
  if [[ $l != "$o" && ${ALLOW_UNPUSHED_MAIN:-0} != 1 ]]; then
    echo "$repo: HEAD ${l:0:9} is not origin/main ${o:0:9}; push or merge first (or ALLOW_UNPUSHED_MAIN=1)" >&2
    git -C "$repo" log --oneline origin/main..HEAD | head -20 | sed 's/^/  unpushed: /' >&2
    git -C "$repo" log --oneline HEAD..origin/main | head -20 | sed 's/^/  not pulled: /' >&2
    exit 1
  fi
}
# the box side of a bundle made from origin/main: clone on first use, then fast-forward
BUNDLE_REF=refs/remotes/origin/main
# box_pull DIR BUNDLE [reset]: reset = discard edits in DIR first (the site checkout, which prebuild writes into)
box_pull() { local r=""; [[ ${3:-} == reset ]] && r="git checkout -q -- . && "; echo "if [ -d $1/.git ]; then cd $1 && ${r}git pull -q --ff-only $2 $BUNDLE_REF; else git init -q -b main $1 && cd $1 && git pull -q --ff-only $2 $BUNDLE_REF; fi"; }
units_of() { local u; [[ $1 == 0 ]] && u="$UNITS0" || u="$UNITSN"; is_relay_box "$1" && u="$u cadre-header-relay"; echo "$u"; }

case "${1:-}" in
  sync)
    pushed_main "$ROOT"
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    git -C "$ROOT" bundle create "$tmp/repo.bundle" origin/main 2>/dev/null
    scpb 0 "$tmp/repo.bundle" "lode@$BOX0_IP:repo.bundle"
    sshb 0 lode "$(box_pull '~/pearl-sol' '~/repo.bundle'); cd ~/pearl-sol && git log --oneline -1"
    ;;
  build) sshb 0 lode 'bash -lc "cd ~/pearl-sol && bash deploy/build.sh"' ;;
  ship)
    need_box 0 1 2
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
    n="$2"; need_box 0 1 2; ip="$(ip_of "$n")"; d="$DEVNET_DOMAIN"
    # a new chain's genesis needs its launch time (genesis-stamp); 0 = not stamped yet
    python3 -c 'import json,sys; t=json.load(open(sys.argv[1]))["timestamp_ms"]; sys.exit(0 if int(t) > 0 else 1)' "$ROOT/deploy/genesis.json" \
      || { echo "deploy/genesis.json timestamp_ms is 0: run scripts/devnet-deploy.sh genesis-stamp and commit it first" >&2; exit 1; }
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    sed -e "s/@RPC_HOST@/$(rpc_host "$n")/g" -e "s/@DOMAIN@/$d/g" "$ROOT/deploy/Caddyfile.tmpl" > "$tmp/Caddyfile"
    [[ $n == 0 ]] && sed -e "s/@DOMAIN@/$d/g" "$ROOT/deploy/Caddyfile.box0.tmpl" >> "$tmp/Caddyfile"
    if grep -q '@[A-Z_]*@' "$tmp/Caddyfile"; then echo "Caddyfile: unfilled placeholder" >&2; grep -n '@[A-Z_]*@' "$tmp/Caddyfile" >&2; exit 1; fi
    cp "$ROOT/deploy/genesis.json" "$tmp/genesis.json"
    cp "$ROOT/deploy/with-rpc.sh" "$tmp/with-rpc"
    python3 "$ROOT/deploy/node-config.py" "$n" > "$tmp/node.json"
    for u in $(units_of "$n"); do cp "$ROOT/deploy/systemd/$u.service" "$tmp/"; done
    printf 'MINER_THREADS=%s\nMINER_EXTRA=%s\n' "$(eval echo "\${BOX${n}_MINER_THREADS}")" "$( [[ $n == 0 ]] && echo '--deliver --attester http://127.0.0.1:10661' )" > "$tmp/miner.env"
    # TOWN_PROGRAM: every town or DM writer on box 0 reads it from services.env (it was a hand edit before 2026-11)
    [[ $n == 0 ]] && printf 'TOKEN_RPC=%s\nBRIDGE_PROGRAM=%s\nSTOREFRONT_PROGRAM=%s\nRESERVE_PROGRAM=%s\nLIGHTCLIENT_PROGRAM=%s\nTOWN_PROGRAM=%s\nPUBLIC_URL=https://storefront.%s\nHR_PARALLEL=%s\nHR_CONFIRM_POLL_MS=%s\nHR_SEND_GAP_MS=%s\nREFUND_GRACE_MS=%s\nCADRE_API_PUBLIC_URL=https://api.%s\nEXPLORER_URL=https://explorer.%s\n' \
      "$SOLANA_RPC" "$BRIDGE_PROGRAM" "$STOREFRONT_PROGRAM" "$RESERVE_PROGRAM" "$LIGHTCLIENT_PROGRAM" "$TOWN_PROGRAM" "$d" "$HR_PARALLEL" "${HR_CONFIRM_POLL_MS:-50}" "${HR_SEND_GAP_MS:-0}" "$REFUND_GRACE_MS" "$d" "$d" > "$tmp/services.env"
    # inference (box 0): the registered model dir (pearl model register writes its manifest.json there), worker threads = half the cores
    [[ $n == 0 ]] && printf 'CADRE_MODEL_DIR=/var/lib/lode/cadre-model\nWORKER_THREADS=%s\nEXPLORER_URL=https://explorer.%s\nGATEWAY_PUBLIC_API_ROOT=https://api.%s/v1/openai\nGATEWAY_EXTRA=%s\n' \
      "${BOX0_WORKER_THREADS:-2}" "$d" "$d" "${GATEWAY_EXTRA:-}" > "$tmp/inference.env"
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
      for u in $(systemctl list-unit-files --plain --no-legend "cadre-*.service" | awk "{print \$1}" | grep -v "^cadre-health\|^cadre-backup"); do
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
      ip="$(ip_of $n)"; if [[ -z $ip || $ip == TBD_* ]]; then echo "== box $n: no IP yet (${ip:-empty})"; continue; fi
      echo "== box $n $ip"
      sshb "$n" lode "curl -s -m 3 -X POST http://127.0.0.1:10630 -H 'content-type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getChainInfo\",\"params\":{}}' | python3 -c 'import json,sys; r=json.load(sys.stdin)[\"result\"]; print(\"height\", r[\"height\"], \"final\", r[\"finalized_height\"], \"peers\", r[\"peers\"])' 2>/dev/null || echo 'node not answering'; systemctl --no-pager --plain list-units 'cadre-*' | grep cadre | awk '{print \$1, \$3, \$4}'; uptime"
    done
    ;;
  site)
    # the website (~/cadre-web, committed main only) built on box 0 against the devnet cadre-api;
    # prebuild copies the miner installer to public/ (devnet defaults): the one committed on this
    # checkout's main (sent as ~/site-installer/scripts/install-miner.sh, CADRE_SOL for the
    # prebuild), so the site does not depend on what box 0's ~/pearl-sol build tree is at
    web="${CADRE_WEB:-$HOME/cadre-web}"; d="$DEVNET_DOMAIN"; need_box 0
    pushed_main "$web"; pushed_main "$ROOT"
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    git -C "$web" bundle create "$tmp/web.bundle" origin/main 2>/dev/null
    git -C "$ROOT" show origin/main:scripts/install-miner.sh > "$tmp/install-miner.sh"
    scpb 0 "$tmp/web.bundle" "lode@$BOX0_IP:web.bundle"
    sshb 0 lode "mkdir -p ~/site-installer/scripts"
    scpb 0 "$tmp/install-miner.sh" "lode@$BOX0_IP:site-installer/scripts/install-miner.sh"
    git -C "$ROOT" show origin/main:deploy/site-release.sh > "$tmp/site-release.sh"
    scpb 0 "$tmp/site-release.sh" "lode@$BOX0_IP:site-installer/site-release.sh"
    # build a new release next to the live one, keep the old chunks, switch ~/cadre-web-current
    # (deploy/site-release.sh); the source checkout ~/cadre-web is only pulled, never built in
    sshb 0 lode "set -e; $(box_pull '~/cadre-web' '~/web.bundle' reset)
      export PATH=\$HOME/.local/node/bin:\$PATH; bash ~/site-installer/site-release.sh https://api.$d https://explorer.$d"
    # the unit runs from ~/cadre-web-current (deploy/systemd/cadre-web.service); refreshed on every deploy
    sshb 0 root 'cat > /etc/systemd/system/cadre-web.service.new && mv /etc/systemd/system/cadre-web.service.new /etc/systemd/system/cadre-web.service && systemctl daemon-reload' < "$ROOT/deploy/systemd/cadre-web.service"
    sshb 0 root "systemctl enable cadre-web >/dev/null 2>&1; systemctl restart cadre-web; sleep 3; systemctl is-active cadre-web"
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
  caddy)
    # caddy N: the https front of box N alone: the caddy build with the rate limit module
    # (deploy/caddy-custom.sh), the Caddyfile from the templates, the sslip aliases again, then a
    # restart (not a reload: the admin endpoint moves to a unix socket). Validates before it swaps;
    # the old file stays at /etc/caddy/Caddyfile.prev.
    n="$2"; d="$DEVNET_DOMAIN"
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    sed -e "s/@RPC_HOST@/$(rpc_host "$n")/g" -e "s/@DOMAIN@/$d/g" "$ROOT/deploy/Caddyfile.tmpl" > "$tmp/Caddyfile"
    [[ $n == 0 ]] && sed -e "s/@DOMAIN@/$d/g" "$ROOT/deploy/Caddyfile.box0.tmpl" >> "$tmp/Caddyfile"
    if grep -q '@[A-Z_]*@' "$tmp/Caddyfile"; then echo "Caddyfile: unfilled placeholder" >&2; exit 1; fi
    scpb "$n" "$tmp/Caddyfile" "root@$(ip_of "$n"):/tmp/Caddyfile.new"
    scpb "$n" "$ROOT/deploy/caddy-custom.sh" "root@$(ip_of "$n"):/tmp/caddy-custom.sh"
    sshb "$n" root "set -e; bash /tmp/caddy-custom.sh; rm -f /tmp/caddy-custom.sh
      caddy validate --config /tmp/Caddyfile.new --adapter caddyfile >/dev/null 2>&1 || { caddy validate --config /tmp/Caddyfile.new --adapter caddyfile 2>&1 | tail -5 >&2; exit 1; }
      cp /etc/caddy/Caddyfile /etc/caddy/Caddyfile.prev; install -m 644 /tmp/Caddyfile.new /etc/caddy/Caddyfile; rm -f /tmp/Caddyfile.new"
    "$0" sslip "$n" >/dev/null 2>&1 || true   # its reload fails across the admin move; the restart below applies it
    sshb "$n" root "caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1 || { cp /etc/caddy/Caddyfile.prev /etc/caddy/Caddyfile; echo 'validate failed after sslip; restored' >&2; exit 1; }
      systemctl restart caddy; systemctl is-active caddy; grep -c 'import rl' /etc/caddy/Caddyfile" ;;
  sslip)
    # sslip N: give every https site on box N a second name under <ip>.sslip.io (rpc.devnet.cadre.ag also
    # answers as rpc.64-95-12-36.sslip.io), so the devnet is reachable before or without the DEVNET_DOMAIN
    # DNS records (docs/REBUILD-2026-11.md D5 fallback). The website (the bare name and app) gets no alias: it
    # stays off the public internet until the owner confirms which site it is. Edits the live Caddyfile, validates, reloads; idempotent.
    n="$2"; ip="$(ip_of "$n")"; d="$DEVNET_DOMAIN"
    sshb "$n" root "set -e; cp /etc/caddy/Caddyfile /etc/caddy/Caddyfile.pre-sslip
      python3 - '$d' '${ip//./-}.sslip.io' <<'PY'
import re, sys
d, alias = sys.argv[1], sys.argv[2]
p = '/etc/caddy/Caddyfile'; out = []
for line in open(p).read().split('\\n'):
    m = re.fullmatch(r'((?:([a-z0-9]+)\\.)?' + re.escape(d) + r') \\{', line)
    if m and m.group(2) and m.group(2) != 'app':  # the site (bare name) and app stay private until the owner picks the site
        line = f'{m.group(1)}, {m.group(2) + \".\" if m.group(2) else \"\"}{alias} {{'
    out.append(line)
open(p, 'w').write('\\n'.join(out))
PY
      caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null 2>&1 || { cp /etc/caddy/Caddyfile.pre-sslip /etc/caddy/Caddyfile; echo 'caddy validate failed; restored' >&2; exit 1; }
      systemctl reload caddy; grep -n 'sslip.io {' /etc/caddy/Caddyfile" ;;
  genesis-stamp)
    # A new chain's launch time (docs/REBUILD-2026-11.md D2/D3): set it right before `install 0/1/2`,
    # commit deploy/genesis.json, then install every box from that one file (all nodes must match).
    ms="${2:-$(python3 -c 'import time; print(int(time.time() * 1000))')}"
    [[ $ms =~ ^[0-9]{13}$ ]] || { echo "genesis-stamp: timestamp_ms must be 13 digits (unix ms), got $ms" >&2; exit 1; }
    python3 - "$ROOT/deploy/genesis.json" "$ms" <<'PY'
import json, sys
p, ms = sys.argv[1], int(sys.argv[2])
g = json.load(open(p))
g["timestamp_ms"] = ms
open(p, "w").write(json.dumps(g, indent=2) + "\n")
print(f"deploy/genesis.json chain_id {g['chain_id']} timestamp_ms {ms}: commit it, then install 0, 1 and 2")
PY
    ;;
  venv)
    # the inference worker's CPU venv on box 0 (cadre-worker.service runs /home/lode/cadre-venv/bin/python;
    # tools/cadre/quantize.py uses it too). Pins from docs/REBUILD-2026-11.md Phase E1 (the runpod GPU
    # bootstrap uses the CUDA wheels instead). Idempotent.
    sshb 0 lode 'set -e; test -x ~/cadre-venv/bin/python || python3 -m venv ~/cadre-venv
      ~/cadre-venv/bin/pip install -q --upgrade pip
      ~/cadre-venv/bin/pip install -q torch==2.5.1 --index-url https://download.pytorch.org/whl/cpu
      ~/cadre-venv/bin/pip install -q transformers==4.46.3 datasets==3.1.0 "numpy>=2" safetensors huggingface_hub
      ~/cadre-venv/bin/python -c "import torch, transformers, datasets, numpy; print(\"venv ok: torch\", torch.__version__, \"transformers\", transformers.__version__, \"numpy\", numpy.__version__)"' ;;
  checkpoint)
    # the base checkpoint for tools/cadre/quantize.py, pinned (D6): public, no HF token needed
    sshb 0 lode "set -e; ~/cadre-venv/bin/python -c 'import sys; from huggingface_hub import snapshot_download; print(snapshot_download(sys.argv[1], revision=sys.argv[2], local_dir=sys.argv[3]))' '$MODEL_CHECKPOINT' '$MODEL_REVISION' /home/lode/hf/llama-3.2-1b-instruct
      cd /home/lode/hf/llama-3.2-1b-instruct && printf '%s@%s\n' '$MODEL_CHECKPOINT' '$MODEL_REVISION' > REVISION && du -sh ." ;;
  relayer-cursor)
    # D7: the old bridge deposits (nonces below RELAYER_DEPOSIT_CURSOR) are not replayed onto the new
    # chain. Written once, before cadre-relayer first starts; never over an existing state file.
    [[ ${RELAYER_DEPOSIT_CURSOR:-} =~ ^[0-9]+$ ]] || { echo "deploy/devnet.env: RELAYER_DEPOSIT_CURSOR is not set" >&2; exit 1; }
    sshb 0 root "CURSOR=$RELAYER_DEPOSIT_CURSOR bash -s" <<'SH'
set -e
f=/var/lib/lode/relayer.json
if systemctl is-active -q cadre-relayer; then echo "cadre-relayer is running: the cursor must be written before its first start" >&2; exit 1; fi
if [ -e "$f" ]; then
  echo "$f exists, left as is:"; python3 -c 'import json,sys; j=json.load(open(sys.argv[1])); print("deposit", j.get("deposit"), "withdrawal", j.get("withdrawal"))' "$f"; exit 0
fi
install -d -o lode -g lode /var/lib/lode
printf '{"deposit": "%s", "withdrawal": "0"}\n' "$CURSOR" > "$f.new"
chown lode:lode "$f.new"; chmod 600 "$f.new"; mv "$f.new" "$f"
echo "wrote $f: deposit cursor $CURSOR"
SH
    ;;
  ssh) n="$2"; shift 2; sshb "$n" lode "$@" ;;
  *) sed -n '2,22p' "$0"; exit 1 ;;
esac
