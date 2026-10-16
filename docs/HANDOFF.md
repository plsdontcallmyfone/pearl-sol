# pearl-sol handoff

READ THIS FIRST in a new session. Update it after every step (decision, commit,
agent result). Keep it current, not historical: rewrite stale lines instead of
appending.

## Goal (owner, 2026-10-16, ongoing)

Build the Pearl-on-Solana design: spec first, then a working localnet M1. Owner
allows running as many agents as needed.

- Token chain: Solana, pump.fun launch, fixed 1B supply. A mining reserve is
  funded by the dev buy plus creator-fee buybacks.
- Agent chain: our own SVM cluster (Agave, own genesis), running in parallel.
  The bridged token is its value token (lock on Solana, mint on the agent chain).
- AI economy: agents post paid AI jobs, pay each other and run Anchor programs.
  Miners do Pearl cuPOW matmul on posted jobs; only proofs bound to a paid job
  earn from the reserve. Each job's mining reward stays below the fee it burns
  (stops fake-job self-dealing).

## Decisions

- No Cosmos (owner). SVM chosen over EVM or a custom chain: same keys and Anchor
  tooling as the token side; a native verify builtin is possible later.
- Ports 10600-10699 (see CLAUDE.md). Repo is local git only, no remote.

## State

- [x] Repo, CLAUDE.md, research copied into research/ (commit f13cae4)
- [x] scripts/localnet.sh up|down|status: two validators, token :10600 and
      agent :10630, both verified (commit 9005000)
- [ ] Research agents (started 2026-10-16), writing into research/:
      NOTES-proof.md (Pearl proof internals, SVM verify cost, binding a proof to a job),
      NOTES-pump.md (pump.fun on localnet, creator fees, buyback),
      NOTES-chain-bridge.md (own SVM cluster, bridge choice, agent-economy primitives).
      If a file is missing, that agent did not finish: rerun its topic.
- [ ] docs/SPEC.md: programs, bridge flow, emission formula, milestones
- [ ] M1 localnet build

## Next step

Read the three NOTES files, write docs/SPEC.md, then build M1.

## Gotchas

- ~/memechain and ~/ansemchain are deleted locally. The private backup
  cocainebit/ansemchain needs `gh auth login` (token invalid 2026-10-16). Not
  needed for SVM.
- No GPU on this Mac: a localnet miner must use a CPU path at low difficulty.
- research/pearlrepo is gitignored (51 MB clone of pearl-research-labs/pearl @ 837c69c).
