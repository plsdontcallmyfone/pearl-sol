#!/usr/bin/env bash
# Solana devnet side of the public Cadre devnet (docs/DEVNET.md), from the Mac.
#   scripts/devnet-solana.sh balances        deployer / ops / storefront wallet SOL
#   scripts/devnet-solana.sh deploy [PROG]   deploy (first time) or upgrade the devnet programs (all, or one of
#                                            reserve bridge storefront lightclient) from ~/.config/pearl-sol/devnet-so
#                                            (fetched from box 0 ~/devnet-so); upgrade authority = deployer
#   scripts/devnet-solana.sh fund SOL        deployer -> ops (SOL), and 0.5 SOL -> storefront wallet
#   scripts/devnet-solana.sh init            one-time on-chain setup (scripts/devnet-init/init.ts), light client init on
#                                            box 0, proof modes, then starts relayer, header-relay, storefront, crank
# Always -u and -k explicit; never touches `solana config`. The deployer key never leaves the Mac.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
set -a
# shellcheck disable=SC1091
source "$ROOT/deploy/devnet.env"
set +a
K="$ROOT/keys/devnet"
SO="${DEVNET_SO:-$HOME/.config/pearl-sol/devnet-so}"
U=(-u "$SOLANA_RPC")
pub() { solana-keygen pubkey "$K/$1-keypair.json"; }
case "${1:-}" in
  balances) for k in deployer ops storefront-wallet; do echo "$k $(pub $k) $(solana balance "${U[@]}" "$(pub $k)")"; done ;;
  deploy)
    for p in ${2:-reserve bridge storefront lightclient}; do
      size=$(stat -f %z "$SO/$p.so" 2>/dev/null || stat -c %s "$SO/$p.so")
      if solana program show "${U[@]}" "$(pub $p)" >/dev/null 2>&1; then
        cur=$(solana program show "${U[@]}" "$(pub $p)" | awk '/Data Length/{print $3}')
        if (( size > cur )); then solana program extend "${U[@]}" -k "$K/deployer-keypair.json" "$(pub $p)" $(( size - cur + 4096 )); fi
        echo "upgrade $p ($size bytes)"
        solana program deploy "${U[@]}" -k "$K/deployer-keypair.json" --program-id "$(pub $p)" --upgrade-authority "$K/deployer-keypair.json" "$SO/$p.so"
      else
        max=$(( size * 11 / 10 ))
        echo "deploy $p ($size bytes, max-len $max, programdata rent $(solana rent "${U[@]}" $((max + 45)) | head -1))"
        solana program deploy "${U[@]}" -k "$K/deployer-keypair.json" --program-id "$K/$p-keypair.json" --upgrade-authority "$K/deployer-keypair.json" --max-len "$max" "$SO/$p.so"
      fi
    done ;;
  fund)
    solana transfer "${U[@]}" -k "$K/deployer-keypair.json" --allow-unfunded-recipient "$(pub ops)" "$2"
    solana transfer "${U[@]}" -k "$K/deployer-keypair.json" --allow-unfunded-recipient "$(pub storefront-wallet)" 0.5 ;;
  init)
    ln -sfn ../../services/relayer/node_modules "$ROOT/scripts/devnet-init/node_modules"
    mkdir -p "$ROOT/.devnet-e2e"
    node "$ROOT/scripts/devnet-init/init.ts"
    "$ROOT/scripts/devnet-deploy.sh" ssh 0 "/opt/lode/bin/header-relay --node http://127.0.0.1:10630 --solana $SOLANA_RPC --payer /etc/lode/keys/ops.json status | grep -q '\"initialized\":true' || /opt/lode/bin/header-relay --node http://127.0.0.1:10630 --solana $SOLANA_RPC --payer /etc/lode/keys/ops.json init"
    node "$ROOT/scripts/devnet-init/init.ts"
    "$ROOT/scripts/devnet-deploy.sh" ctl 0 enable --now cadre-relayer cadre-header-relay cadre-storefront cadre-crank ;;
  *) sed -n '2,10p' "$0"; exit 1 ;;
esac
