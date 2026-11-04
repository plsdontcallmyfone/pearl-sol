# Cadre on Robinhood Chain testnet: research and build plan (2026-11-04)

Read-only research. No files in any repo were edited, no transactions sent, no key contents printed. LIVE = checked this session with curl/eth_call (user-agent curl/8.4.0); DOCS = docs.robinhood.com.

## 0. Short answer

A second CADRE side on Robinhood Chain (RH) testnet is practical. The token, bridge escrow, storefront, reserve and a signed header oracle are ordinary Solidity, and RH lifts the 24 KB code limit to 96 KB. The one hard piece is the trust-minimised light client: Pearl PoW verification is blake3-heavy (7-10M CU per header on Solana) and would cost tens of millions of gas per header in Solidity. RH has Stylus (ArbWasm stylusVersion 3, LIVE), and crates/pearl-verify-sbf is already no_std Rust with its own blake3, so phase 2 ports that crate to a Stylus contract. Phase 1 ships with attester-signed final headers and signer-mode releases, which is the same trust model Solana's bridge still supports (`set_release_modes`).

The Cadre chain itself needs a small consensus change before any second side: deposits are replay-keyed by Solana nonce only, and the single withdrawal log has no destination, so the same withdrawal could be released on both sides. The devnet is being re-genesised now (docs/REBUILD-2026-11.md D2), so the cheapest moment to add a `domain` to BridgeUnlock/BridgeLock is in that genesis.

## 1. Robinhood Chain testnet facts

| Item | Value | Source |
|---|---|---|
| Chain id | 46630 (0xb626), "Robinhood Chain Testnet", gas token ETH | DOCS https://docs.robinhood.com/chain/connecting + LIVE |
| Public RPC | https://rpc.testnet.chain.robinhood.com (also `/rpc`), rate limited | DOCS + LIVE |
| LIVE answers (2026-11-04) | eth_chainId 0xb626; eth_blockNumber 0x8d77ed8 = 148,340,440; eth_gasPrice 0x989680 = 0.01 gwei | LIVE |
| Sequencer / feed | https://sequencer.testnet.chain.robinhood.com; wss://feed.testnet.chain.robinhood.com is the sequencer feed (compressed only since 2026-09-17), not a JSON-RPC websocket | DOCS https://docs.robinhood.com/chain/notices-and-upgrades |
| Provider RPCs | Alchemy https://robinhood-testnet.g.alchemy.com/v2/{KEY} (+ wss), Chainstack, QuickNode, dRPC listed | DOCS |
| Explorer | https://explorer.testnet.chain.robinhood.com (Blockscout; verify with `--verifier blockscout --verifier-url https://explorer.testnet.chain.robinhood.com/api/`); `/api/v2/stats` 200 | DOCS + LIVE |
| Parent chain | Ethereum Sepolia (L2, not L3); latest block l1BlockNumber 0xb80e9c, a Sepolia height | DOCS https://docs.robinhood.com/chain/protocol-contracts + LIVE |
| Faucets | official https://faucet.testnet.chain.robinhood.com (ETH + test TSLA/AMZN/PLTR/NFLX/AMD, address only; returned 429 to curl, use a browser); Chainstack (1 ETH/24 h, needs 0.08 mainnet ETH + API key); Alchemy (0.1 ETH/24 h, needs 0.001 mainnet ETH); QuickNode (12 h, 0.001 mainnet ETH); ethfaucet.com (0.1/24 h, BringID); Chainlink (LINK only) | https://www.datawallet.com/crypto/get-robinhood-chain-testnet-tokens |
| Bridge from Sepolia | Arbitrum portal https://portal.arbitrum.io/bridge?destinationChain=robinhood-chain-testnet&sourceChain=sepolia (third-party link, docs.orbitflare.com); deposits about 10 min, withdrawals about 7 days | https://docs.robinhood.com/chain/bridging |
| L1 (Sepolia) contracts | Rollup 0xdc5F8E399DBd8a9F5F87AeC4C23Beb12431b386D, SequencerInbox 0xA0D9dB3DC9791D54b5183C1C1866eFe1eCA7D414, Delayed Inbox 0xF2939afA86F6f933A3CE17fCAB007907B6b0B7a4, Bridge 0x96295BDad104eaD97cC08797b3dC68efF59CcF30, Outbox 0x8D180Caf588f3Da027BEf1F42a106Da93F90b166, L1 GatewayRouter 0xF6F11aAEE80875776C264d93B37B34cE437382D1 | DOCS |
| L2 contracts | WETH 0x7943e237c7F95DA44E0301572D358911207852Fa (LIVE: code, symbol WETH; NOT the mainnet WETH), GatewayRouter 0x77bF00A6A90c600f214b34BAFBB7918c0cF113A8, Multicall 0xa432504b6F04Cafe775b09D8AA92e8dbe41Ec7a8, Permit2 0x000000000022D473030F116dDEE9F6B43aC78BA3 | DOCS + LIVE |

