# header-relay

Permissionless relay from the agent-chain node (RPC 10630) into the Solana light
client (`token/programs/lightclient`, ID `BUDUW2bGexRGRHhEkBUj8dAvffRuXQQacredxKtRMHK5`).
All progress lives on Solana (header accounts, tip, canonical ring), so it is
resumable: restart and it continues from the light client's tip.

    cargo build --release
    R=target/release/header-relay
    $R init --height 0 [--relay-reward L] [--retain-final N]   # checkpoint + params from getParams
    $R run [--once] [--work .header-relay/work-keypair.json] [--cu-price uL]

Per header: `submit_header`, `begin_verify` + proof upload (950-byte chunks), the verify
steps packed by `lightclient::client::plan` (CU table in `client::cost`), then job
registration for PostJob / order BridgeUnlock txs in that block, reorg when the
node's tip is a heavier verified branch, `mark_final` up to the final floor, and
`close_header` (rent back) for its own final headers `retain_final` below the floor.
Job blocks need their job registered first, so the checkpoint must precede any job
still being mined.

Devnet helpers (1-node devnet, own data dir; check `lsof -ti :10630` first):

    $R mine 3                 # fallback blocks via getBlockTemplate / submitBlock
    $R seed                   # BridgeUnlock (attester-signed), BridgeLock, PostJob + a job block, K+2 blocks
    $R harvest --out FILE     # headers, proofs, txs, final withdrawal proofs (token/tests/lightclient-litesvm/fixtures/devnet.json)

Live run 2026-10-17 (solana-test-validator :10600, 1-node devnet, K = 6): 18 headers
incl. one job block relayed, 583 txs, resumed after a restart.
