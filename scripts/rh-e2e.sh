#!/usr/bin/env bash
# RH side end to end, fully LOCAL (docs/RH-TESTNET-PLAN.md 4.5 "Anvil e2e"):
#   anvil (chain id 46630, 1 s blocks, so the `safe` tag moves like a real L2)
#   + one local agent-node with bridge v2 activated by AdminConfig (keys 51, 50)
#   + the token-evm contracts deployed with forge script (Deploy.s.sol, real v4 PoolManager)
#   + the RH relayer (RELAYER_SIDE=rh) and the header-oracle poster, as their own processes.
# Steps (services/relayer/e2e/rh-e2e.ts): deposit -> BridgeUnlockV2; BridgeLockV2 -> release
# (signer mode); BridgeLockV2 -> oracle post -> releaseProof (proof mode); storefront order ->
# job -> finalizeOrder; refund; exact per-side invariant.
#
#   scripts/rh-e2e.sh            run (stops everything it started at the end)
#   scripts/rh-e2e.sh --keep     leave the stack up (stop later with: scripts/rh-e2e.sh stop)
#   scripts/rh-e2e.sh stop       stop a kept stack (by the PIDs it recorded)
#
# Ports (Mac block 10750-10759, checked with lsof before binding; see CLAUDE.md):
#   10750 anvil   10752/10753 agent-node RPC/WS   10754 agent-node p2p
#   10755 RH relayer health   10756 header-oracle poster health
# Keys are fresh throwaway keys made in $RH_E2E_DIR/keys on every run; anvil's default
# accounts are never used (EVM keys get ETH by anvil_setBalance). Nothing touches a public
# network. Processes are stopped by recorded PID only.
#
# Env: RH_E2E_DIR (default .rh-e2e), AGENT_NODE (default chain/target/release/agent-node,
# built with debuginfo off if missing), RH_E2E_FINALITY (default safe).
set -euo pipefail
export NODE_OPTIONS="${NODE_OPTIONS:-} --no-deprecation"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="${RH_E2E_DIR:-$ROOT/.rh-e2e}"
REL="$ROOT/services/relayer"
EVM="$ROOT/token-evm"
BIN="${AGENT_NODE:-$ROOT/chain/target/release/agent-node}"
P_ANVIL=10750 P_RPC=10752 P_WS=10753 P_P2P=10754 P_RELAYER=10755 P_ORACLE=10756
RH_RPC="http://127.0.0.1:$P_ANVIL"
NODE_RPC="http://127.0.0.1:$P_RPC"
CADRE_CHAIN_ID=1380533301
PROCS=(relayer oracle miner node anvil)

log() { printf '[rh-e2e %s] %s\n' "$(date +%H:%M:%S)" "$*"; }

pid_of() { cat "$DIR/$1.pid" 2>/dev/null || true; }
stop_one() {
  local n="$1" pid
  pid="$(pid_of "$n")"
  # kill by identity: the recorded PID must still be the process this script started
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null && ps -p "$pid" -o command= | grep -qE 'agent-node (run|mine)|mine-loop\.sh|^anvil --port '"$P_ANVIL"'|node src/(main|evm/oracle-main)\.ts'; then
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
    log "stopped $n ($pid)"
  fi
  rm -f "$DIR/$n.pid"
}
stop_all() { for n in "${PROCS[@]}"; do stop_one "$n"; done; }

if [[ "${1:-}" == "stop" ]]; then stop_all; exit 0; fi
KEEP=0
[[ "${1:-}" == "--keep" ]] && KEEP=1

for t in anvil forge cast node python3; do command -v "$t" >/dev/null || { echo "missing $t" >&2; exit 1; }; done
for p in $P_ANVIL $P_RPC $P_WS $P_P2P $P_RELAYER $P_ORACLE; do
  if [[ -n "$(lsof -ti :$p 2>/dev/null)" ]]; then echo "port $p in use (pid $(lsof -ti :$p | tr '\n' ' ')); not starting" >&2; exit 1; fi
done
if [[ ! -x "$BIN" ]]; then
  log "building agent-node (release, debuginfo off)"
  (cd "$ROOT/chain" && CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --release -p agent-node)