**Gas (LIVE).** Minimum and base fee 0.01 gwei (mainnet about 0.02 gwei). ArbGasInfo L1 base fee estimate is 0 on testnet, so calldata is effectively free there today; mainnet charges an L1 data fee per calldata byte (DOCS https://docs.robinhood.com/chain/gas-and-fees). Deploy estimates via eth_estimateGas: 20 KB runtime 4.41M gas = about 0.000044 test ETH (mainnet 4.57M gas, about 0.000092 ETH); 48 KB 10.8M gas = 0.000108 ETH. A whole phase-1 deploy is well under 0.001 ETH. Block gas limit reads 2^50 (Arbitrum artificial); the effective per-tx limit is Arbitrum's 32M.

**Differences from vanilla Arbitrum / Ethereum.**
- Contract size limit raised to 96 KB runtime (98,304) and 192 KB initcode. LIVE on testnet and mainnet: creates succeed up to 98,304 bytes and fail at 131,072 ("limit 98304"). DOCS https://docs.robinhood.com/chain/differences-from-ethereum. (Side note: memory's "DeskHook over EIP-170" constraint no longer applies on RH.)
- ArbSys `arbOSVersion()` = 116 on both (Nitro reports 55 + version, so ArbOS 61; inference, no doc states it).
- Stylus: ArbWasm 0x71 `stylusVersion()` = 3, `inkPrice()` = 10000 on both chains (LIVE). Rust/WASM contracts are available.
- Precompiles: EIP-2537 BLS12-381 present (G1ADD returns 128 zero bytes for zero input; 0x0c-0x11 live); RIP-7212 P256VERIFY at 0x100 present (valid vector returns 1). No ed25519 precompile (none exists on Arbitrum).
- EVM level: TSTORE/MCOPY (Cancun), EIP-2935 history contract (Prague), CLZ works (Osaka); BLOBBASEFEE unsupported. Compile for prague/cancun; avoid blobbasefee.
- Quirks: block.number is an L1 estimate, prevrandao constant, FCFS ordering, L1-to-L2 sender aliasing. No deployer allowlist found; kappas deployed freely to testnet.

**DEX / launchpad on testnet (LIVE getCode).**

| Contract | Testnet | Mainnet |
|---|---|---|
| Uniswap v4 PoolManager 0x8366a39cc670b4001a1121b8f6a443a643e40951 | 24,009 B (same code hash as mainnet) | 24,009 B |
| Uniswap v3 factory 0x1f7d7550... | no code | 24,535 B |
| Uniswap v2 factory 0x8bcEaA40... | no code | 13,859 B |
| Permit2, CREATE2 deployer 0x4e59..., Multicall3 0xcA11..., EntryPoint v0.7, Safe singleton factory | present | present |

No canonical Uniswap periphery (PositionManager, UniversalRouter, Quoter) is documented on testnet; the explorer has many user-deployed copies, none official. Testnet stock tokens (explorer-verified, probably the faucet's, not doc-confirmed): TSLA 0xC9f9c86933092BbbfFF3CCb4b105A4A94bf3Bd4E, AMZN 0x5884aD2f920c162CFBbACc88C9C51AA75eC09E02, PLTR 0x1FBE1a0e43594b3455993B5dE5Fd0A7A266298d0, NFLX 0x3b8262A63d25f0477c4DDE23F83cfe22Cb768C93, AMD 0x71178BAc73cBeb415514eB542a8995b82669778d; mock USDG copies 0x8F9231B0F448bA9AD045348437458721676c23BC and 0x433947311e248C9fEc39c69Fac2a305661CCb907. No launchpad (pons is mainnet-only per ~/kappas/review/0dte/DEPLOYMENTS-TESTNET.md).

**What replaces PumpSwap:** a CADRE/WETH pool on the canonical v4 PoolManager, driven by our own small contract that calls `unlock` and does `modifyLiquidity` (seed, full range) and `swap` in `unlockCallback` (no PositionManager needed). ~/kappas/contracts/src/interfaces/IUniswapV4.sol and PonsCashless.sol already implement the unlock/sync/settle/take pattern on RH. Using the same PoolManager address on mainnet means the code moves unchanged. Mainnet also has v2 and v3 if a v3 pool is preferred there.
## 2. Solana-side components and their EVM equivalents

Repo: ~/pearl-sol-rebuild (branch devnet-rebuild). Program sources under token/programs, verifier crate crates/pearl-verify-sbf, chain consensus under chain/.

### 2.1 What is already chain-agnostic, and what each primitive costs on EVM

| Primitive | Where it is used | Solana today | EVM (RH Chain, Arbitrum Orbit) |
|---|---|---|---|
| Header: fixed 256 bytes, `block_hash = blake3(header)` | light client, every proof reader | software blake3 in SBF (crates/pearl-verify-sbf/src/blake3.rs) | no blake3 precompile. Solidity/Yul blake3 is roughly 15-30k gas per 64-byte compression, so one header hash (4 compressions) is about 100k gas. In Stylus (Rust to WASM) it is 10-100x cheaper and the same Rust code compiles |
| PoW proof: about 24.7 KB (k 2048, r 128, 4x8 tile), `proof_hash = blake3(proof)`, openings, noise, jackpot, WTEMA | lightclient verify steps | 7.25M CU (job block, 34 txs) to 10.24M CU (fallback, 37 txs) | THE hard part. In Solidity the blake3 of 24.7 KB alone is about 390 compressions (about 6-12M gas) and the int7 openings/noise add more: an estimate of 30-100M gas per header, over Arbitrum's 32M per-tx cap, so it would need Solana-style step splitting. Plus about 25 KB calldata per header posted to L1. Port to Stylus instead: pearl-verify-sbf is already `no_std` + alloc with no external hash crates, which is exactly what Stylus wants |
| Difficulty: WTEMA in U256/U512 (`difficulty.rs`) | submit_header | primitive_types U256 | Solidity uint256 native; U512 step needs a mulmod/2-word helper, or Stylus |
| Withdrawal log proof: Commonware QMDB keyless MMR over SHA-256 (`mmr.rs`) | bridge.release_proof | sol sha256 syscall | SHA-256 precompile 0x02 (60 + 12 gas/word): cheap, about 20-60k gas per proof. Port mmr.rs verify to Solidity directly |
| Final-header accumulator: SHA-256 MMR (`acc.rs`) | accumulate, prove_header, restore_header | sha256 syscall | same as above, cheap; or skip on EVM (storage is not rent-charged, headers need not be closed) |
| tx_root paths: blake3 binary merkle (`chaintx.rs verify_tx_path`) | finalize_order_proof, expire_order_proof, register_job | blake3 software | depth about 10-12, one compression per level: about 200-400k gas in Yul, much less in Stylus |
| Chain tx signatures and evaluator signatures: ed25519 | order finalize by proof re-verifies one (evaluator, sig) pair; chain tx parsing | ed25519 precompile instruction | NO ed25519 precompile on Arbitrum (RIP-7212 is P-256 only). Solidity ed25519 is about 1-1.3M gas per verify; Stylus (ed25519-dalek or curve25519 no_std) far less. Avoid on EVM where possible |
| Attester M-of-N over deposit/release/refund messages | bridge.release (signer mode), refund_deposit, storefront finalize_order (signer mode) | ed25519 precompile | use secp256k1 attester keys with ecrecover (3k gas each) and EIP-712 messages on the EVM side. The Cadre chain side keeps ed25519 for BridgeUnlock (the attesters sign off chain; nothing on EVM verifies ed25519) |
| Amount units: chain 9 decimals, SPL 6 decimals, `SPL_SCALE = 1000` | bridge, relayer invariant | SPL u64 | ERC-20 with `decimals() = 6` keeps every message and invariant identical (`amount_spl` semantics). 18 decimals would need a 10^12 scale in the bridge only |
| Domains and messages: `pearl-sol:deposit:v1`, `pearl-sol:deposit-order:v1`, order job id `sha256("pearl-sol:order-job:v1" || nonce LE)` | chain/agent-types/src/tx.rs | | must get NEW EVM domains (see 2.3), the formats themselves port as-is |

Measured precompile/feature facts (this session, eth_call against the live RPCs):

- RH testnet `https://rpc.testnet.chain.robinhood.com` answers chain id 0xb626 = 46630.
- ArbWasm (0x71) `stylusVersion()` = 3 on BOTH mainnet and testnet, so Stylus contracts are available. ArbSys (0x64) `arbOSVersion()` returns 116 on both (ArbOS reports 55 + version, so ArbOS 61 by that convention).
- EIP-2537 BLS12-381 G1ADD (0x0b) answers on testnet (Prague-level precompiles present).

### 2.2 Program by program

**bridge** (token/programs/bridge, 711 lines). Instructions: `initialize(signers, threshold)`, `set_release_modes(modes, light_client)`, `set_signers`, `set_admin`, `deposit(amount, agent_recipient)` (SPL into the vault, Deposit PDA per nonce), `release(nonce, recipient, amount)` (M-of-N ed25519 via precompile introspection, Processed PDA per nonce), `refund_deposit(nonce, buyer)` (attester-signed refund, refund record PDA), `release_proof(nonce, entry, location, proof)` (88-byte withdrawal entry `nonce || from[32] || solana_recipient[32] || amount_spl || height`, MMR inclusion vs `withdraw_root` of a FINAL light-client header).
EVM: `CadreBridge.sol` holding the ERC-20 in escrow (lock/unlock, nothing minted, as on Solana). `deposit(amount, bytes32 cadreRecipient)` emits `Deposit(nonce, sender, cadreRecipient, amount)` and stores `deposits[nonce]` (amount, sender, refunded flag) so the relayer can read state, not only logs. `release(nonce, recipient, amount, sigs)` with EIP-712 + ecrecover M-of-N and a `processed` bitmap; `releaseProof(entry, location, proof)` that calls `CadreLightClient.isFinal(headerHash)` and verifies the SHA-256 MMR in Solidity; `refundDeposit(nonce, sigs)`; `setReleaseModes`, `setSigners`, admin via Ownable2Step (later a Safe). Events replace PDAs as the relayer's change feed.

**lightclient** (token/programs/lightclient, 2,300 lines + crates/pearl-verify-sbf 2,403 lines). Instructions: initialize, set_params, set_speed, submit_header, init_work, begin_verify, write_proof, verify_proof_hash, fallback_gen_b / fallback_hash_bt / fallback_hash_a, verify_opening, verify_noise, verify_jackpot(_with_parent), connect, close_work, reorg, mark_final(_linked), register_job, replace_job, begin_verify_job(_via), close_header, init_accumulator, accumulate, prove_header, restore_header. Accounts: Config, Canon ring (256), HeaderRecord per hash, JobCommit per job, Pool, Accumulator, relayer work accounts (69,632 bytes).
EVM: two options, see 4.3 decision E3.
- Signed (phase 1): `CadreHeaderOracle.sol`: M-of-N secp256k1 attesters sign `(height, block_hash, withdraw_root, tx_root, is_final)` for finalized headers; the contract stores `headers[hash] = {height, tx_root, withdraw_root, final}`. About 60-100k gas per header incl. 3 ecrecovers, and only FINAL headers need posting (the bridge and storefront only read final ones), so it can batch every N blocks (one root covers all earlier withdrawals because the withdrawal log is append-only). Same trust as Solana's signer mode today.
- Proven (phase 2): `CadreLightClient` as a Stylus contract compiled from pearl-verify-sbf (cfg feature `stylus` replacing the Anchor glue): `submitHeader(bytes256)`, chunked `writeProof`, staged `verify*` calls exactly like the Solana steps but likely 1-3 txs per header, `reorg`, `markFinal`. WorkDone/connect parallel-relay logic ports unchanged. No rent: the work buffer can be a mapping keyed by (relayer, header). Cost to measure on testnet before deciding (see 4.4 step 7).
- Interim option: relay only every Nth final header plus a SHA-256 MMR accumulator of hashes is NOT sound without verifying the work of every header in between; do not do it in proof mode.

**storefront** (token/programs/storefront, 2,582 lines). Instructions: initialize, set_finalize_modes, update_config, set_attesters, set_admin, set_buyback, set_buyback_cap, open_order(draft_id, recipient, spec), cancel_draft, buy_job (pump curve) / buy_job_amm (PumpSwap): SOL in, buy CADRE, split W + P, 10% buyback share held, bridge Deposit from the trader PDA with an Order account; finalize_order / expire_order (attester-signed), mark_refunded, finalize_order_proof / expire_order_proof (tx_root inclusion + one ed25519 evaluator re-verify), settle_share, crank_buyback / crank_buyback_amm. Events as Anchor logs.
EVM: `CadreStorefront.sol`: `openOrder(spec)` stores the JobSpec hash (the full PostJob body goes in calldata and an `OrderOpened` event so the relayer rebuilds and checks `body_hash`), `buyJob(nonce, minOut)` payable in ETH (or USDG via Permit2) swaps on the Uniswap pool (v3 SwapRouter02 or v4 PoolManager unlock callback), splits W + P, holds the buyback share, calls `CadreBridge.depositFor(order)`. `finalizeOrder` / `expireOrder` with EIP-712 attester sigs (secp256k1), `markRefunded`, `settleShare`, `crankBuyback(maxIn, minOut)`. Proof mode `finalizeOrderProof(tx, path, headerHash)` needs blake3 tx_root path + the ed25519 evaluator check: do it in Stylus, or keep finalize signer-only on EVM until phase 2 (recommended).

**reserve** (token/programs/reserve, 1,374 lines). Instructions: initialize, update_config, set_fund_limits, migrate_config, set_admin, launch (pump.fun create + dev buy through a creator PDA), collect_curve_fees, crank_buyback (curve), crank_buyback_amm (PumpSwap), fund_mining (permissionless, rate-limited, moves reserve tokens into the bridge with `mining_pool` as recipient; destinations freeze after first funding).
EVM: `CadreReserve.sol` holding the ERC-20 mining reserve: `fundMining(amount)` with the same limits (`maxPerCall`, `minInterval`, window cap, pause, frozen destination) calling `CadreBridge.depositFor(miningPool)`; `crankBuyback(maxIn, minOut)` spending accrued ETH fees through the pool. There is no pump.fun creator fee on RH, so the revenue source is the LP fee of the CADRE pool (v3 position fees via `collect`, or a v4 hook fee) plus the storefront buyback share. `launch` becomes a deploy script (mint fixed supply, seed the pool), not an instruction.

**town** (token/programs/town, 961 lines) and **mandate** (937 lines). Town = profiles, posts, reactions/tips in SOL, follows, consent, paid DMs; mandate = user trading accounts swapping on PumpSwap. Neither depends on the Cadre chain. Recommend NOT porting either for the RH side at first (decision E7); the site keeps one town on Solana. If wanted later: `CadreTown.sol` events-only (posts as events, tips in ETH) and a mandate vault against the Uniswap pool.

**pump.fun / PumpSwap** (fixtures/pump, reserve + storefront CPIs, cadre-api `/v1/tx/pump-buy|pump-sell|pump-create`). EVM: Uniswap v3 (or v4) CADRE/WETH pool on RH testnet if Uniswap is deployed there (part 1), else our own deploy of v2/v3 core from the Foundry libs found in part 3, or ~/rothhood's v4 hook launchpad.

### 2.3 Cadre chain changes (consensus, needed for a second side)

This is the part that is easy to miss. The chain today assumes exactly one external side:
- `BridgeUnlock` replay set is keyed by `solana_nonce` only (`schema::deposit_key(nonce)`); an RH deposit nonce 5 and a Solana deposit nonce 5 would collide.
- `BridgeLock { solana_recipient: [32], amount }` appends ONE global withdrawal log; the Solana bridge's `release_proof` accepts any final entry. If the RH bridge also accepted entries from the same log, one withdrawal could be released on both chains. Every entry must carry a destination domain, and each side must refuse entries for the other domain.
- `SetRoute` liens pay `solana_recipient` only.
- One escrow and one relayer invariant: `vault_spl x 1000 == total - escrow + pending`.

Change set (new tx kinds or a versioned body, behind a height, or simply in genesis since the devnet is being re-genesised now per REBUILD-2026-11.md D2):
1. `BridgeUnlockV2 { domain u8, ext_nonce u64, recipient [32], amount_ext u64, job_order, sigs }` with replay key `(domain, nonce)` and message `b"cadre:deposit:v2" || domain || chain_id_ext u64 || bridge_addr [20|32] || nonce || recipient || amount` (domain 1 = Solana, 2 = RH Chain 46630). Keep kind 6 as domain 1.
2. `BridgeLockV2 { domain u8, recipient [32], amount }` (EVM address left-padded to 32 bytes). Withdrawal entry v2 = `version 0x02 || domain || nonce || from || recipient || amount_ext || height` with per-domain nonces (so each side's `processed` bitmap is dense). The Solana `release_proof` already requires `entry.len() == 88`, so a v2 entry (different length) is refused by the deployed program as is; the Solana side keeps using v1 entries (kind 7) and only RH uses v2. A separate `withdraw_root_evm` would need a header change, so one log with a domain byte is the simpler route. The RH bridge must require `version == 0x02 && domain == 2`.
3. Liens: `SetRouteV2 { domain, recipient, ... }`.
4. Escrow accounting per side: either one escrow with per-domain counters (`outstanding[domain]` = units released minus units locked back) so a side can never unlock more than was deposited there, or two escrow addresses. Relayer invariant per side: `vault_side x scale == outstanding_side + pending_side`.
5. getParams / RPC additions: getDeposit(domain, nonce), getWithdrawalProof(domain, nonce), counters per domain. crates/pearl-verify-sbf `chaintx` parsers for the v2 kinds.

### 2.4 Off-chain services and client code paths

| Component | Files | Solana coupling | EVM change |
|---|---|---|---|
| relayer (TS, 871-line core) | services/relayer/src/{relayer,chain,codec,agent-tx,config,state}.ts | `ChainIO` interface already exists (getAccounts + send), but codecs read Anchor PDAs (Deposit, Order, Processed, Config) and build Anchor ixs | add `EvmSide` (viem): read `deposits(nonce)`, `processed(nonce)`, `OrderOpened` logs, send `release`/`refundDeposit`/`finalizeOrder`; make `tick()` iterate over `sides[]`, each with its own state cursor and invariant. One relayer process per side is simpler (separate state files and keys) and keeps the Solana relayer untouched |
| header-relay (Rust) | services/header-relay/src/{relay,sol,settle,chain,ledger,main}.rs (2,894 lines) | Solana RpcClient, CU budgets, work accounts, Anchor ix builders from lightclient::client | phase 1: new small service `evm-header-oracle` (or a mode of the attester) signing final headers and posting batched roots; phase 2: an `evm` backend of relay.rs behind a `Target` trait (submit, write chunk, verify step, status read, reorg, mark_final) using alloy. `settle.rs` (release_proof / finalize_order_proof submission) gets an EVM twin |
| attester / settle | services/attester, header-relay settle | ed25519 attester key signs order finalize messages | add a secp256k1 attester key and EIP-712 signing for EVM finalize/refund/header-oracle messages |
| storefront-x402 (TS) | services/storefront-x402/src/{x402,storefront,service,chain,reconcile}.ts | x402 "exact" on Solana (signed SOL/USDC transfer tx) | x402 has an EVM "exact" scheme (EIP-3009 transferWithAuthorization on USDC-like tokens); on RH the natural asset is ETH or USDG (check EIP-3009 support on USDG); otherwise the service builds a `buyJob` call for the payer |
| crank (TS) | services/crank/src/{main,storefront,trade,accounts}.ts | pump curve / PumpSwap buybacks, fund_mining | EVM crank: `CadreReserve.fundMining`, `crankBuyback`, `storefront.crankBuyback`, v3 `collect` of LP fees |
| cadre-api (TS) | services/cadre-api/src/sol.ts (713 lines) + routes wallet/inference/launches/trading/boards/... | unsigned Solana tx builders (`/v1/tx/bridge-deposit`, `pump-buy`, `pump-sell`, `crank-buyback`, `fund-mining`), bridge stats | add `?side=rh` (or `/v1/rh/...`) builders returning EIP-1193 call objects `{to, data, value}`; `/v1/bridge` and `/v1/summary` report both vaults; supply view per side |
| explorer indexer | apps/explorer/src/{soltx,decode,indexer}.ts | decodes Solana txs and PDAs | index EVM bridge/storefront events via eth_getLogs (mind the 10k-log cap noted for RH mainnet); decode BridgeUnlockV2/BridgeLockV2 |
| CLI | crates/cadre-cli/src/solana.rs (815 lines), endpoints.rs | Solana reads (bridge vault, orders, light client lag), program ids | `evm.rs` reads via alloy, endpoints for RH testnet; `cadre bridge out --to 0x..` builds BridgeLockV2 |
| extension / Cadre Wallet | extension/src/solana/{tx,programs,rpc}.ts; ~/pearl-wallet (Leather-based) | Solana Wallet Standard + window.cadre for the chain | EVM signing needs secp256k1 keys and an EIP-1193 provider. Cheapest: the site uses any injected EVM wallet (MetaMask, Rabby) for the RH side via wagmi/viem; Cadre Wallet keeps chain + Solana (decision E6) |
| cadre-web (~/cadre-web) | lib/*, buy + bridge pages | Solana Wallet Standard | add a side switch (Solana / Robinhood Chain), wagmi config for 46630, bridge-in/out and buy-job forms calling the API builders |
| scripts | scripts/devnet-solana.sh, scripts/devnet-init/init.ts | Solana deploy + init order | `scripts/devnet-rh.sh` (forge script deploy, init, verify on Blockscout) |

## 3. Reusable EVM code on this Mac, and deployer keys

Most RH project dirs named in memory are gone: ~/float, ~/rothhood, ~/ponsinomics, ~/tender, ~/bearer, ~/floatdesks-launchpad, ~/musetrade, ~/curio, ~/rotating-reserve, ~/agent-fund, ~/altsol-hyperevm, ~/crosslaunch do not exist. ~/pearl-sol*/contracts are Cadre-chain WASM contracts, not EVM. No hardhat anywhere.

| Path | What | Reuse |
|---|---|---|
| /Users/achi/kappas/contracts | LIVE on 4663 (sdk/deployments/4663.json). foundry.toml: solc 0.8.26, cancun, optimizer 400, no via_ir, rpc_endpoints robinhood, fs_permissions to write deployments JSON; lib forge-std + solady | Foundry profile; script/pons/PonsDeploy.s.sol + /Users/achi/kappas/ops/mainnet-deploy.sh (dry run with `--sender`, broadcast with key from file, balance check first) as the deploy template; v4 unlock/swap interfaces (src/interfaces/IUniswapV4.sol, PonsCashless.sol, PonsLaunchVault.sol, PonsVaultOps.sol) |
| /Users/achi/kappas/research/0dte/forks/ponsfamily-ref/PonsV2LaunchFactory.sol (lines ~1340-1460) | v4 pool initialize + full-range seeding via PositionManager/Permit2 | reference only (its OZ / v4-core libs are not on disk) |
| /Users/achi/kappas/sdk/src/config.ts, /Users/achi/kappas/keeper/src/config.ts | viem `defineChain` 4663, keeper skeleton (KEEPER_RPC_URL, KEEPER_KEY_FILE, dry run, intervals) | copy the 46630 chain def + keeper loop for the EVM crank/oracle |
| /Users/achi/kappas/app/src/chain/types.ts + chain/{evm,sol}/adapter.ts | chain-neutral seam (Identity, string ids, bigint amounts), one adapter per chain | pattern for the relayer/cadre-api side abstraction |
| /Users/achi/ballast/src/MemeToken.sol | minimal fixed-supply ERC-20 (52 lines, no owner/mint) | base for CADRE ERC-20 (add decimals 6) |
| /Users/achi/doyouinfer/contracts/TestUSDC.sol | EIP-712 + EIP-3009 transferWithAuthorization with nonce map | pattern for EIP-712 attester messages and for an x402 test token |
| /Users/achi/kappas/scripts/dev.sh, test.sh | anvil fork stack on port 11000 | template for the anvil e2e (use our own port block) |

Chain facts from kappas notes: the official RPC keeps only about 10 minutes of state (use an archive RPC for history; https://robinhood.drpc.org free tier refuses eth_getLogs); never use anvil's default accounts on a 4663 fork (they carry EIP-7702 sweeper delegations on the real chain).

Must be fetched or written fresh: OpenZeppelin (or keep solady), v4-core interfaces as a lib, the bridge escrow with nonces, EIP-712 M-of-N verification, SHA-256 MMR verifier, blake3 (Stylus), Stylus toolchain (`cargo stylus`).

**Deployer keys (paths only) and balances (LIVE eth_getBalance, 2026-11-04):**

| Key file | Address | Mainnet 4663 | Testnet 46630 |
|---|---|---|---|
| /Users/achi/Desktop/kappas-keys/deployer-private-key.txt | 0x80514E0a71C9cb428836219974Dc6348Fae90Cc2 | 0.010197 ETH | 0 |
| /Users/achi/Desktop/kappas-keys/owner-private-key.txt | 0x1A309472C8d068b53a47C149D8dFAF782e640828 | 0.004 ETH | 0 |
| /Users/achi/.tender/testnet-deployer (address in testnet-deployer.addr) | 0x0b63Dee2702b778f9E3D1E5034B8250F801fA5A9 | 0 | 0.0014127 ETH |
| kappas keeper (address only; key was on the deleted box 64.95.13.106) | 0xB9a20a1a21c722997C0f02268A1bdF79de1C9D23 | 0.012922 ETH | 0 |
| /Users/achi/.kappas/dev/*.key, /Users/achi/.tender/pons-dev/*.key | fork/dev keys | 0 | 0 |

No `.env` with a PRIVATE_KEY and no Foundry keystores exist. 0.0014 test ETH covers the whole phase-1 deploy (about 0.0005 ETH) but not much traffic; get more from the official faucet. Recommendation: create a dedicated Cadre RH deployer (e.g. keys/rh-testnet/deployer.key, gitignored, like keys/devnet) rather than reuse the tender/kappas keys, so the kappas mainnet owner/deployer stays single-purpose.

## 4. Build plan

### 4.1 Contracts (new Foundry project `token-evm/` in ~/pearl-sol, solc 0.8.26+, evm prague, solady, forge-std, v4-core interfaces)

| Contract | Role | Notes |
|---|---|---|
| `CadreToken` (ERC-20) | fixed supply, decimals 6, no owner, no mint | supply per decision E1 |
| `CadreBridge` | lock/unlock escrow: `deposit(amount, bytes32 cadreRecipient)`, `depositFor` (storefront/reserve), `release` (EIP-712 M-of-N ecrecover), `releaseProof(entry, location, proof, headerHash)`, `refundDeposit`, `setReleaseModes`, `setSigners`, pause | events `Deposit`, `Released`, `Refunded`; `processed` bitmap per withdrawal nonce; entry v2 with domain check |
| `CadreHeaderOracle` (phase 1) | attester-signed FINAL headers: `postFinal(header256, sigs)` stores `{height, tx_root, withdraw_root, state_root}`; attesters sign the block hash and its roots, so signed mode needs no blake3 on chain | interface `ICadreHeaders.isFinal(hash) / roots(hash)` shared with phase 2 |
| `Sha256Mmr` library | Commonware keyless MMR inclusion (port of crates/pearl-verify-sbf/src/mmr.rs) | precompile 0x02; vectors from crates/pearl-verify-sbf tests |
| `CadreStorefront` | openOrder (PostJob body in calldata + event, body hash stored), buyJob (ETH in, v4 swap, W/P split, 10% buyback share held), finalizeOrder / expireOrder (EIP-712 attester), markRefunded, settleShare, crankBuyback (to reserve up to cap, rest burned to 0xdead or held: a fixed-supply ERC-20 with no burn needs `transfer(0xdead)`) | finalize-by-proof waits for phase 2 |
| `CadreReserve` | mining reserve: fundMining with the Solana limits (per call, interval, window, pause, frozen destination), crankBuyback from collected LP fees | no pump.fun creator fee on RH |
| `CadreV4Desk` | thin v4 `unlockCallback` contract: `seed(fullRange)`, `swapExactIn` for storefront/reserve/users, `collect` | canonical PoolManager 0x8366...0951 on testnet and mainnet |
| `CadreLightClient` (phase 2, Stylus, Rust) | pearl-verify-sbf compiled for Stylus: submitHeader, chunked proof write, staged verify steps, connect, reorg, markFinal; implements `ICadreHeaders` | bridge/storefront switch by `setReleaseModes`, no redeploy |

### 4.2 Cadre chain changes (chain/, crates/pearl-verify-sbf, crates/pearl-node)

`BridgeUnlockV2 {domain, ext_nonce, recipient, amount_ext, job_order, sigs}` with replay key (domain, nonce) and domain-tagged messages; `BridgeLockV2 {domain, recipient[32], amount}` appending v2 entries (version byte, domain, per-domain nonce); `SetRouteV2` with domain; per-domain outstanding counters so a side can never unlock more than it locked; RPC getDeposit/getWithdrawalProof/counters by domain; codecs in crates/pearl-node, services/relayer/src/codec.ts, extension + pearl-wallet decoders. Put it in the new genesis (chain_id 1128350803) if the rebuild has not started its chain yet; otherwise behind an activation height.

### 4.3 Services (chain abstraction)

- **Side abstraction:** one `Side` interface in TS (`readDeposits(from)`, `isProcessed(nonce)`, `readOrders`, `vaultBalance`, `release`, `refund`, `finalizeOrder`) with `SolanaSide` (existing code moved behind it) and `EvmSide` (viem). Run one relayer process per side (own state file, own keys, own invariant); the Solana relayer's behaviour does not change.
- **relayer:** EvmSide reads `deposits(nonce)` and `OrderOpened` logs at the `finalized` block tag (Arbitrum RPCs support safe/finalized; testnet can use `safe` for speed), signs ed25519 BridgeUnlockV2 for the chain, and secp256k1 EIP-712 releases/refunds for RH. Invariant: `vault_rh x 1000 == outstanding_rh + pending_rh`. eth_getLogs in ranges under the 10k cap.
- **header oracle (phase 1):** new mode of services/attester (or a small `evm-headers` binary under services/header-relay) that posts every Nth final header (one post covers all earlier withdrawals because the log is append-only); configurable cadence, e.g. every 10 final headers.
- **settle:** EVM twin of services/header-relay/src/settle.rs: releaseProof for final withdrawals once the oracle has a covering header (phase 1 can also use signer release).
- **header-relay (phase 2):** `Target` trait over relay.rs with `SolanaTarget` and `StylusTarget` (alloy).
- **storefront-x402:** add the EVM "exact" scheme (EIP-3009) for a USDC-like token if RH testnet has one that supports it, else return an unsigned `buyJob` call for the payer.
- **crank:** EVM jobs: fundMining, storefront/reserve crankBuyback, v4 fee collect (copy the kappas keeper loop).
- **cadre-api:** `side=rh` variants of `/v1/tx/bridge-deposit`, `/v1/tx/bridge-lock`, pump-buy/sell (v4 desk), crank, fund-mining, returning EIP-1193 `{to, data, value}`; `/v1/bridge` and `/v1/summary` show both sides; supply view per side.
- **explorer:** EVM event indexer (bridge, storefront, reserve), decoders for the v2 kinds.

### 4.4 CLI, wallet, site

- crates/cadre-cli: `evm.rs` (alloy reads), endpoints for 46630, `cadre bridge out --to 0x...` building BridgeLockV2, `cadre status` shows the RH side.
- ~/cadre-web: side switch (Solana / Robinhood Chain), wagmi config for 46630, bridge in/out and buy forms using the API builders; injected EVM wallet (MetaMask, Rabby) for the RH side.
- Cadre Wallet (~/pearl-wallet): codecs for the v2 kinds only in phase 1; EVM signing in the wallet is decision E6.

### 4.5 Tests

- Foundry unit + fuzz: token, bridge (replay, wrong domain, signer threshold, mode switch, invariant under random deposit/release sequences), MMR verifier against vectors exported from crates/pearl-verify-sbf tests and a real devnet withdrawal, storefront order lifecycle and buyback cap, reserve limits, v4 desk on a fork of testnet (`forge test --fork-url` with a non-default sender, per the kappas EIP-7702 note).
- Chain: executor tests for v2 kinds (replay per domain, domain refusal, per-domain outstanding, supply invariant randomized test extended), codec vectors shared with TS.
- Anvil e2e (`scripts/rh-e2e.sh`): anvil with the canonical PoolManager etched or a testnet fork, local agent-chain node (devnet.sh), relayer EvmSide, header oracle, crank; steps: deposit -> BridgeUnlockV2 -> BridgeLockV2 -> oracle post -> releaseProof; buyJob -> order job mined and attested -> finalizeOrder; refund path. Ports: a new block inside 10600-10699 is full, so reserve e.g. 10740-10749 if free or ask the orchestrator for a block (this repo's CLAUDE.md lists 10740-10749 as the trading row's test ports; check with lsof first).
- Testnet run: the same e2e against RH testnet + the rebuilt Cadre devnet, twice in a row.
- Phase 2: Stylus contract unit tests via `cargo stylus` + a gas measurement of one fallback and one job header before committing to it.

### 4.6 Deploy order on RH testnet

1. Funded dedicated deployer (faucet) and attester secp256k1 keys (files, gitignored).
2. CadreToken (mint all to the deployer).
3. CadreHeaderOracle (attesters, threshold).
4. CadreBridge (token, oracle, signers, modes = signers + proof).
5. CadreV4Desk; initialize CADRE/WETH pool on PoolManager, seed liquidity.
6. CadreReserve (token, bridge, mining_pool on the chain), CadreStorefront (token, bridge, desk, reserve, attesters, buyback cap).
7. Transfer supply split per E1: bridge vault share, reserve share, LP share.
8. Chain side: AdminConfig for domain 2 (RH bridge address, chain id 46630, attesters).
9. Verify all on Blockscout; write deploy/rh-testnet.json; start relayer (rh), header oracle, crank (rh); cadre-api + site config.
10. e2e twice.

### 4.7 Rough hours (one agent, focused; parallelisable across 3-4 agents)

| Work | Hours |
|---|---|
| Chain v2 bridge kinds + tests + codecs (Rust, TS, wallet decoders) | 14-18 |
| Contracts: token, bridge, MMR lib, header oracle | 12-16 |
| Contracts: storefront, reserve, v4 desk | 16-22 |
| Foundry tests + fork tests | 12-16 |
| Relayer Side abstraction + EvmSide | 12-16 |
| Header oracle + EVM settle | 8-10 |
| Crank (rh), storefront-x402 EVM path | 8-12 |
| cadre-api builders + bridge/supply views, explorer indexer | 12-16 |
| CLI | 4-6 |
| cadre-web side switch + forms | 10-14 |
| Anvil e2e + testnet deploy + two passing runs | 12-16 |
| **Phase 1 total** | **about 120-160 h** |
| Phase 2: Stylus light client port + header-relay StylusTarget + measurements | 40-70 |

### 4.8 Open decisions for the owner (recommended default in bold)

| # | Decision | Options | Default |
|---|---|---|---|
| E1 | Supply across two sides | (a) one 1B total split between sides: the chain escrow holds the whole 1B; Solana side keeps its mint, RH gets a NEW fixed ERC-20 and the chain's escrow is partitioned so outstanding per side is capped by what that side locked; (b) a second independent 1B on RH (total 2B, breaks the fixed-supply story); (c) RH token = a bridged representation of Solana CADRE via Wormhole NTT / LayerZero OFT (one supply, third-party trust) | **(a) on testnet now**: RH CADRE fixed supply equal to an agreed slice (e.g. the same 1B number on testnet only, since testnet tokens have no value), with per-domain outstanding caps on the chain. For mainnet decide before launch, since only (a) or (c) keep 1B total |
| E2 | When to add the chain v2 bridge kinds | in the rebuild genesis now, or behind a height later | **In the rebuild genesis now**, if the new chain has not started; it avoids a rolling upgrade |
| E3 | Light client on RH | signed header oracle first; Stylus port; Solidity port; ZK (SP1/RISC Zero) proof of header verification | **Signed oracle in phase 1, Stylus port in phase 2** after measuring gas on testnet. Solidity is too costly; ZK is a larger project |
| E4 | Pool venue | canonical v4 PoolManager with our own desk contract; deploy our own v2/v3 on testnet; ~/rothhood-style hook launchpad (source gone) | **Canonical v4 PoolManager**, same address on mainnet |
| E5 | Pay asset for buyJob / x402 | ETH; mock USDG; both | **ETH** on testnet (USDG on testnet is unofficial copies) |
| E6 | Wallet for the RH side | injected EVM wallets via the site; add EVM to Cadre Wallet | **Injected wallets first**; Cadre Wallet EVM later (memory: keep Leather UI, minimal changes) |
| E7 | Town and mandate on RH | port; keep them on Solana only | **Solana only** (neither depends on the Cadre chain) |
| E8 | Deposit finality on RH | `finalized` tag (L1-final, slower), `safe`, latest | **`safe` on testnet, `finalized` on mainnet** |
| E9 | Header oracle cadence | every final header; every N | **Every 10 final headers** plus on demand when a withdrawal is waiting |
| E10 | Deployer | new dedicated key; reuse ~/.tender/testnet-deployer | **New dedicated key** under keys/rh-testnet (gitignored) |
| E11 | Admin | EOA ops key; Safe on RH (Safe singleton factory is present) | **EOA on testnet**, Safe before mainnet |
