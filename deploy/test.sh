#!/usr/bin/env bash
# Test suites for the commit the devnet runs (on box 0, user lode): bash deploy/test.sh > ~/test.log
# JS LiteSVM suites (reserve, storefront npm test) abort with std::bad_alloc on these boxes
# (docs/HANDOFF.md item 13); run those on the Mac.
set -uo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/share/solana/install/active_release/bin:$HOME/.local/node/bin:$PATH"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
echo "commit $(git -C "$ROOT" rev-parse --short HEAD)"
fails=0
t() { local name="$1"; shift; local s=$(date +%s); local out; out="$("$@" 2>&1)"; local rc=$?
  local sum; sum="$(echo "$out" | awk '/^test result/{p+=$4; f+=$6; i+=$8; n++} /^# pass/{tp+=$3} /^# fail/{tf+=$3} END{if(n) printf "rust %d passed %d failed %d ignored", p, f, i; if(tp+tf) printf " node %d passed %d failed", tp, tf}')"
  echo "$([ $rc = 0 ] && echo PASS || echo FAIL) $name ($(( $(date +%s) - s )) s): $sum"; [ $rc = 0 ] || { fails=$((fails+1)); echo "$out" | tail -40; }; }
t chain bash -c "cd $ROOT/chain && cargo test --workspace --release -q"
t crates bash -c "cd $ROOT/crates && cargo test --workspace --release -q"
t pearl-cli bash -c "cd $ROOT/crates/pearl-cli && cargo test --release -q"
t header-relay bash -c "cd $ROOT/services/header-relay && cargo test --release -q"
t header-relay-devnet-build bash -c "cd $ROOT/services/header-relay && cargo build --release -q --features devnet"
t bridge-litesvm bash -c "cd $ROOT/token/tests/bridge-litesvm && cargo test --release -q"
t lightclient-litesvm bash -c "cd $ROOT/token/tests/lightclient-litesvm && cargo test --release -q"
for d in services/relayer services/storefront-x402; do t "$d" bash -c "cd $ROOT/$d && npm test --silent && npm run typecheck --silent"; done
t explorer bash -c "cd $ROOT/apps/explorer && npm test --silent"
echo "FAILS=$fails"
