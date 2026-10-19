#!/usr/bin/env bash
# On the devnet build box (box 0, user lode, repo ~/pearl-sol): build everything the devnet runs.
#   bash deploy/build.sh [rust|sbf|npm|all]   (default all; one heavy build at a time)
# Outputs: /opt/lode/bin/{agent-node,pearl-miner,pearl-attester,pearl-job,header-relay,pearl}
#          ~/devnet-so/{reserve,bridge,storefront,lightclient}.so  (--features devnet: the devnet program ids)
#          /opt/lode/idl/{reserve,storefront,bridge,lightclient}.json (IDL "address" = the devnet id)
set -euo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/share/solana/install/active_release/bin:$HOME/.local/node/bin:$PATH"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
what="${1:-all}"
t0=$(date +%s)
step() { echo "[$(( $(date +%s) - t0 )) s] $*"; }
if [[ $what == all || $what == rust ]]; then
  step "agent-node"; (cd "$ROOT/chain" && cargo build --release -q -p agent-node)
  step "pearl-job, pearl-miner, pearl-attester"; (cd "$ROOT/crates" && cargo build --release -q -p pearl-job -p pearl-miner -p pearl-attester)
  step "pearl (CLI)"; (cd "$ROOT/crates/pearl-cli" && cargo build --release -q)
  step "header-relay (devnet ids)"; (cd "$ROOT/services/header-relay" && cargo build --release -q --features devnet)
  install -m 755 "$ROOT/chain/target/release/agent-node" "$ROOT/crates/target/release/pearl-miner" "$ROOT/crates/target/release/pearl-attester" \
    "$ROOT/crates/target/release/pearl-job" "$ROOT/services/header-relay/target/release/header-relay" "$ROOT/crates/pearl-cli/target/release/pearl" /opt/lode/bin/
fi
if [[ $what == all || $what == sbf ]]; then
  # localnet build first (IDLs + the localnet .so the tests use), with the committed localnet program keypairs
  mkdir -p "$ROOT/token/target/deploy"
  for p in reserve bridge storefront lightclient; do cp -n "$ROOT/keys/localnet/$p-keypair.json" "$ROOT/token/target/deploy/" 2>/dev/null || true; done
  step "anchor build (localnet ids, IDLs)"; (cd "$ROOT/token" && anchor build >/dev/null)
  mkdir -p ~/devnet-so /opt/lode/idl
  for p in reserve bridge storefront lightclient; do
    step "$p.so (devnet ids)"
    (cd "$ROOT/token/programs/$p" && cargo build-sbf --features devnet --sbf-out-dir ~/devnet-so -- -q >/dev/null 2>&1) || (cd "$ROOT/token/programs/$p" && cargo build-sbf --features devnet --sbf-out-dir ~/devnet-so)
    var="$(echo "$p" | tr a-z A-Z)_PROGRAM"
    python3 - "$ROOT/token/target/idl/$p.json" "/opt/lode/idl/$p.json" "${!var}" <<'PY'
import json, sys
d = json.load(open(sys.argv[1])); d["address"] = sys.argv[3]; json.dump(d, open(sys.argv[2], "w"), indent=2)
PY
  done
  rm -f ~/devnet-so/*-keypair.json
  ls -l ~/devnet-so/*.so; sha256sum ~/devnet-so/*.so
fi
if [[ $what == all || $what == npm ]]; then
  for d in services/relayer services/crank services/storefront-x402; do step "npm ci $d"; (cd "$ROOT/$d" && npm ci --silent); done
fi
step "done"
