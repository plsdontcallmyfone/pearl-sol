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

Proof-mode settlement (`run --settle`, src/settle.rs): when the bridge has
`release_modes` proof on, it calls `bridge.release_proof` for each final withdrawal not
yet Processed, with the node's historical MMR proof (getWithdrawalProof `at_height` =
the light client's final height) against that final light-client header's
`withdraw_root` (the header's hash must match the node's); when the storefront has `finalize_modes` proof
on, it calls `finalize_order_proof` (CompleteJob tx + tx_root path + ed25519 precompile
of the evaluator signature) or `expire_order_proof` for Pending orders whose chain job
closed in a final light-client header. The payer must be the order's buyer, the
storefront admin or an attester. `status` prints the light client's tip / final as JSON.

Speed: confirmations are polled every 50 ms and proof chunks go out as one batch. On a
`--ticks-per-slot 8` validator (50 ms slots, what scripts/demo.sh uses) a fallback header
takes about 1.4 s (37 txs); at the default 400 ms slots about 20 s before these changes.
4 ticks per slot was slower (about 14 s per header; confirmations lag).

Historical proofs (2026-10-17): the relay proves each withdrawal against the header the
light client has final, so it no longer needs a final header carrying the node's
current root (that was a gap while withdrawals landed every block). Against an older
node without `at_height` it falls back to the current-root proof.