fi
[[ -d "$REL/node_modules/viem" ]] || (cd "$REL" && npm install --no-audit --no-fund)
[[ -f "$EVM/lib/forge-std/src/Script.sol" ]] || (cd "$ROOT" && git submodule update --init --recursive token-evm)

rm -rf "$DIR"
mkdir -p "$DIR"
trap '[[ $KEEP == 1 ]] || stop_all' EXIT
T0=$(date +%s)

# ---------------------------------------------------------------- keys
(cd "$REL" && node e2e/rh-e2e.ts keys "$DIR") > "$DIR/keys.log"
idx() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$DIR/keys/index.json" "$1"; }
K="$DIR/keys"

# ---------------------------------------------------------------- agent chain (one node)
seed="$(printf 'rh-e2e node %s %s %s' "$(date +%s)" "$$" "$RANDOM" | shasum -a 256 | cut -d' ' -f1)"
cat > "$DIR/genesis.json" <<EOF
{
  "chain_id": $CADRE_CHAIN_ID,
  "timestamp_ms": $(( $(date +%s) * 1000 )),
  "target_bits": "0x207fffff",
  "admin": "$(idx admin)",
  "attesters": ["$(idx attester)"],
  "attester_threshold": 1,
  "params": {"target_block_time_ms": 1000, "wtema_half_life_ms": 600000, "finality_k": 4, "min_fee": 0}
}
EOF
cat > "$DIR/node.json" <<EOF
{
  "data_dir": "$DIR/node/data",
  "genesis": "$DIR/genesis.json",
  "p2p_key": "$seed",
  "p2p_listen": "127.0.0.1:$P_P2P",
  "rpc_listen": "127.0.0.1:$P_RPC",
  "ws_listen": "127.0.0.1:$P_WS",
  "peers": [{"key": "$("$BIN" pubkey "$seed")", "addr": "127.0.0.1:$P_P2P"}],
  "verifier": "pearl",
  "tip_interval_ms": 1000
}
EOF
mkdir -p "$DIR/node"
RUST_LOG=warn nohup "$BIN" run --config "$DIR/node.json" > "$DIR/node.log" 2>&1 &
echo $! > "$DIR/node.pid"; disown
for _ in $(seq 1 60); do curl -s -m 1 -X POST "$NODE_RPC" -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getChainInfo","params":{}}' | grep -q chain_id && break; sleep 0.5; done
# about 2 blocks a second (a free-running fallback miner makes dozens a second; the devnet's are 30 s apart)
cat > "$DIR/mine-loop.sh" <<EOF
while :; do "$BIN" mine --rpc "$NODE_RPC" --miner "$(idx miner)" --blocks 1 >/dev/null 2>&1; sleep 0.5; done
EOF
nohup bash "$DIR/mine-loop.sh" > "$DIR/miner.log" 2>&1 &
echo $! > "$DIR/miner.pid"; disown
log "agent-node pid $(pid_of node) rpc :$P_RPC, miner pid $(pid_of miner)"

# ---------------------------------------------------------------- anvil + contracts
anvil --port "$P_ANVIL" --chain-id 46630 --block-time 1 --silent > "$DIR/anvil.log" 2>&1 &
echo $! > "$DIR/anvil.pid"; disown
for _ in $(seq 1 40); do cast chain-id --rpc-url "$RH_RPC" >/dev/null 2>&1 && break; sleep 0.25; done
for n in deployer signer0 signer1 signer2 user buyer dest; do
  cast rpc --rpc-url "$RH_RPC" anvil_setBalance "$(idx $n)" 0x3635C9ADC5DEA00000 >/dev/null # 1000 ETH, local only
done
log "anvil pid $(pid_of anvil) on :$P_ANVIL (chain id $(cast chain-id --rpc-url "$RH_RPC"))"

DPK="$(cat "$K/deployer.key")"
SIGNERS="$(idx signer0),$(idx signer1),$(idx signer2)"
cd "$EVM"
PM="$(forge create lib/v4-core/src/PoolManager.sol:PoolManager --rpc-url "$RH_RPC" --private-key "$DPK" --broadcast \
  --constructor-args "$(idx deployer)" 2>&1 | awk '/Deployed to:/ {print $3}')"
