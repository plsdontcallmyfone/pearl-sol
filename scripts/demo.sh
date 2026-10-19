#!/usr/bin/env bash
# M1 end to end demo (SPEC 7 M1 item 6) on real local processes:
#   token chain  scripts/localnet.sh up token        (:10600, pump fixtures + reserve + bridge + storefront)
#   agent chain  scripts/devnet.sh with DEVNET_DIR=.demo/devnet (3 nodes, RPC 10630/10632/10634)
#   services     relayer :10660, attester :10661, miner (--deliver), crank, storefront-x402 :10662,
#                header-relay (light client relay + proof-mode settlement, no port)
# Steps, PASS/FAIL each, supply invariant after each: scripts/demo/run.ts.
#
#   scripts/demo.sh          build if needed, run all steps, tear everything down (exit 1 on any FAIL)
#   scripts/demo.sh down     stop whatever a previous run left (DEMO_KEEP_UP=1), by recorded PIDs
#
# Env: DEMO_TICKS_PER_SLOT (token validator, default 8 = 50 ms slots), DEMO_KEEP_UP=1 (leave the stack running), DEMO_K (finality depth, default 6),
#      DEMO_BLOCK_MS (target block time, default 1500), DEMO_MINER_THREADS (default 4),
#      CADRE_MODEL_DIR (quantized model for the inference step, default ~/cadre-models/llama-3.2-1b-w7a7;
#      tools/cadre/quantize.py makes it), CADRE_PYTHON (python with torch + transformers for the worker),
#      DEMO_STEPS (comma list of step numbers to run, default all; 1-6 are the setup the later steps need).
# Kills by PID only (never by pattern). Never touches `solana config`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEMO="$ROOT/.demo"
export DEMO_DIR="$DEMO"

down() {
  local f pid
  for f in "$DEMO"/pids/*.pid; do
    [[ -f $f ]] || continue
    pid="$(cat "$f")"
    # only our own processes: check what the PID is now before killing it
    if kill -0 "$pid" 2>/dev/null && ps -p "$pid" -o command= | grep -Eq "pearl-miner|pearl-attester|header-relay|cadre-gateway|tools/cadre/worker.py|src/main.ts"; then
      kill "$pid" && echo "stopped $(basename "$f" .pid) ($pid)"
    fi
    rm -f "$f"
  done
  DEVNET_DIR="$DEMO/devnet" bash "$ROOT/scripts/devnet.sh" stop || true
  bash "$ROOT/scripts/localnet.sh" down token || true
}

if [[ ${1:-} == down ]]; then down; exit 0; fi

# --- preflight: our ports must be free (a previous run may still be up)
busy=""
for p in 10600 10601 10602 10603 10630 10631 10632 10633 10634 10635 10640 10641 10642 10660 10661 10662 10664 10665; do
  # listeners only (TCP LISTEN or a bound UDP port): a client of the old stack (e.g. a service polling 10600) does not own the port
  holders="$( (lsof -ti tcp:$p -sTCP:LISTEN; lsof -ti udp:$p) 2>/dev/null | sort -u | tr '\n' ' ')"
  if [[ -n "${holders// /}" ]]; then busy+=" $p(pid $holders)"; fi
done
if [[ -n $busy ]]; then
  echo "ports in use:$busy" >&2
  echo "if a previous demo is still up: scripts/demo.sh down" >&2
  exit 1
fi

# --- fresh demo state (keys, devnet data, relayer/storefront state, logs)
rm -rf "$DEMO"
mkdir -p "$DEMO"

# --- builds (one heavy build at a time)
t=$(date +%s)
echo "building agent-node (chain/, pinned toolchain)..."
(cd "$ROOT/chain" && cargo build --release -q -p agent-node)
echo "building pearl-job, pearl-miner, pearl-attester, cadre-gateway (crates/)..."
(cd "$ROOT/crates" && cargo build --release -q -p pearl-job -p pearl-miner -p pearl-attester -p cadre-gateway)
echo "building the pearl CLI (crates/pearl-cli)..."
(cd "$ROOT/crates/pearl-cli" && cargo build --release -q)
echo "building header-relay (services/header-relay)..."
(cd "$ROOT/services/header-relay" && cargo build --release -q)
for d in relayer crank storefront-x402; do
  [[ -d "$ROOT/services/$d/node_modules" ]] || (cd "$ROOT/services/$d" && npm ci --silent)
done
echo "builds ready in $(( $(date +%s) - t )) s"
for so in reserve bridge storefront lightclient; do
  [[ -f "$ROOT/token/target/deploy/$so.so" ]] || { echo "missing token/target/deploy/$so.so: cd token && anchor build" >&2; exit 1; }
done

# the driver resolves web3.js etc. through the relayer's node_modules (one module instance)
ln -sfn ../../services/relayer/node_modules "$ROOT/scripts/demo/node_modules"

trap 'status=$?; if [[ ${DEMO_KEEP_UP:-0} != 1 ]]; then down >/dev/null 2>&1 || true; fi; exit $status' EXIT
node "$ROOT/scripts/demo/run.ts"
