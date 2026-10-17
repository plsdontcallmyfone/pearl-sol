#!/usr/bin/env bash
# Local agent-chain devnet (chain/bin/agent-node).
#
#   scripts/devnet.sh init      write .devnet/genesis.json and node configs (once)
#   scripts/devnet.sh start     build (release) if needed and start 3 nodes
#   scripts/devnet.sh stop      stop the nodes by recorded PID
#   scripts/devnet.sh status    PIDs and chain heads
#   scripts/devnet.sh mine [N]  mine N fallback blocks (0 = forever) on node 0
#   scripts/devnet.sh reset     stop and delete all devnet data (keeps nothing)
#
#   scripts/devnet.sh open CMD  the same commands for the OPEN-membership devnet:
#                               5 nodes in .devnet-open, node 0 is the seed and nodes
#                               1-4 know only the seed address (127.0.0.1:10640); they
#                               find each other by address gossip (net = "open").
#
# Static mode (default): 3 nodes, Commonware discovery over a fixed peer list.
# Ports: node i RPC 10630+2i, WebSocket 10631+2i, p2p 10640+i (open: i = 0..4, so
# RPC 10630-10638, WS 10631-10639, p2p 10640-10644).
# Admin = keys/localnet/payer, attester = keys/localnet/attester (1 of 1),
# miner payout = keys/localnet/miner. Never pattern-kills: PIDs only, checked.
#
# Env overrides (read by init; the demo uses them, scripts/demo.sh):
#   DEVNET_DIR      data dir (default .devnet)
#   DEVNET_PARAMS   JSON object merged over the genesis params below, e.g.
#                   '{"finality_k": 6, "target_block_time_ms": 1500}'
#   DEVNET_TARGET   genesis target_bits (default 0x207fffff)
#   DEVNET_NET      static (default) or open; `open` as the first argument sets it
#   DEVNET_NODES    node count (default 3 static, 5 open; open allows at most 5)
#   DEVNET_MINE_NODE  node index `mine` talks to (default 0)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ "${1:-}" == "open" ]]; then
  DEVNET_NET=open
  shift
fi
NET="${DEVNET_NET:-static}"
if [[ "$NET" == "open" ]]; then
  DEV="${DEVNET_DIR:-$ROOT/.devnet-open}"
  N="${DEVNET_NODES:-5}"
else
  DEV="${DEVNET_DIR:-$ROOT/.devnet}"
  N="${DEVNET_NODES:-3}"
fi
# an initialised devnet remembers its mode and size
if [[ -f "$DEV/devnet.env" ]]; then
  # shellcheck disable=SC1091
  source "$DEV/devnet.env"
fi
BIN="$ROOT/chain/target/release/agent-node"
RPC_BASE=10630
P2P_BASE=10640

pubkey_of_keyfile() { # Solana JSON keypair -> base58 pubkey
  python3 - "$1" <<'EOF'
import json, sys
b = bytes(json.load(open(sys.argv[1]))[32:64])
A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n = int.from_bytes(b, "big"); s = ""
while n: n, r = divmod(n, 58); s = A[r] + s
print("1" * (len(b) - len(b.lstrip(b"\0"))) + s)
EOF
}

seed_of_keyfile() { # first 32 bytes, hex
  python3 -c 'import json,sys; print(bytes(json.load(open(sys.argv[1]))[:32]).hex())' "$1"
}

node_seed() { printf 'pearl-sol devnet node %s' "$1" | shasum -a 256 | cut -d' ' -f1; }

build() {
  if [[ ! -x "$BIN" || -n "$(find "$ROOT/chain" -name '*.rs' -newer "$BIN" -not -path '*/target/*' | head -1)" ]]; then
    echo "building agent-node (release)..."
    (cd "$ROOT/chain" && cargo build --release -p agent-node)
  fi
}

init() {
  mkdir -p "$DEV"
  if [[ -f "$DEV/genesis.json" ]]; then echo "already initialised ($DEV)"; return; fi
  build
  local admin attester ts
  admin="$(pubkey_of_keyfile "$ROOT/keys/localnet/payer-keypair.json")"
  attester="$(pubkey_of_keyfile "$ROOT/keys/localnet/attester-keypair.json")"
  ts=$(( $(date +%s) * 1000 ))
  local params='{"target_block_time_ms": 10000, "wtema_half_life_ms": 600000, "finality_k": 30, "min_fee": 1000}'
  params="$(python3 -c 'import json,sys; p=json.loads(sys.argv[1]); p.update(json.loads(sys.argv[2] or "{}")); print(json.dumps(p))' "$params" "${DEVNET_PARAMS:-}")"
  cat > "$DEV/genesis.json" <<EOF
{
  "chain_id": 1347616769,
  "timestamp_ms": $ts,
  "target_bits": "${DEVNET_TARGET:-0x207fffff}",
  "admin": "$admin",
  "attesters": ["$attester"],
  "attester_threshold": 1,
  "params": $params
}
EOF
  if [[ "$NET" == "open" && "$N" -gt 5 ]]; then echo "open devnet: at most 5 nodes (p2p 10640-10644)" >&2; exit 2; fi
  printf 'NET=%s\nN=%s\n' "$NET" "$N" > "$DEV/devnet.env"
  if [[ "$NET" == "open" ]]; then init_open_nodes; else init_static_nodes; fi
  echo "initialised $DEV ($NET, $N nodes; admin $admin, attester $attester)"
}