[[ -n "$PM" ]] || { echo "PoolManager deploy failed" >&2; exit 1; }
mining_pool="0x$(printf '11%.0s' $(seq 1 32))" # any chain account; the e2e does not fund mining
CADRE_CHAIN_ID=$CADRE_CHAIN_ID MINING_POOL="$mining_pool" ATTESTERS="$SIGNERS" SIGNERS="$SIGNERS" STORE_ATTESTERS="$SIGNERS" \
  ATTESTER_THRESHOLD=2 SIGNER_THRESHOLD=2 STORE_THRESHOLD=2 POOL_MANAGER="$PM" DEPLOYER="$(idx deployer)" \
  DEPLOY_OUT=deployments/rh-e2e-local.json \
  forge script script/Deploy.s.sol --rpc-url "$RH_RPC" --broadcast --private-key "$DPK" --sender "$(idx deployer)" --slow > "$DIR/deploy.log" 2>&1 \
  || { tail -30 "$DIR/deploy.log" >&2; exit 1; }
mv deployments/rh-e2e-local.json "$DIR/deploy.json"
rm -rf broadcast/Deploy.s.sol/46630 cache/Deploy.s.sol/46630
cd "$ROOT"
jq() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$DIR/deploy.json" "$1"; }
log "deployed: PoolManager $PM, bridge $(jq bridge), storefront $(jq storefront), oracle $(jq headerOracle)"

# ---------------------------------------------------------------- bridge v2 on the chain
(cd "$REL" && NODE_RPC="$NODE_RPC" node e2e/rh-e2e.ts activate "$DIR")

# ---------------------------------------------------------------- relayer (rh) + oracle poster
COMMON=(RH_RPC="$RH_RPC" RH_CHAIN_ID=46630 NODE_RPC="$NODE_RPC" RH_SIGNER_KEY="$K/signer0.key"
  RH_ATTESTER_KEYS="$K/signer0.key,$K/signer2.key" POLL_MS=1500)
BRIDGE="$(jq bridge)" STORE="$(jq storefront)" ORACLE="$(jq headerOracle)"
cd "$REL"
# started straight from this shell so $! is the node process itself (stopped by that PID)
env "${COMMON[@]}" RELAYER_SIDE=rh RH_BRIDGE="$BRIDGE" RH_STOREFRONT="$STORE" \
  RH_FINALITY="${RH_E2E_FINALITY:-safe}" RH_STATE_FILE="$DIR/relayer-rh.json" RH_REFUND_GRACE_MS=5000 \
  SIGNER_KEYS="$K/attester-keypair.json" AGENT_KEY="$K/agent-keypair.json" HEALTH_PORT=$P_RELAYER \
  node src/main.ts > "$DIR/relayer.log" 2>&1 &
echo $! > "$DIR/relayer.pid"; disown
env "${COMMON[@]}" RH_ORACLE_SENDER_KEY="$K/signer2.key" RH_HEADER_ORACLE="$ORACLE" RH_BRIDGE="$BRIDGE" ORACLE_EVERY=10 HEALTH_PORT=$P_ORACLE \
  node src/evm/oracle-main.ts > "$DIR/oracle.log" 2>&1 &
echo $! > "$DIR/oracle.pid"; disown
cd "$ROOT"
sleep 3
for n in relayer oracle; do kill -0 "$(pid_of $n)" 2>/dev/null || { echo "$n died:" >&2; tail -20 "$DIR/$n.log" >&2; exit 1; }; done
log "relayer pid $(pid_of relayer) (health :$P_RELAYER), oracle poster pid $(pid_of oracle) (health :$P_ORACLE); setup $(( $(date +%s) - T0 )) s"

# ---------------------------------------------------------------- steps
set +e
(cd "$REL" && RH_RPC="$RH_RPC" NODE_RPC="$NODE_RPC" RELAYER_HEALTH="http://127.0.0.1:$P_RELAYER/health" \
  ORACLE_HEALTH="http://127.0.0.1:$P_ORACLE/health" node e2e/rh-e2e.ts run "$DIR")
rc=$?
set -e
log "total $(( $(date +%s) - T0 )) s; logs and transcript.json in $DIR"
if [[ $KEEP == 1 ]]; then log "stack left up: scripts/rh-e2e.sh stop"; fi
exit $rc
