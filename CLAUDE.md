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
| Claude subagent (M1 integration) | 2026-10-16 | scripts/demo.sh, scripts/demo/, fixes in any component for the real-validator e2e run (IN PROGRESS) |
| Research subagent (Commonware) | 2026-10-16 | research/NOTES-commonware.md only (DONE, uncommitted) |
| Claude subagent (storefront) | 2026-10-16 | token/programs/storefront, token/tests/storefront-litesvm, services/storefront-x402, keys/localnet/storefront-keypair.json, storefront entry in token/Anchor.toml, no bridge changes needed (DONE, committed 18f3cd0) |
| Claude subagent (miner v2) | 2026-10-16 | crates/pearl-job (pow module), crates/pearl-node (node RPC client, tx encoding, mock node), services/miner (block producer), services/attester (node mode) (DONE, committed) |
| Claude subagent (chain node) | 2026-10-16 | chain/ (Cargo workspace: agent-types, state, executor, consensus-pow, sync, net, mempool, rpc, bin/agent-node), docs/RPC.md, scripts/devnet.sh, .devnet/ (IN PROGRESS) |
| Claude subagent (relayer v2) | 2026-10-16 | services/relayer only: rewire to agent-chain node RPC (BridgeUnlock/BridgeLock, K-deep), order deposits, finalize_order, invariant (DONE) |
