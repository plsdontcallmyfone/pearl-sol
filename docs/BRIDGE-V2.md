# Two-side bridge on the Cadre chain (bridge v2)

Status 2026-11-04: implemented in chain/ (agent-types, state, executor, consensus-pow, rpc, mempool), crates/pearl-verify-sbf, crates/cadre-cli and services/relayer codecs. Not deployed. Plan: docs/RH-TESTNET-PLAN.md sections 2.3 and 4.2. The EVM side writes its own layouts in token-evm/FORMATS.md; this file is the chain side, to diff against it.

## 1. What changed and what did not

The chain used to assume one external side (Solana). Bridge v2 adds bridge domains:

| Domain | Side | Kinds |
|---|---|---|
| 1 | Solana | the original BridgeUnlock (6), BridgeLock (7), SetRoute (9), ClearRoute (10), unchanged byte for byte |
| 2 | Robinhood Chain (testnet chain id 46630) | BridgeUnlockV2 (40), BridgeLockV2 (41), SetRouteV2 (42); ClearRoute (10) clears either |
| 3..=15 | later sides | same v2 kinds, once configured |

Nothing about an existing kind, record, message or log entry changed. The v2 kinds are refused until the chain parameter `bridge_v2_height` (default unset = u64::MAX) is reached, and they always refuse domain 1. A determinism test (chain/executor/tests/bridge2_golden.rs) runs a scripted history of the original kinds and checks the hash of every write set, withdrawal-log entry and coinbase receipt against a value taken on the base commit f3be94a.

From activation there is one new rule on domain 1: a BridgeLock, or a lien payout to Solana, may not exceed domain 1's outstanding (section 5). While no other side has unlocked anything, domain 1's outstanding is the whole circulating supply, so the rule cannot refuse a lock a sender can pay for.

## 2. Numbers (reserved)

- Tx kinds 40, 41, 42.
- AdminConfig keys 50 (`bridge_v2_height`), 51 (`bridge_domain`); 52-55 reserved. They live in their own record, not in the config record.
- State schema tags 53 (params), 54 (v2 deposit), 55 (v2 withdrawal), 56 (domain counters), 57 (lien domain).
- Withdrawal-log entry version byte 0x02.

## 3. Byte layouts

