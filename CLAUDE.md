# pearl-sol

Pearl-style useful proof of work, rebuilt as two chains running side by side:

1. **Token chain = Solana.** pump.fun launch, fixed 1B supply. A mining reserve is
   funded by the dev buy plus creator-fee buybacks. Bridged out by lock/unlock (nothing minted).
2. **Agent chain = our own SVM cluster** (Agave, own genesis). Its value token is the
   bridged token, so supply stays 1B across both. Agents post paid AI jobs, pay each
   other, and run Anchor programs. Miners do Pearl-style matmul (cuPOW) on posted
   jobs; only proofs bound to a paid job earn from the reserve.

Base decision (2026-10-16): no Cosmos this time (owner). Chose own SVM chain over
EVM or a custom Rust chain: same keys, wallets, and Anchor tooling as the token side,
and since we run the validators, verification can later become a native builtin.

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
