# pump.fun integration notes (as of 2026-10-16)

Scope: launch the token on pump.fun (fixed 1B supply), fund a mining reserve from the dev
buy plus buybacks paid from creator fees, and run the whole flow locally.

Everything marked **[verified]** was checked on 2026-10-16 against mainnet state (Helius RPC,
slot ~457,798,000) and by executing the real mainnet program bytes in-process with LiteSVM
(no validator, no ports). The check script is `research/pump-litesvm/svm.js` (see section 7).

Pump ships breaking changes often (last deploys: pump 2026-10-13, pump_fees 2026-10-12,
pump_amm 2026-10-08, mayhem 2026-10-02). Re-check `pump-public-docs` before each milestone.

---

## 1. Program IDs, key accounts, IDLs

### Programs (all Upgradeable Loader v3, same upgrade authority `7gZufwwAo17y5kg8FMyJy2phgpvv9RSdzWtdXiWHjFr8`) [verified]

| Program | ID | ProgramData |
| --- | --- | --- |
| pump (bonding curve) | `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` | `B5MvUwXdiW1NMM6QFFD3ssPKBujD4zMohncbM73Z2BQu` |
| PumpSwap AMM (pump_amm) | `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` | `6naEzKeUuFh1Jeeu51NXQgr5qkXgXtc9WKNct4xynVJc` |
| pump_fees (fee config, sharing, buyback vaults) | `pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ` | `75Uu23mqWBb8LM8vDppqC1mQAnCcBuLXhVaDezVMQLRw` |
| mayhem | `MAyhSmzXzV1pTf7LsNkrNwkWKTo4ougAJ1PPg47MD4e` | `4TXghKDfUy26yBMjicyfSfAp995GbLtNvW1d1uPSknub` |
| pump_agent_payments (tokenized agents) | `AgenTMiC2hvxGebTsgmsD4HHBa8WEcqGFf87iwRRxLo7` | `H6H5jKFLoexLYnponVsKZV7eCEE279yNJWQ2gotN74Sd` |
| Metaplex token metadata (legacy `create` only) | `metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s` | |

Same IDs on devnet for pump / pump_amm (per PUMP_PROGRAM_README).

### Global / config PDAs [verified, exist on mainnet]

| Account | Address | Seeds |
| --- | --- | --- |
| pump Global | `4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf` | `["global"]` @pump |
| pump event authority | `Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1` | `["__event_authority"]` @pump |
| pump mint authority | `TSLvdd1pWpHVjahSpsvCXUbgwsL3JAcvokwaKt1eokM` | `["mint-authority"]` @pump |
| pump global volume accumulator | `Hq2wp8uJ9jCPsYgNHex8RtqdvMPfVGoYwjvF1ATiwn2Y` | `["global_volume_accumulator"]` @pump |
| pump FeeConfig | `8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt` | `["fee_config", pump_id]` @pump_fees |
| pump QuoteControl | `6z6GDdfb2AjR9ZhJmAUQ5cipJCVxQvLJhB2H8mCwTFBP` | `["quote-control"]` @pump (only non-whitelisted quotes) |
| PumpSwap GlobalConfig | `ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw` | `["global_config"]` @pump_amm |
| PumpSwap event authority | `GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR` | `["__event_authority"]` @pump_amm |
| PumpSwap global volume accumulator | `C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw` | `["global_volume_accumulator"]` @pump_amm |
| PumpSwap FeeConfig | `5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx` | `["fee_config", pump_amm_id]` @pump_fees |
| pump_fees FeeProgramGlobal | `CHqnuTkj6sXDFknM652aEFPECZh9qVsBXWkhPohmV9dA` | `["fee-program-global"]` @pump_fees |
| mayhem global params | `13ec7XdrjF3h3YcqBTFDSReRcUFwbCnJaAQspM4j6DDJ` | `["global-params"]` @mayhem |
| mayhem sol vault | `BwWK17cbHxwWBKZkUYvzxLcNQ1YVyaFezduWbtm2de6s` | `["sol-vault"]` @mayhem |
| Global withdraw_authority (migrate_v2 account) | `39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg` | system wallet |
| pump trade ALT (mainnet) | `Hyif6eWb8x88RVrvjPfabsgRYnwkVnyByEXTVTXbUcyP` | lookup table |