All integers little-endian (the chain's convention everywhere, the v1 entry included). `||` is concatenation.

### 3.1 Tx bodies (after the common prefix `version 1 || chain_id u32 || from [32] || nonce u64 || fee u64 || kind u8`, before `sig [64]`)

```text
BridgeUnlockV2 (40)  domain u8 | ext_nonce u64 | recipient [32] | amount_ext u64 | job_order u8 (0|1)
                     | [PostJob body if 1] | count u8 | (attester pubkey [32] | sig [64])*
BridgeLockV2   (41)  domain u8 | recipient [32] | amount u64
SetRouteV2     (42)  domain u8 | recipient [32] | route_bps u16 | until_height u64 | cap u64 | release_key [32]
```

- `amount_ext` is in the side's token units (6 decimals); the chain credits `amount_ext x 1000` base units (9 decimals), as on Solana.
- `amount` of BridgeLockV2 is in chain base units and must be a positive multiple of 1000; the entry carries `amount / 1000`.
- An EVM recipient is the 20-byte address left-padded with 12 zero bytes. For a domain whose bridge address is 20 bytes (an EVM side), the chain refuses a BridgeLockV2 / SetRouteV2 recipient whose first 12 bytes are not zero.
- BridgeUnlockV2 pays no fee (like kind 6); the attester set (AdminConfig `attesters`, `attester_threshold`) signs it, the same ed25519 keys as for Solana deposits.
- PostJob body and its `deadline_height` (relative for an order) are exactly as in kind 6.

### 3.2 Attested messages (RAW ed25519 over these bytes, no framing)

```text
deposit_v2        b"cadre:deposit:v2"       || domain u8 || ext_chain_id u64 || bridge_addr (20 or 32 raw bytes)
                                            || ext_nonce u64 || recipient [32] || amount_ext u64
deposit_order_v2  b"cadre:deposit-order:v2" || the same fields || sha256(PostJob body)
```

`ext_chain_id` and `bridge_addr` come from the domain's configuration on the chain (section 4), so a signature for one deployment never unlocks on another, and changing a domain's bridge address voids older signatures. For Robinhood Chain testnet: `domain = 2`, `ext_chain_id = 46630`, `bridge_addr` = the CadreBridge contract's 20 bytes. Example (chain/agent-types/vectors/bridge2.json): bridge `0x5a5a...5a`, nonce 5, amount 2,500,000 gives a 93-byte message (16 + 1 + 8 + 20 + 8 + 32 + 8).

Order job id of a v2 order: `sha256(b"cadre:order-job:v2" || domain u8 || ext_nonce u64)`. Its Pearl header is the same as a Solana order's: `header_id = recipient`, `terms_hash = blake3("pearl-sol:order-terms:v1" || recipient || m u32 || n u32 || k u32 || max_blocks u32 || deadline_blocks u64)`.

### 3.3 Withdrawal-log entry v2 (90 bytes)

```text
0x02 | domain u8 | nonce u64 | from [32] | recipient [32] | amount_ext u64 | height u64
```

- `nonce` counts per domain from 0 (dense, for the side's `processed` bitmap).
- Same log and same `withdraw_root` in the header as v1 entries; no header change. MMR element and proof format are the v1 ones (Commonware keyless MMR, sha256).
- Log order inside one block: all v1 entries of the block in tx order, then all v2 entries in tx order. Locations: `log_base + i` for v1 entry i, `log_base + v1_count + j` for v2 entry j.
- The Solana bridge's `release_proof` requires `entry.len() == 88` (token/programs/bridge/src/lib.rs, `WITHDRAWAL_ENTRY_LEN`) before reading any field, so it refuses every v2 entry as deployed; chain/state/src/bridge2.rs `solana_bridge_refuses_v2_entries_by_length` reads that source and checks it. The Solana signer-mode release only signs what getBridgeEvents (v1) lists, which never includes v2 entries.
- The EVM bridge must require `len == 90 && entry[0] == 0x02 && entry[1] == its domain` and parse the u64 fields little-endian.

### 3.4 AdminConfig values

```text
key 50 bridge_v2_height  u64 LE. Must be above the admin tx's block height (or u64::MAX to cancel), and only while
                         not active; once the chain reaches it the height is fixed for good.
key 51 bridge_domain     domain u8 (2..=15) | ext_chain_id u64 | addr_len u8 (20|32) | addr | paused u8 (0|1)
                         Adds or replaces one domain (at most 8). Allowed any time; the admin only.
```

### 3.5 State records (own records; absent = defaults, so state roots are unchanged until an admin sets one)

```text
tag 53  BridgeParams   key blake3("pearl-sol:state:v1" || 53)
        53 | 1 | activation_height u64 | count u8 | (domain u8 | ext_chain_id u64 | addr_len u8 | addr | paused u8)*
tag 54  DepositV2      id domain u8 || nonce u64   54 | height u64 | recipient [32] | amount_ext u64
tag 55  WithdrawalV2   id domain u8 || nonce u64   55 | the 90-byte entry
tag 56  DomainStats    id domain u8                56 | unlocked u64 | locked u64 | deposit_count u64 | withdrawal_count u64
tag 57  RouteDomain    id account [32]             57 | domain u8   (only while a lien pays out to domain >= 2)
```

A lien is the same Route record (tag 10) as before; only its domain sits in tag 57.

## 4. Rules

BridgeUnlockV2: v2 active; bridge not paused; domain configured (2..=15) and not paused; `(domain, ext_nonce)` not processed; attester threshold met over the v2 message; amount positive; recipient not the escrow. Moves `amount_ext x 1000` from the escrow to the recipient, adds it to the domain's `unlocked`. With a job order it creates the order job exactly like a Solana order (job id per 3.2).

BridgeLockV2: v2 active; bridge and domain running; recipient set (and padded for EVM); amount a positive multiple of 1000; `locked + amount <= unlocked` for the domain. Moves `amount` from the sender to the escrow, appends a v2 entry with the domain's next nonce.

SetRouteV2: v2 active, domain running, recipient rules as above, and the SetRoute rules (one lien at a time, an active lien can only be extended with the same domain, recipient and release key). A v1 SetRoute cannot extend a lien to another domain, and replacing an ended one resets its domain to 1. Payouts of a domain >= 2 lien write v2 entries; when the domain cannot take them (paused, or its outstanding is too small) the lien keeps them pending, exactly as while the bridge is paused.

## 5. Escrow accounting per side

One escrow, per-domain counters. For a domain d >= 2, `outstanding_d = unlocked_d - locked_d` is what d's vault must hold; nothing ever takes `locked_d` above `unlocked_d`. Domain 1 has no counter record: its outstanding is derived,

    outstanding_1 = TOTAL_SUPPLY - escrow_balance - sum(outstanding_d, d >= 2)

which equals the relayer's existing `total - escrow` until another side exists. That makes domain 1's counter correct at activation with no migration and no snapshot. From activation, a BridgeLock or a lien payout to Solana must not exceed `outstanding_1`. Relayer invariant per side: `vault_side x 1000 == outstanding_side + pending_side`.

## 6. RPC (docs/RPC.md)

- `getParams`: `bridge_v2_height` (null when unset), `bridge_v2_active` (for the next block), `bridge_domains` `[{domain, ext_chain_id, bridge_addr, evm, paused}]`, `keys.bridge_v2_height` = 50, `keys.bridge_domain` = 51.
- `getDeposit {domain, nonce}`: with `domain` >= 2 reads the v2 replay set; without `domain` (or 1) it is the original method with `solana_nonce`.
- `getWithdrawalProof {domain, nonce, at_height?}`: v2 entry, location, MMR proof and root, same shape as v1 plus `withdrawal.version = 2`, `recipient_evm`.
- `getBridgeEvents {domain, from_nonce?, limit?}`: v2 withdrawals of a domain.
- `getBridgeCounters`: escrow balance and per-domain `{unlocked, locked, outstanding, deposit_count, withdrawal_count}`; domain 1 marked `derived`.
- `withdrawals` WebSocket topic: v2 entries arrive with `version: 2` and `domain`.
- `sendTx` refuses a v2 kind before activation, an unknown or paused domain, bad attester signatures and an already processed `(domain, nonce)`.

## 7. Upgrading the running devnet (chain_id 1128350803)

The v2 code is additive, so the devnet keeps its genesis and history:

1. Build `agent-node` (chain/bin/agent-node) from this branch and install it on every node (boxes 0-2 and any outside node), one box at a time with health checks, like earlier rollouts. Miners (`pearl-miner`) and header-relay need no change for the activation itself. Old nodes can not decode kinds 40-42 nor AdminConfig keys 50-51, so nothing in step 2 may be sent before every node runs the new binary.
2. Admin sends AdminConfig key 51 with domain 2's value (chain id 46630, the deployed CadreBridge address, paused 0).
3. Admin sends AdminConfig key 50 with a height comfortably ahead of the tip (for example tip + 100).
4. Before the first v2 order (BridgeUnlockV2 with a job order) is mined, upgrade the Solana light client (token/programs/lightclient, which links crates/pearl-verify-sbf) and header-relay, so they can rebuild v2 order jobs; otherwise the light client cannot verify blocks mining such a job. Plain v2 deposits and v2 withdrawals do not need it.
5. Start the RH relayer side (services/relayer EvmSide, separate process and state file) only after `getParams.bridge_v2_active` is true.
