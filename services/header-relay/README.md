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
registration for PostJob / order BridgeUnlock txs once that block is canonical, reorg when
the node's branch is a heavier verified one, `mark_final` up to the final floor,
`accumulate`, and `close_header` (rent back) for its own final headers `retain_final` below
the floor. Job blocks need their job registered first, so the checkpoint must precede any
job still being mined.

Pipelined (src/relay.rs, 2026-10-18): `--parallel N` (default 8) headers are verified at
the same time, one work account each (`<work>-i.json`, about 0.49 SOL rent each on a real
cluster; `close-works` returns it). The light client takes a child on a merely submitted
parent and marks a header Verified only once its parent is (else WorkDone, then
`connect`), so nothing about soundness changes (SPEC 5.3 "Parallel relay"). Every pass:
one batched status poll, one batched account read for the window, the submit frontier
(two headers per tx), each flight's next stage, and one batch of connect / reorg /
mark_final / accumulate / close txs. Txs ask for the planned CU (`--full-cu` = 1.4M each).
Every `--lag-secs` (10) it prints

    lag {"node_tip":..,"lc_tip":..,"lc_final":..,"lag_blocks":..,"inflight":..,"parallel":8,"headers_per_min":..,...}

and `status` prints `node_height` and `lag_blocks`. Tests: `cargo test --release --test
catchup` (needs token/target/deploy/lightclient.so): the real pipeline against the program
in LiteSVM with simulated slots; sequential falls behind at one block per 4 slots,
parallel 8 drains a 30-block backlog in 67 slots and then stays within 3 blocks.

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

Speed: before the pipeline a fallback header took about 1.4 s (37 txs, 12 confirmation
rounds) on an idle `--ticks-per-slot 8` validator, just under the demo's 1.5 s blocks; at
load 30 (366 ms slots) it took 7.5 s and the relay fell 12,000 blocks behind. Now N headers
share those rounds.

Solana clock on a test validator: `submit_header` refuses a header more than
`max_future_ms` (the chain's 120 s) ahead of the Solana Clock sysvar. A
`solana-test-validator --ticks-per-slot 8` (50 ms nominal slots, what scripts/demo.sh uses)
whose real slots are slower (256-366 ms on the loaded test VPS) has a clock capped at
about 125 ms per slot, so it falls behind wall time by about 30 s per minute, and the light
client can then follow the chain only as fast as that clock (about 19 of 40 headers/min at
1.5 s blocks). The relay logs `header H is X s ahead of the Solana clock ...` and counts
`clock_waits` in the lag line. Real Solana's clock tracks wall time; on a localnet raise the
limit (`header-relay set-params --max-future-ms 86400000`, admin) or run the validator at
the default ticks per slot.

Historical proofs (2026-10-17): the relay proves each withdrawal against the header the
light client has final, so it no longer needs a final header carrying the node's
current root (that was a gap while withdrawals landed every block). Against an older
node without `at_height` it falls back to the current-root proof.
