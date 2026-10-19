#!/usr/bin/env bash
# Local Solana stand-in for the token chain (pump.fun, reserve, bridge, storefront).
#   token  = the one to use (default)
#   agent  = SUPERSEDED SVM agent chain (SPEC v1), kept only for reference; the real
#            agent chain is our own node, started with scripts/devnet.sh
# Usage: scripts/localnet.sh up|down|status [token|agent]   (default: token)
# Kills by the PID this script recorded, never by pattern.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN="$ROOT/.ledger"
mkdir -p "$RUN"

ports() {
  case "$1" in
    token) RPC=10600 FAUCET=10602 GOSSIP=10603 DYN=10604-10629 ;;
    agent) RPC=10630 FAUCET=10632 GOSSIP=10633 DYN=10634-10659 ;;
    *) echo "unknown chain $1" >&2; exit 1 ;;
  esac
}

up() {
  local c=$1; shift
  ports "$c"
  local pidf="$RUN/$c.pid"
  if [[ -f $pidf ]] && kill -0 "$(cat "$pidf")" 2>/dev/null; then
    echo "$c already up (pid $(cat "$pidf"))"; return
  fi
  for p in $RPC $((RPC+1)) $FAUCET $GOSSIP; do
    if lsof -ti :"$p" >/dev/null 2>&1; then echo "port $p is taken, refusing" >&2; exit 1; fi
  done
  local extra=()
  if [[ $c == token ]]; then
    # pump.fun programs + mainnet accounts pinned by scripts/pump-fixtures.sh
    local fx="$ROOT/fixtures/pump/validator-args.txt"
    [[ -f $fx ]] || { echo "missing $fx, run scripts/pump-fixtures.sh" >&2; exit 1; }
    # shellcheck disable=SC2207
    extra=($(sed "s#fixtures/#$ROOT/fixtures/#g" "$fx"))
  else
    # agent chain: the whole 1e18-lamport supply sits in the bridge escrow PDA
    extra=(--account GoszXkVtKwDgZSEWNjdPwuJ2eJGtohz6bCvnthNd5n5U
      "$ROOT/services/relayer/fixtures/escrow-account.json")
  fi
  # our programs at genesis, if built (upgrade authority = localnet payer)
  local payer; payer=$(solana-keygen pubkey "$ROOT/keys/localnet/payer-keypair.json")
  local progs; [[ $c == token ]] && progs="reserve bridge storefront lightclient" || progs="escrow agents jobs mining"
  for p in $progs; do
    local so="$ROOT/$c/target/deploy/$p.so"
    [[ -f $so ]] || { echo "note: $so not built, skipping" >&2; continue; }
    extra+=(--upgradeable-program "$(solana-keygen pubkey "$ROOT/keys/localnet/$p-keypair.json")" "$so" "$payer")
  done
  # tx history: solana-test-validator keeps 10000 shreds by default, about 6 s of the demo's
  # busy 50 ms slots, so getSignaturesForAddress forgot a finalize from a minute earlier
  # (demo step 11 "finalized by null"). LOCALNET_LEDGER_SHREDS overrides.
  solana-test-validator --ledger "$RUN/$c-ledger" --reset --quiet \
    --limit-ledger-size "${LOCALNET_LEDGER_SHREDS:-5000000}" \
    --bind-address 127.0.0.1 --rpc-port "$RPC" --faucet-port "$FAUCET" \
    --gossip-port "$GOSSIP" --dynamic-port-range "$DYN" ${extra[@]+"${extra[@]}"} "$@" \
    >"$RUN/$c.log" 2>&1 &
  echo $! >"$pidf"
  for _ in $(seq 60); do
    solana -u "http://127.0.0.1:$RPC" cluster-version >/dev/null 2>&1 && { echo "$c up on :$RPC (pid $(cat "$pidf"))"; return; }
    sleep 1
  done
  echo "$c failed to start, see $RUN/$c.log" >&2; exit 1
}

down() {
  local pidf="$RUN/$1.pid"
  [[ -f $pidf ]] || { echo "$1 not running"; return; }
  local pid; pid=$(cat "$pidf")
  if ps -p "$pid" -o command= 2>/dev/null | grep -q "$RUN/$1-ledger"; then
    kill "$pid" && echo "$1 stopped (pid $pid)"
  else
    echo "$1: pid $pid is not our validator, leaving it"
  fi
  rm -f "$pidf"
}

status() { ports "$1"; printf '%s :%s ' "$1" "$RPC"; solana -u "http://127.0.0.1:$RPC" cluster-version 2>/dev/null || echo down; }

cmd=${1:-status}; shift || true
chains=${1:-token}; shift || true
for c in $chains; do
  case $cmd in
    up) up "$c" "$@" ;;
    down) down "$c" ;;
    status) status "$c" ;;
  esac
done