### Per-coin PDAs

| Account | Seeds |
| --- | --- |
| bonding curve | `["bonding-curve", mint]` @pump |
| curve token vault | ATA(bonding_curve, mint, Token-2022) |
| curve creator vault (SOL) | `["creator-vault", bonding_curve.creator]` @pump (one vault per creator, shared across all that creator's coins) |
| user volume accumulator | `["user_volume_accumulator", user]` @pump (created by the first v3 trade, 0.0018 SOL rent) |
| sharing config | `["sharing-config", mint]` @pump_fees |
| pool authority | `["pool-authority", mint]` @pump |
| canonical PumpSwap pool | `["pool", u16 index=0, pool_authority, base_mint, quote_mint]` @pump_amm |
| AMM creator vault authority | `["creator_vault", pool.coin_creator]` @pump_amm (note underscore, not hyphen) |
| AMM creator vault token account | ATA(amm creator vault authority, wSOL, SPL Token) |

### Fee recipients [verified]

- 8 normal protocol fee recipients (`62qc2C...`, `7VtfL8...`, `7hTckg...`, `9rPYyA...`, `AVmoTt...`,
  `CebN5W...`, `FWsW1x...`, `G5UZAV...`): system wallets. PumpSwap also lists `JCRGumoE9Qi5BBgULTgdgTLjSgkCMSbF62ZZfGs84JeU`.
- 8 reserved recipients for mayhem coins (`GesfTA...` and the `reserved_fee_recipients` array).
- 8 buyback fee recipients (`5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD` and 7 more, full list in
  `pump-public-docs/docs/FEE_RECIPIENTS.md`). These are **pump_fees-owned BuybackVault accounts
  (208 bytes)**, not wallets. They receive pump's own $PUMP buyback share. Every trade must pass one.

### IDLs and SDKs

- Canonical: https://github.com/pump-fun/pump-public-docs `idl/pump.json`, `idl/pump_amm.json`,
  `idl/pump_fees.json` (+ `.ts` types). Commit read: `639b87f` (2026-10-13, "pump and fee IDLs refreshed").
- `pump_agent_payments.json` is only in the Rust crate (`pump-rust-client` 0.4.1, `idls/`). Copies of all
  four are in `research/pump-litesvm/idl/`.
- On-chain Anchor IDL accounts exist too (`anchor idl fetch <id>`), but the GitHub copies are what pump
  updates and documents.
- SDKs: `@pump-fun/pump-sdk` 4.1.0, `@pump-fun/pump-swap-sdk` 2.1.0, `pump-rust-client` 0.4.1 (crates.io).
  For Anchor CPI use `pump_rust_client` (`declare_program!(pump)`); decode accounts through
  `pump_rust_client::state` (zero-pads old short accounts), not the raw IDL types (see `docs/CPI_README.md`).

### Live Global parameters [verified, decoded from mainnet]

| Field | Value | Meaning |
| --- | --- | --- |
| token_total_supply | 1,000,000,000,000,000 | 1B tokens, 6 decimals |
| initial_virtual_token_reserves | 1,073,000,000,000,000 | 1.073B virtual |
| initial_virtual_sol_reserves | 30,000,000,000 | 30 SOL virtual |
| initial_real_token_reserves | 793,100,000,000,000 | 793.1M sellable on the curve; 206.9M go to the pool at migration |
| pool_migration_fee | 15,000,001 lamports | 0.015 SOL taken at graduation |
| fee_basis_points / creator_fee_basis_points | 95 / 5 | legacy fallbacks; FeeConfig overrides |
| buyback_basis_points | 5000 | 50% of the protocol fee goes to pump's buyback vaults |
| whitelisted_quote_mints | USDC | SOL is native |
| max_configurable_creator_fee_bps | 300 | only for custom-pair coins |
| create_v2_enabled, mayhem_mode_enabled, is_holder_reward_enabled | true | |
| max_curve_depth | 2 | pump-coin-as-quote depth |

---

## 2. Running pump on a local validator

### Has anyone done it?

Yes, pump itself: `pump-rust-client` 0.4.1 ships a local-validator harness (`justfile`,
`src/bin/local_validator`, `src/bin/clone_devnet_accounts`, `artifacts/*.so`). It loads programs with
`TestValidatorGenesis::add_program` and clones these fixed PDAs: pump global, event_authority,
mint_authority, global_volume_accumulator, fee_config, quote_control; pump_amm global_config,
event_authority, global_volume_accumulator, fee_config; pump_agent_payments global_config; mayhem
global_params, sol_vault; the devnet and mainnet ALTs (rewritten to "active from slot 0"). It also
loads `mpl_token_metadata`, `pump_agent_payments`, `terminal_proxy` (`term9YPb9mzAsABaqN71A4xdbxHmpBNZavpBiQKZzN3`)
and, for USDC paths only, `pump_stables_router` + Raydium v4 + OpenBook. Note its defaults bind
8899 / 9000 / 5050 (8899 and 9000 are owned by other projects on this machine); it honours
`PUMP_LOCAL_RPC_PORT` and `PUMP_LOCAL_FAUCET_PORT`. We should not run it as-is; we use our own
10600 block with plain `solana-test-validator`.

Surfpool (mainnet fork that fetches accounts lazily) is another option, but plain cloning is
deterministic and small enough.

### Minimal set, proven [verified with LiteSVM on mainnet bytes]

For a **SOL-paired, non-mayhem** coin, `create_v2` + dev buy + `buy_v3` + `sell_v3` +
`sweep_creator_fee` + `collect_creator_fee_v2` succeeded with only:

- programs: **pump** and **pump_fees** (buys fail with `InvalidProgramExecutable` on `fee_program`
  without pump_fees). mayhem and mpl metadata are **not** executed for a non-mayhem `create_v2`
  (Token-2022 metadata extension, no Metaplex).
- accounts: pump **Global**, pump **FeeConfig**, pump **global_volume_accumulator** (create+buy fails with
  `AccountNotInitialized` without it), and the **wSOL mint** `So11111111111111111111111111111111111111112`
  (v3 trades require it initialized; LiteSVM lacks it, solana-test-validator genesis normally has it,
  check with `solana account So111... -u http://127.0.0.1:10600`).

For **graduation and PumpSwap** (synthetic-migration `buy_v3`, `migrate_v2`, PumpSwap `buy` and `buy_v2`,
both sweeps, `collect_coin_creator_fee`) add:

- program **pump_amm**
- PumpSwap **GlobalConfig**, PumpSwap **FeeConfig**, PumpSwap **global_volume_accumulator**
- the **withdraw_authority** wallet `39azUY...`
- the **wSOL ATAs of the buyback fee recipients** (PumpSwap `buy_v2` fails with `AccountTypeNotSupported`
  6027 when the buyback recipient's quote token account is missing)

Recommended (rent realism on a real validator, cheap to include): the 8 buyback vault accounts, the
8 (+1) protocol fee recipient wallets and their wSOL ATAs, pump_fees FeeProgramGlobal, mayhem program +
global_params + sol_vault (only needed if we ever test mayhem coins).

### Command line (our port block; do not run blindly, check `lsof -ti :10600` first)

```sh
# RPC used only for cloning; key read from file, never printed
export CLONE_RPC="https://mainnet.helius-rpc.com/?api-key=$(cat ~/.config/bordr-watch/helius.key)"
W=So11111111111111111111111111111111111111112; T=TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA
BB="5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD 9M4giFFMxmFGXtc3feFzRai56WbBqehoSeRE5GK7gf7 GXPFM2caqTtQYC2cJ5yJRi9VDkpsYZXzYdwYpGnLmtDL 3BpXnfJaUTiwXnJNe7Ej1rcbzqTTQUvLShZaWazebsVR 5cjcW9wExnJJiqgLjq7DEG75Pm6JBgE1hNv4B2vHXUW6 EHAAiTxcdDwQ3U4bU6YcMsQGaekdzLS3B5SmYo46kJtL 5eHhjP8JaYkz83CWwvGU2uMUXefd3AazWGx4gpcuEEYD A7hAgCzFw14fejgCp387JUJRMNyz4j89JKnhtKU8piqW"
# ATAs: spl-token address --verbose --owner <o> --token $W  (or compute in a script)

solana-test-validator --reset --ledger .ledger/token-chain \
  --rpc-port 10600 --faucet-port 10602 --gossip-port 10603 --dynamic-port-range 10604-10629 \
  -u "$CLONE_RPC" \
  --clone-upgradeable-program 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P \
  --clone-upgradeable-program pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA \
  --clone-upgradeable-program pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ \
  --clone-upgradeable-program MAyhSmzXzV1pTf7LsNkrNwkWKTo4ougAJ1PPg47MD4e \
  --clone 4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf \
  --clone 8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt \
  --clone Hq2wp8uJ9jCPsYgNHex8RtqdvMPfVGoYwjvF1ATiwn2Y \
  --clone ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw \
  --clone 5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx \
  --clone C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw \
  --clone CHqnuTkj6sXDFknM652aEFPECZh9qVsBXWkhPohmV9dA \
  --clone 13ec7XdrjF3h3YcqBTFDSReRcUFwbCnJaAQspM4j6DDJ \
  --clone BwWK17cbHxwWBKZkUYvzxLcNQ1YVyaFezduWbtm2de6s \
  --clone 39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg \
  $(for a in $BB; do echo --clone $a; done) \
  --clone <8 wSOL ATAs of the buyback recipients> \
  --clone <normal fee recipients + their wSOL ATAs (optional)>
```

Notes:

- `--clone-upgradeable-program` copies program + programdata at the current mainnet version. For
  reproducible tests pin the bytes instead: `solana program dump -u "$CLONE_RPC" <id> pump.so` once, then
  `--upgradeable-program <id> pump.so none` (or `--bpf-program`). pump.so and pump_amm.so dump as
  ~10.5 MB (padded programdata), pump_fees 0.95 MB, mayhem 0.78 MB.
- Cloned config accounts keep pump's admin (`FFWtrEQ4B4PKQoVuHYzZq8FabGkVatYzDpEVHsK5rrhF`), so fee tiers
  cannot be changed locally. To test admin paths, `solana account <addr> --output json -u "$CLONE_RPC"`,
  patch the authority bytes, and load with `--account <addr> file.json`.
- Event authority PDAs need not exist (they only sign self-CPIs). Mint authority PDA need not exist.
- Clock: the LiteSVM run set the clock to wall time; test validator does this already.
- Never `pkill` the validator; kill by PID (machine rule).

### Exact account lists per instruction

From the IDL (`idl/pump.json`) and confirmed by execution:

**create_v2** (16 accounts): mint (signer, new Token-2022 mint), mint_authority PDA, bonding_curve,
associated_bonding_curve (Token-2022 ATA), global, user (signer, payer), system, Token-2022, ATA program,
mayhem_program_id, mayhem global_params, mayhem sol_vault, mayhem_state `["mayhem-state", mint]`,
mayhem_token_vault (ATA(sol_vault, mint, T22)), event_authority, pump program.
Args: name (<=32), symbol (<=13), uri (<=200), creator (any non-default pubkey, **does not sign**),
is_mayhem_mode, is_cashback_enabled (must be false/omitted), creator_fee_bps (ignored for SOL),
is_holder_reward.

**buy_v3 / sell_v3 / buy_exact_quote_in_v3** (17 accounts): global, base_mint, quote_mint (pass wSOL),
base_token_program (Token-2022), quote_token_program (SPL Token), bonding_curve,
associated_base_bonding_curve, associated_quote_bonding_curve (derived, unused for SOL), user (signer),
associated_base_user (**must exist**), associated_quote_user (derived, unused for SOL),
user_volume_accumulator, fee_config `8Wf5Ti...`, buyback_fee_recipient (one of the 8), system,
event_authority, pump program. v3 keeps protocol + creator fee on the curve until swept.

**buy_v2 / sell_v2** (27 / 26 accounts): adds fee_recipient + its quote ATA, buyback recipient quote ATA,
creator_vault + its quote ATA, sharing_config, global_volume_accumulator,
associated_user_volume_accumulator, fee_config, fee_program. Pays fees per trade.

**sweep_creator_fee** (pump, 13 accounts): payer (signer), global, base_mint, quote_mint, quote_token_program,
ATA program, system, bonding_curve, associated_quote_bonding_curve, recipient = creator_vault PDA,
associated_quote_recipient, event_authority, program. No data. Permissionless.

**collect_creator_fee_v2** (10 accounts): creator (writable, not signer), creator_token_account
(ATA(creator, SPL, wSOL), unused for SOL), creator_vault, creator_vault_token_account, quote_mint (wSOL),
quote_token_program, ATA program, system, event_authority, program. **Permissionless** [verified: a random
cranker signed, the creator did not].

**migrate_v2** (27 accounts): permissionless once `complete`; needs withdraw_authority account, pump_amm
program, amm global_config, pool, pool_authority, LP mint, vaults, Token-2022, rent sysvar.
`[verified]` ~470k CU.

**PumpSwap sweep_creator_fee** (12 accounts) + **collect_coin_creator_fee** (8 accounts): permissionless;
destination is a **wSOL token account** of `coin_creator` that must already exist.

Measured CU (LiteSVM, mainnet bytes): create_v2 + ATA + dev buy 230-250k; buy_v3 ~70k (incl. ATA create);
sell_v3 ~45k; curve sweep + collect ~55k; synthetic-migration buy_v3 ~61k; migrate_v2 ~470k;
PumpSwap buy (legacy) ~215-275k incl. wSOL wrap; PumpSwap buy_v2 ~88k; 2 sweeps + 2 collects ~130k.

---

## 3. Creator fee mechanics (now)

### Rates [verified from on-chain FeeConfig; matches https://pump.fun/docs/fees, "Last Updated 17 October 2026"]

- **Bonding curve (SOL or USDC pair):** creator 0.30%, protocol 0.95%, LP 0% (total 1.25%). FeeConfig has a
  single tier at threshold 0, so it is flat across the curve.
- **Canonical PumpSwap pool, SOL pair, by SOL market cap (price x 1B):**

| Market cap (SOL) | Creator | Protocol | LP | Total |
| --- | --- | --- | --- | --- |
| 0 - 420 | 0.30% | 0.93% | 0.02% | 1.25% |
| 420 - 1,470 | 0.95% | 0.05% | 0.20% | 1.20% |
| 1,470 - 2,460 | 0.90% | 0.05% | 0.20% | 1.15% |
| 2,460 - 3,440 | 0.85% | | | |
| 3,440 - 4,420 | 0.80% | | | |
| 4,420 - 9,820 | 0.75% | | | |
| 9,820 - 14,740 | 0.70% | | | |
| ... steps of ~4,900 SOL, -0.05% each ... | | | | |
| 49,120 - 54,030 | 0.30% | | | |
| then -0.025% per step | | | | |
| 98,240 and up | 0.05% | 0.05% | 0.20% | 0.30% |

  (Full 24-tier table in pump.fun/docs/fees; the same numbers decode from `5PHirr...` fee_tiers.)
  USDC pairs use `stable_fee_tiers` by USDC market cap. Non-canonical pools: creator 0%.
- Of the protocol fee, `buyback_basis_points = 5000`, so half goes to pump's own buyback vaults on
  every trade. That is pump's $PUMP buyback, not ours.
- Every fee is charged **in the quote asset (SOL for us)**, never in the coin.

### Where it accrues

| Phase | Trade instruction | Creator fee goes to | How it reaches the creator |
| --- | --- | --- | --- |
| Curve | legacy `buy`/`sell`, `buy_v2`/`sell_v2` | paid per trade into `creator_vault` = pump PDA `["creator-vault", creator]` (lamports) | `collect_creator_fee_v2` (lamports to `creator`) |
| Curve | `buy_v3`/`sell_v3`/multi-hop | kept on the curve in `BondingCurve.creator_fee` (lamports sit in the curve account) | `sweep_creator_fee` (curve to creator_vault), then `collect_creator_fee_v2` |
| Pool | PumpSwap legacy `buy`/`sell` | paid per trade into the AMM creator vault ATA (wSOL) of `["creator_vault", coin_creator]` | `collect_coin_creator_fee` (wSOL to coin_creator's wSOL ATA) |
| Pool | PumpSwap `buy_v2`/`sell_v2`/multi-hop | kept in the pool quote vault, counted in `Pool.creator_fees` | PumpSwap `sweep_creator_fee`, then `collect_coin_creator_fee` |

Old and new trade styles mix freely on one coin, so a keeper must always sweep before collecting.
The curve sweep still works after migration. Sweeps and collects are all permissionless and can be in
one transaction [verified: curve sweep + pool sweep + create wSOL ATA + collect_creator_fee_v2 +
collect_coin_creator_fee in one tx, ~130k CU].

Collected forms: **curve fees arrive as native lamports** on the creator account; **pool fees arrive as
wSOL** in the creator's wSOL ATA. The creator vault keeps 890,880 lamports rent.

Note the curve `creator_vault` is keyed by creator, not by mint: if one creator key launches two coins,
fees pool together. Use a creator key dedicated to this one coin.

### Claim instructions summary

- pump: `sweep_creator_fee`, `collect_creator_fee_v2` (legacy `collect_creator_fee` needed the creator
  to sign; v2 does not).
- pump_amm: `sweep_creator_fee`, `collect_coin_creator_fee`; `transfer_creator_fees_to_pump(_v2)` moves AMM
  fees into the curve vault (fee-sharing path).
- pump_fees fee sharing: `create_fee_sharing_config` (creator or pump admin), `update_fee_shares_v2` (once;
  up to 10 shareholders, bps sum 10,000, then admin revoked), `distribute_creator_fees_v2` (pump,
  permissionless). After a coin moves to a sharing config, `collect_creator_fee_v2` /
  `collect_coin_creator_fee` fail.

### Things pump can change under us

- `admin_cto` / `admin_cto_pool` (community takeover by pump admin) can change the creator, or convert the
  coin to a holder rewards coin. The docs say "Contact the CTO team" for creator changes, so creator
  fee routing is ultimately admin-controlled. Pump also said in Jan 2026 creator fees "failed" and has been
  shifting toward holder rewards (cashback deprecated 2026-09-12).
- Fee tiers live in FeeConfig and are admin-editable (they changed on 2025-09-01 and since). Never
  hardcode the rate; read FeeConfig.
- Breaking account-list changes happen every few weeks (fee recipients 2026-04-28, v3 trades 2026-10-08,
  depth-2 quotes 2026-10-13). Pin `pump-rust-client` and re-test on each release.

---

## 4. Dev buy at create

- `create_v2` makes a **Token-2022** mint (decimals 6, MetadataPointer + TokenMetadata extensions, no
  Metaplex). Mint authority and freeze authority are **null** after create, and supply is exactly
  1,000,000,000.000000 [verified]. Fixed 1B supply is guaranteed by pump.
- Create and the dev buy go in **one transaction**: SDK `PUMP_SDK.createV2AndBuyInstructions` emits
  `create_v2` + `createAssociatedTokenAccountIdempotent` + a buy (18 accounts, legacy `buy` shape), so no one
  can buy ahead of the dev. Rust: `PumpSdk::create_v2_and_buy_instruction`. The creator does not need to
  sign, so `creator` can be our reserve program's PDA while a normal wallet pays [verified: creator =
  arbitrary non-signing key, curve recorded it as creator].
- The dev buy itself pays the 0.30% creator fee into our own creator vault (recovered by collect).
- `create_v2` is free (only rent). Graduation costs 0.015 SOL taken from the curve.

### Sizing (constant product on 30 SOL / 1.073B virtual; plus 1.25% fees)

| Dev buy (% of 1B) | Tokens | SOL incl. fees |
| --- | --- | --- |
| 1% | 10M | 0.29 |
| 5% | 50M | 1.49 |
| 10% | 100M | 3.12 |
| 15% | 150M | 4.94 |
| 20% | 200M | 6.96 |
| 25% | 250M | 9.23 |
| 30% | 300M | 11.79 |
| 50% | 500M | 26.5 |
| 79.31% (whole curve, graduates) | 793.1M | 86.1 |

[verified: 3 SOL bought 96,449,438 tokens.] On-chain there is no dev-buy cap except the curve (and with
`buy_v3` a buy can even run past the curve into the pool reserves). What is "sensible" is a product call,
but points to weigh: markets read a large dev share as rug risk; everything above ~10-15% of supply looks
like a bundle on trackers; tokens bought this way sit in a visible PDA, which helps if the reserve's
release rules are on chain and public. A 5-10% reserve (1.5-3.1 SOL) is a typical range; a reserve of
20%+ costs 7+ SOL and moves the price far up the curve for everyone after.

---

## 5. Buyback design (claim creator fees, buy the token, deposit to the reserve)

### CPI into pump: allowed

- Pump publishes `docs/CPI_README.md` with an Anchor wrapper example (`declare_program!(pump)`,
  `pump::cpi::buy_v2`). No instruction or error in the IDLs restricts CPI callers (I scanned all error
  codes; the only "UnauthorizedProgram" check is pump_fees accepting calls only from pump/PumpSwap on its
  internal instructions). pump's own harness loads a `terminal_proxy` program that CPIs into pump.
- Not tested here: a CPI from our own program (needs an Anchor build). The LiteSVM script is the place
  to add it.

### Recommended shape

1. **Creator = a program PDA**, e.g. `reserve_authority = PDA(["fee-sink", mint], reserve_program)`, that is
   **system-owned with no data**. Reason: pump pays curve creator fees by lamport transfer to `creator`, and
   a buy CPI moves SOL from `user` with a System Program transfer, which only works from a system-owned,
   data-less account. Our program signs for it with `invoke_signed`. It must hold at least the rent minimum
   (890,880 lamports) before the first small collect.
2. **Crank (permissionless, anyone can call, or our keeper):**
   - pre-graduation: `sweep_creator_fee` + `collect_creator_fee_v2` (lamports land on the PDA), then CPI
     `buy_exact_quote_in_v3` (or `buy_v3`) with `user = reserve_authority`, spending the collected SOL minus a
     rent buffer, with a `min_tokens_out` from an on-chain quote.
   - post-graduation: curve sweep + collect (old curve fees), PumpSwap `sweep_creator_fee` +
     `collect_coin_creator_fee` (wSOL into the PDA's wSOL ATA), then CPI PumpSwap `buy_exact_quote_in_v2`
     (or `buy_exact_quote_in`) spending the wSOL. Or one `multi_hop_swap` if routing ever matters.
   - tokens land in the PDA's Token-2022 ATA = the **mining reserve vault**, controlled only by the reserve
     program (lock-and-mint bridge to the agent chain reads from here).
3. **Do it all in our program, not a keeper wallet**, so fees can never be diverted: the program only
   permits "collect then buy into vault". A keeper just pays tx fees.
4. **Slippage / MEV:** the crank is public and predictable, so it can be sandwiched. Mitigate with a per-call
   SOL cap, a min-interval, an on-chain min-out computed from current reserves with a tight bound, and
   optionally a Jito bundle from our keeper.
5. **Self-referential fee:** our buyback trades pay the 1.25% (curve) or tiered (pool) fee, 0.30%+ of which
   comes back to us as creator fee next round. Fine, just account for it.
6. **Stable volume accumulator:** each buyer gets a `user_volume_accumulator`; the PDA's one is created on its
   first v3 trade (0.0018 SOL).

### Alternative pump-native options (and why not)

- **Fee sharing config**: point 100% of shares at our PDA. Adds a pump_fees dependency and a one-shot
  admin step; no benefit over plain creator = PDA unless we want to split fees (e.g. reserve 80% / ops 20%,
  up to 10 recipients). Keep in mind as the way to add an ops split later.
- **Tokenized agents (`pump_agent_payments`, `agent_initialize(authority, buyback_bps)`)**: pump's own
  agent product. Its `agent_buyback_trigger` is signed by pump's `global_buyback_authority`
  (`GmFrDZT2cdrqykgTikVdXbe8EtCgzUDM9VsDhQnwsUsG`) and sends bought tokens to a `burn_authority` PDA, i.e. it is
  buy-and-burn run by pump on agent payment revenue, not creator-fee buybacks into our vault. Not a fit for a
  reserve.
- **Holder rewards coin**: creator fee goes to holders, nothing to claim. Incompatible with a buyback
  reserve; do not set `is_holder_reward`.

---

## 6. Open questions for the owner / next steps

- Dev buy size (section 4 table).
- Whether a share of creator fees should go to ops (then use fee sharing, up to 10 recipients).
- Coin metadata: `uri` must point to JSON (pump.fun's UI uploads to IPFS). Coins created by direct program
  call are indexed by pump.fun from events (not verified here).
- Write the reserve program + LiteSVM/test-validator test for the CPI buyback path.

---

## 7. Reproducing the check

`research/pump-litesvm/` (no node_modules, no .so committed):

```sh
cd research/pump-litesvm && npm i
mkdir -p so && for e in pump:6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P pump_amm:pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA \
  pump_fees:pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ mayhem:MAyhSmzXzV1pTf7LsNkrNwkWKTo4ougAJ1PPg47MD4e; do
  solana program dump -u "$RPC" "${e#*:}" "so/${e%%:*}.so"; done
RPC="https://mainnet.helius-rpc.com/?api-key=$(cat ~/.config/bordr-watch/helius.key)" GRAD=1 node svm.js
# knobs: ONLY=global,feeCfg,wsol,gva (subset of cloned configs), PROGS=pump,pump_fees, NOBB=1, NOFR=1, NOATA=1
```

Output of the full run on 2026-10-16: create_v2 + 3 SOL dev buy OK (96.45M tokens, creator vault
9,779,769 lamports); buy_v3 OK (curve creator_fee 5,336,401 waiting); sell_v3 OK; sweep + collect by a
non-creator OK (reserve +16,963,592 lamports); buy_v3 past the curve end OK (complete, synthetic migration);
migrate_v2 by a random user OK (pool coin_creator = reserve); PumpSwap buy + buy_v2 OK; both sweeps + both
collects OK (reserve +270,813,315 lamports + 23,787,592 wSOL).

---

## Sources

- pump-fun/pump-public-docs, commit `639b87f` (2026-10-13): README.md; docs/PUMP_PROGRAM_README.md,
  PUMP_CREATOR_FEE_README.md, PUMP_SWAP_CREATOR_FEE_README.md, FEE_PROGRAM_README.md, FEE_RECIPIENTS.md,
  BREAKING_FEE_RECIPIENT.md, CPI_README.md, SYNTHETIC_MIGRATION.md, HOLDER_REWARDS_README.md,
  docs/instructions/COIN_CREATION.md, COLLECT_CREATOR_FEE.md, SWEEP_FEES.md, TRADE_V3.md,
  CREATOR_FEE_SHARING.md; idl/*.json. https://github.com/pump-fun/pump-public-docs
- pump-rust-client 0.4.1 (crates.io): justfile, src/bin/local_validator, src/bin/clone_devnet_accounts,
  src/constants.rs, idls/pump_agent_payments.json. https://crates.io/crates/pump-rust-client/0.4.1
- @pump-fun/pump-sdk 4.1.0 and @pump-fun/pump-swap-sdk 2.1.0 (npm), src/sdk.ts, src/pda.ts, src/fees.ts.
- pump.fun Fees page (scraped 2026-10-16, page says "Last Updated: 17 October 2026"): https://pump.fun/docs/fees
- Mainnet account reads via Helius RPC, 2026-10-16 (Global, FeeConfigs, GlobalConfig, programdata, fee
  recipients, deploy slots 456494698 / 454596406 / 456132171 / 452655053 dated with getBlockTime).
- Coinspeaker via Yahoo Finance, 2026-01-10, "Pump.fun Admits Creator Fees Failed, Pivoting to Trader-Set
  Rewards": https://finance.yahoo.com/news/pump-fun-admits-creator-fees-160120983.html
- Blockworks on the dynamic fee model: https://blockworks.com/news/pumpdotfun-fee-model
