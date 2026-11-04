# token-evm: Cadre on Robinhood Chain (phase 1 contracts)

Foundry project for the second CADRE token side on Robinhood Chain (Arbitrum Orbit L2;
testnet chain id 46630, RPC https://rpc.testnet.chain.robinhood.com). Plan and decisions:
docs/RH-TESTNET-PLAN.md (E1-E11 at their recommended defaults). Byte layouts the chain and
the relayer must match: [FORMATS.md](FORMATS.md).

| Contract | Port of | Role |
|---|---|---|
| `CadreToken` | the pump.fun SPL mint | fixed supply ERC-20, 6 decimals, no owner, no mint, holders can burn (E1 a) |
| `CadreBridge` | token/programs/bridge | lock/unlock escrow, domain 2: `deposit` / `depositFor`, EIP-712 M-of-N `release`, `releaseProof` (v2 entry + SHA-256 MMR against an `ICadreHeaders` final header), `refundDeposit` (order deposits only), release modes, signer set, pause, processed bitmap |
| `CadreHeaderOracle` | token/programs/lightclient (signed mode) | attester-signed FINAL headers bound to the Cadre chain id, implements `ICadreHeaders` (phase 2: a Stylus light client with the same interface) |
| `Sha256Mmr` (library) | crates/pearl-verify-sbf/src/mmr.rs | Commonware MMR inclusion proofs, SHA-256 precompile |
| `CadreStorefront` | token/programs/storefront | one-tx `buyJob` (ETH in, v4 buy, W/P split, 10% buyback share held, bridge order deposit), EIP-712 finalize/expire, refunds, `settleShare`, `crankBuyback` to the reserve up to a cap, burn above it |
| `CadreReserve` | token/programs/reserve | permissionless rate-limited `fundMining` (Solana defaults: 10M per call, 1 h interval, 20M per 24 h), frozen destination, `crankBuyback` from LP fees |
| `CadreV4Desk` | PumpSwap CPIs | thin `unlockCallback` desk on the canonical v4 PoolManager 0x8366a39cc670b4001a1121b8f6a443a643e40951: init, full-range seed (locked, no remove), buy/sell, fee collect to the reserve, cp quote |

Phase 1 limits: storefront finalize is signer-only (finalize by proof needs blake3 tx_root
paths and an ed25519 check, phase 2 Stylus); the header oracle is a trusted M-of-N.

## Build and test

    cd token-evm
    git submodule update --init --recursive    # forge-std v1.16.2, solady v0.1.26, v4-core v4.0.0
    forge build
    forge test

solc 0.8.26, evm cancun (v4-core's PoolManager pins 0.8.26; Cancun bytecode runs on RH). The
tests run on a local EVM with the real v4-core PoolManager; no network. Every test key is a
label-derived key (`makeAddrAndKey`), never an anvil default account (on RH those carry
EIP-7702 sweeper delegations).

Vectors in test/vectors/ are written by `vectors-gen/` (Rust, own workspace, toolchain 1.95):
commonware-storage 2026.9.0 MMR proofs checked by crates/pearl-verify-sbf, the two real
devnet withdrawals from token/tests/lightclient-litesvm/fixtures/devnet.json, and an
independent encoding of the FORMATS.md messages.

    cd vectors-gen && cargo run --release && rm -rf target

## Deploy order (plan 4.6; NOT run here)

1. Fund a dedicated deployer (E10: new key under keys/rh-testnet/, gitignored) from the
   faucet; create the secp256k1 keys for header attesters, bridge signers and storefront
   attesters (files, gitignored). Never reuse kappas/tender keys.
2. `CadreToken` (whole supply to the deployer).
3. `CadreHeaderOracle(cadreChainId, attesters, threshold)`.
4. `CadreBridge(token, signers, threshold, minWithdrawalNonce)`, then
   `setReleaseModes(3, oracle)` (signers + proof).
5. `CadreReserve` (the desk's fee recipient is immutable, so the reserve comes first), then
   `CadreV4Desk(PoolManager, token, 3000, 60, reserve)`, `reserve.setBuyLimits(desk)`,
   `initializePool`, `addLiquidity` (full range, locked).
6. `CadreStorefront(token, bridge, desk, config, buyback, reserveCap, attesters, threshold)`,
   then `bridge.setOrderSource(storefront)` (frozen after the first order).
7. Supply split per E1: reserve share to the reserve, LP share in the pool, the rest held by
   the deployer/owner. Do NOT transfer tokens straight into the bridge: only deposits back
   chain coins.
8. Chain side: AdminConfig for domain 2 (RH bridge address, chain id 46630, attesters,
   the domain-2 outstanding cap).
9. Verify on Blockscout (`--verifier blockscout --verifier-url
   https://explorer.testnet.chain.robinhood.com/api/`), commit the deployment JSON, start the
   relayer (rh side), header oracle poster (every 10 final headers, E9) and crank.
10. e2e twice in a row.

Steps 2-7 are `script/Deploy.s.sol` (test/Deploy.t.sol runs it locally against a local
PoolManager):

    # dry run, needs only the deployer address
    CADRE_CHAIN_ID=1128350803 MINING_POOL=0x... ATTESTERS=0x..,0x..,0x.. SIGNERS=... STORE_ATTESTERS=... \
      forge script script/Deploy.s.sol --rpc-url rh_testnet --sender $DEPLOYER
    # broadcast: the owner's step, key read from its file
    ... forge script script/Deploy.s.sol --rpc-url rh_testnet --broadcast \
      --private-key "$(cat keys/rh-testnet/deployer.key)" --sender $DEPLOYER

It writes deployments/rh-testnet.json (or `DEPLOY_OUT`). Admin is an EOA on testnet (E11);
move every contract to a Safe before mainnet (solady `Ownable` two-step handover:
`requestOwnershipHandover` / `completeOwnershipHandover`).

## Operational notes

- A bridge redeploy must pass `minWithdrawalNonce` = the chain's next domain-2 withdrawal
  nonce, or the old bridge's withdrawals could be paid again by the new one (the entry does
  not name the bridge).
- Deposit finality for the relayer: `safe` tag on testnet, `finalized` on mainnet (E8).
- RH's public RPC keeps about 10 minutes of state and caps eth_getLogs at 10k logs; the
  relayer should read `deposits(nonce)` state and scan logs in small ranges.