init_open_nodes() {
  local i seeds
  for ((i = 0; i < N; i++)); do
    mkdir -p "$DEV/node$i"
    seeds='[]'
    [[ $i -gt 0 ]] && seeds="[\"127.0.0.1:$P2P_BASE\"]"
    cat > "$DEV/node$i.json" <<EOF
{
  "data_dir": "$DEV/node$i/data",
  "genesis": "$DEV/genesis.json",
  "p2p_key": "$(node_seed $i)",
  "p2p_listen": "127.0.0.1:$((P2P_BASE + i))",
  "rpc_listen": "127.0.0.1:$((RPC_BASE + 2 * i))",
  "ws_listen": "127.0.0.1:$((RPC_BASE + 2 * i + 1))",
  "net": "open",
  "seeds": $seeds,
  "open": {"max_outbound": 8, "max_inbound": 32},
  "verifier": "pearl",
  "tip_interval_ms": 2000
}
EOF
  done
}

init_static_nodes() {
  local peers="" i seed key
  for ((i = 0; i < N; i++)); do
    seed="$(node_seed $i)"
    key="$("$BIN" pubkey "$seed")"
    peers+="{\"key\": \"$key\", \"addr\": \"127.0.0.1:$((P2P_BASE + i))\"}"
    [[ $i -lt $((N - 1)) ]] && peers+=", "
  done
  for ((i = 0; i < N; i++)); do
    mkdir -p "$DEV/node$i"
    cat > "$DEV/node$i.json" <<EOF
{
  "data_dir": "$DEV/node$i/data",
  "genesis": "$DEV/genesis.json",
  "p2p_key": "$(node_seed $i)",
  "p2p_listen": "127.0.0.1:$((P2P_BASE + i))",
  "rpc_listen": "127.0.0.1:$((RPC_BASE + 2 * i))",
  "ws_listen": "127.0.0.1:$((RPC_BASE + 2 * i + 1))",
  "peers": [$peers],
  "verifier": "pearl",
  "tip_interval_ms": 2000
}
EOF
  done
}

pid_alive() { # pid is ours and alive
  local pid="$1"
  [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null && ps -p "$pid" -o command= | grep -q "agent-node run"
}

start() {
  [[ -f "$DEV/genesis.json" ]] || init
  build
  local i
  for ((i = 0; i < N; i++)); do
    if pid_alive "$(cat "$DEV/node$i.pid" 2>/dev/null || true)"; then echo "node$i already running"; continue; fi
    for p in $((RPC_BASE + 2 * i)) $((RPC_BASE + 2 * i + 1)) $((P2P_BASE + i)); do
      if [[ -n "$(lsof -ti :$p 2>/dev/null)" ]]; then
        echo "port $p is in use (pid $(lsof -ti :$p | tr '\n' ' ')); not starting node$i" >&2
        exit 1
      fi
    done
    RUST_LOG="${RUST_LOG:-info}" nohup "$BIN" run --config "$DEV/node$i.json" > "$DEV/node$i.log" 2>&1 &
    echo $! > "$DEV/node$i.pid"
    echo "node$i pid $! rpc :$((RPC_BASE + 2 * i)) ws :$((RPC_BASE + 2 * i + 1)) p2p :$((P2P_BASE + i))"
  done
  sleep 1
  status
}

stop() {
  local i pid
  for ((i = 0; i < N; i++)); do
    pid="$(cat "$DEV/node$i.pid" 2>/dev/null || true)"
    if pid_alive "$pid"; then
      kill "$pid" && echo "stopped node$i ($pid)"
      for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2>/dev/null || break; sleep 0.3; done
    fi
    rm -f "$DEV/node$i.pid"
  done
}

rpc() { # port method [params-json]
  local params="${3:-}"
  [[ -n "$params" ]] || params='{}'
  curl -s -m 3 -X POST "http://127.0.0.1:$1/" -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$2\",\"params\":$params}"
}

status() {
  local i pid
  for ((i = 0; i < N; i++)); do
    pid="$(cat "$DEV/node$i.pid" 2>/dev/null || true)"
    if pid_alive "$pid"; then
      echo "node$i pid $pid $(rpc $((RPC_BASE + 2 * i)) getChainInfo | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("height", r["height"], "tip", r["tip"][:16], "final", r["finalized_height"], "peers", r["peers"], "mempool", r["mempool_size"])' 2>/dev/null || echo '(rpc not up)')"
    else
      echo "node$i stopped"
    fi
  done
}

mine() {
  build
  local miner
  miner="$(pubkey_of_keyfile "$ROOT/keys/localnet/miner-keypair.json")"
  "$BIN" mine --rpc "http://127.0.0.1:$((RPC_BASE + 2 * ${DEVNET_MINE_NODE:-0}))" --miner "$miner" --blocks "${1:-0}"
}

reset() {
  stop
  rm -rf "$DEV"
  echo "removed $DEV"
}

case "${1:-status}" in
  init) init ;;
  start) start ;;
  stop) stop ;;
  status) status ;;
  mine) mine "${2:-0}" ;;
  reset) reset ;;
  *) echo "usage: $0 [open] init|start|stop|status|mine [N]|reset" >&2; exit 2 ;;
esac
