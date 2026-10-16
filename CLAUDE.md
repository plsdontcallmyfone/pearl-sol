# pearl-sol

Pearl-style useful proof of work, rebuilt as two chains running side by side:

1. **Token chain = Solana.** pump.fun launch, fixed 1B supply. A mining reserve is
   funded by the dev buy plus creator-fee buybacks. Lock-and-mint bridge out.
2. **Agent chain = our own SVM cluster** (Agave, own genesis). Its value token is the
   bridged token, so supply stays 1B across both. Agents post paid AI jobs, pay each
   other, and run Anchor programs. Miners do Pearl-style matmul (cuPOW) on posted
   jobs; only proofs bound to a paid job earn from the reserve.

Base decision (2026-10-16): no Cosmos this time (owner). Chose own SVM chain over
EVM or a custom Rust chain: same keys, wallets, and Anchor tooling as the token side,
and since we run the validators, verification can later become a native builtin.

Spec: `docs/SPEC.md`. Research from the 2026-10-15 session: `research/`
(`research/pearlrepo` is a clone of github.com/pearl-research-labs/pearl @ 837c69c,
ISC, gitignored; re-clone if missing).

## Ports (owned block 10600-10699)

    10600 / 10601   token-chain localnet (Solana stand-in) RPC / WS, faucet 10602
    10603-10629     token-chain gossip + dynamic range
    10630 / 10631   agent-chain localnet RPC / WS, faucet 10632
    10633-10659     agent-chain gossip + dynamic range
    10660-10669     relayer, indexer, miner/pool services
    10670-10679     web frontends

`lsof -ti :<port>` before binding. Kill by PID only. Always pass `-u` and `-k`
explicitly; never touch `solana config`.

## Who is working on what

| Who | Started | Scope |
|-----|---------|-------|
| Claude session f2b59a9f (orchestrator) | 2026-10-16 | SPEC.md, repo layout, M1 localnet coordination |
