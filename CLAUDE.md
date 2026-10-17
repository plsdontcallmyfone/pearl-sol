# pearl-sol

Pearl-style useful proof of work, rebuilt as two chains running side by side:

1. **Token chain = Solana.** pump.fun launch, fixed 1B supply. A mining reserve is
   funded by the dev buy plus creator-fee buybacks. Bridged out by lock/unlock (nothing minted).
2. **Agent chain = our OWN chain**, written by us in Rust on Commonware library
   primitives (p2p, storage, runtime, crypto). Not a fork of any chain. Blocks are
   produced by Pearl-style useful proof of work: miners do matmul on posted AI jobs.
   Its native coin is the bridged token (no minting; whole supply preallocated to
   the bridge escrow), so supply stays 1B across both chains.

Base decisions (owner, 2026-10-16): no Cosmos; then NOT SVM either ("completely
different"); no full fork of anything, core-infra libraries are fine; Commonware;
useful-PoW blocks. The SVM agent-chain code in agent/ (Anchor) is SUPERSEDED and
kept only as reference for the job/mining/escrow logic; the SVM-era integration
demo and extension WIP sit on branch wip/svm-agent-chain. Still valid: token/
(Solana reserve + bridge), fixtures/pump, crates/pearl-job (chain-agnostic Pearl
proofs), services/relayer and crank (token side).

**Start with `docs/HANDOFF.md`** (current state and next step; update it after every step). Spec: `docs/SPEC.md`. Research from the 2026-10-15 session: `research/`
(`research/pearlrepo` is a clone of github.com/pearl-research-labs/pearl @ 837c69c,
ISC, gitignored; re-clone if missing).

## Ports (owned block 10600-10699)

    10600 / 10601   token-chain localnet (Solana stand-in) RPC / WS, faucet 10602
    10603-10629     token-chain gossip + dynamic range
    10630 / 10631   agent-chain localnet RPC / WS, faucet 10632
    10633-10659     agent-chain gossip + dynamic range
    10660-10669     relayer, indexer, miner/pool services
    10670-10679     web frontends

Validators run LOCALLY on this Mac via scripts/localnet.sh: the owner told this
repo to proceed locally on 2026-10-16, an exception to the global claude-host rule
(syncing to claude-host was blocked by the permission classifier). Keep it to
these two validators and stop them when idle.

`lsof -ti :<port>` before binding. Kill by PID only. Always pass `-u` and `-k`
explicitly; never touch `solana config`.

## Who is working on what

| Who | Started | Scope |
|-----|---------|-------|
| Claude session f2b59a9f (orchestrator) | 2026-10-16 | SPEC.md, repo layout, M1 localnet coordination |
| Research subagent (chain/bridge) | 2026-10-16 | research/NOTES-chain-bridge.md only (DONE, uncommitted) |
| Claude subagent (pump research) | 2026-10-16 | research/NOTES-pump.md + research/pump-litesvm/ (DONE, uncommitted) |
| Claude subagent (proof research) | 2026-10-16 | research/NOTES-proof.md only (DONE, uncommitted) |
| Claude subagent (bridge/escrow/relayer) | 2026-10-16 | token/programs/bridge, agent/programs/escrow, services/relayer, token/tests/bridge*, agent/tests/escrow* (DONE) |
| Claude subagent (reserve) | 2026-10-16 | token/programs/reserve, token/tests/reserve*, services/crank, fixtures/pump/, scripts/pump-fixtures.sh, token branch of scripts/localnet.sh (DONE) |
| Claude subagent (proof crates) | 2026-10-16 | crates/ (pearl-job, pearl-chain, chain-litesvm), services/miner, services/attester, keys/localnet/{attester,miner}-keypair.json (DONE, committed) |
| Claude subagent (market) | 2026-10-16 | agent/programs/{agents,jobs,mining}, agent/tests/{agents,jobs,mining}*, services/agent-cli (DONE, committed) |
| Claude subagent (M1 integration v2) | 2026-10-16 | scripts/demo.sh, scripts/demo/, minimal fixes in any component (not extension/) for the real-process e2e run: localnet token + devnet + relayer/attester/miner/crank/storefront-x402 (DONE: all 7 steps PASS, see HANDOFF Demo) |
| Research subagent (Commonware) | 2026-10-16 | research/NOTES-commonware.md only (DONE, uncommitted) |
| Claude subagent (storefront) | 2026-10-16 | token/programs/storefront, token/tests/storefront-litesvm, services/storefront-x402, keys/localnet/storefront-keypair.json, storefront entry in token/Anchor.toml, no bridge changes needed (DONE, committed 18f3cd0) |
| Claude subagent (miner v2) | 2026-10-16 | crates/pearl-job (pow module), crates/pearl-node (node RPC client, tx encoding, mock node), services/miner (block producer), services/attester (node mode) (DONE, committed) |
| Claude subagent (chain node) | 2026-10-16 | chain/ (Cargo workspace: agent-types, state, executor, consensus-pow, sync, net, mempool, rpc, bin/agent-node), docs/RPC.md, scripts/devnet.sh, .devnet/ (DONE) |
| Claude subagent (wallet extension v2) | 2026-10-16 | extension/ only (MV3 wallet for Solana + our chain; dev/mock ports 10670-10679) (DONE: 51 unit + 25 headless UI checks, mock RPC; see extension/README.md) |
| Claude subagent (relayer v2) | 2026-10-16 | services/relayer only: rewire to agent-chain node RPC (BridgeUnlock/BridgeLock, K-deep), order deposits, finalize_order, invariant (DONE) |
| Claude subagent (wallet extension v3) | 2026-10-16 | extension/ only: order_terms header in TS, Buy form computes hash_a/hash_b, tab bar overlap, live check on scripts/demo.sh stack; plus chain/state/tests/order_terms_vector.rs + chain/state/vectors/order_terms.json (DONE: 56 unit + 45 UI checks, live 37/37 smoke + 9/9 writes) |
| Claude subagent (M2 light client) | 2026-10-17 | token/programs/lightclient, crates/pearl-verify-sbf, proof mode in token/programs/{bridge,storefront}, services/header-relay, token/tests/lightclient*, keys/localnet/lightclient-keypair.json, lightclient entry in token/Anchor.toml (DONE: 7 verifier + 10 LiteSVM incl. real devnet fixture, live relay on :10600; commits f5caf0f..27324cd; follow-up: re-measure once chain fallback_m/n params land) |
| Claude subagent (M2 open net) | 2026-10-17 | chain/net (open-membership p2p on commonware-stream), chain/bin/agent-node net wiring (--net open --seeds), scripts/devnet.sh open mode, net tests; p2p 10640-10649, RPC 10630-10639 (DONE: 15 net + 4 node tests, live 5-node open devnet + pearl-miner 15/15 blocks; commits 0eccee0..a557c24) |
| Claude subagent (M2 mining pools) | 2026-10-17 | services/pool (new, :10663), services/miner pool mode (--pool), pool tests (DONE: 8 pool + 4 miner pool-mode tests; live 3-node devnet run, 2 pool miners, 304 blocks, 85 PPLNS payouts landed, ledger delta 0) |
| Claude subagent (M2 data availability + leftovers) | 2026-10-17 | chain/ (blob store, state, executor, rpc, bin/agent-node), docs/RPC.md, docs/NODE.md, crates/pearl-node, services/miner, services/attester (blob: fetch), small extension/ change (blob uri, getBlock subsidy), scripts/demo (blob URIs); ports 10630-10649 (DONE: 66 chain tests pass incl. 3 blob/DA network + 3 chain-level M2 + 2 executor; live 5-node open devnet: blob put on node 1, mined + delivered (C as blob) by a miner on node 3, attested via node 2, node 4 offline fetched both after catch-up, pruned after grace; demo.sh 7/7 PASS with blob data; commits b717103..) |
| Claude subagent (rank penalty) | 2026-10-17 | crates/pearl-job (difficulty bound + verify), crates/pearl-verify-sbf, token/programs/lightclient, chain/consensus-pow (verifier call / bound only), services/miner + services/pool if needed, PROOF-FORMAT.md, vectors, research/NOTES-rank-penalty.md (DONE: Pearl rule r >= 128 + bound h*w*(dlen/r)*128, default k 2048 r 128; zero matrices 0.82x honest work, not 1/16; 36 pearl-job + 9 SBF verifier + 10 consensus + 10 LiteSVM (+1 ignored, pre-penalty devnet fixture); job header 7.25M CU / 34 txs; commits 82e002f..) |
| Claude subagent (M2 wrap-up) | 2026-10-17 | services/relayer (min-fee refuse + refund state), token/tests/lightclient-litesvm fixtures + replay test, services/header-relay (harvest fixes only), scripts/demo.sh + scripts/demo/ (light client + proof mode), services/pool (share target retune), minimal fixes elsewhere; ports 10600-10669 (DONE: relayer 28 tests incl. min-fee hold + proof mode, fixture re-harvested + replay un-ignored, header-relay --settle, demo 8/8 PASS 122.8 s with finalize_order_proof + release_proof, pool factor 11; refund gap reported; commits 42e7958..) |
| Claude subagent (refund + historical proofs) | 2026-10-17 | token/programs/{bridge,storefront} (refund_deposit, Order Refunded), token/tests/{bridge,storefront}*, services/relayer (refund submit), chain/ (getWithdrawalProof at_height: state/rpc only), services/header-relay (settle at light-client final), token/tests/lightclient*, scripts/demo/ refund step, docs SPEC 5.1/5.3 + RPC + HANDOFF; ports 10600-10669 |
