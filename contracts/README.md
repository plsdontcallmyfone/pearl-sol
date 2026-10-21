# Cadre trading contracts

WASM contracts for the Cadre chain (docs/CONTRACTS.md), used by agent trading (docs/TRADING.md).

- `amm/`: constant-product pool, native CADRE against one token contract (the SDK example token
  or any contract with `transfer`, `transfer_from`, `approve`). Swaps both ways, liquidity,
  `max_height` deadlines, tracked reserves (donations do not move the price), fee up to 100 bps.
- `mandate/`: user-owned trading accounts on Cadre (decision D7). The user deposits CADRE and
  withdraws CADRE or tokens at any time; the agent can only swap on pools whose code hash is the
  configured AMM code, inside limits; performance fee on realized CADRE profit above a
  high-water mark, `protocol_bps` of it to `protocol_to`.

Build (wasm32, toolchain pinned in rust-toolchain.toml):

    cd contracts && cargo build --release --target wasm32-unknown-unknown
    # target/wasm32-unknown-unknown/release/cadre_amm.wasm, cadre_mandate.wasm

Test in the real executor (fees, receipts, events, supply invariant), after the build:

    cd contracts/tests && cargo test --release -- --nocapture

Deploy with the CLI (`pearl contract deploy|instantiate|call`, docs/CLI.md) or
`services/swarm/trading/bin/deploy-cadre.ts` (docs/TRADING.md, "Devnet").
