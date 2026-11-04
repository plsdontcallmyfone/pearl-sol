# token-evm byte formats (Robinhood Chain side, domain 2)

What the Cadre chain (BridgeUnlockV2 / BridgeLockV2), the relayer, the attesters and the
header oracle service must produce or check to match token-evm/. Every integer inside a
Cadre message or entry is little endian (as on the chain and the Solana side). EIP-712
structs use ordinary ABI encoding. Plan: docs/RH-TESTNET-PLAN.md 2.3 and 4.2.

Chain side: docs/BRIDGE-V2.md in the chain builder's branch. Every layout below was diffed
against it and matches (2026-11-04); `test/ChainBridge2.t.sol` checks the deposit, order
deposit, order job id, PostJob body and v2 entry bytes against the chain's own vectors
(`test/vectors/chain-bridge2.json`, copied from chain/agent-types/vectors/bridge2.json by
`vectors-gen/chain_bridge2.py`; the file's `source` field names the commit).

Vectors: `test/vectors/formats.json` (independent Rust encoding in `vectors-gen/`, the
Solidity tests compare byte for byte), `test/vectors/mmr.json` (commonware-storage MMR),
`test/vectors/devnet-withdrawals.json` (real devnet v1 withdrawals). Regenerate with
`cargo run --release` in `vectors-gen/`.

## 1. Constants

| Name | Value |
|---|---|
| domain Solana | `1` (v1 messages and 88-byte v1 entries, unchanged) |
| domain RH | `2` (testnet). The chain takes v2 domains 2..=15; `CadreBridge.domain` is a constructor immutable, and a redeployed bridge takes a NEW domain (the chain's replay set is keyed by (domain, nonce)) |
| RH testnet chain id | `46630` (the contracts use `block.chainid`) |
| bridge address in messages | the 20-byte CadreBridge address, raw (not padded) |
| token units | CADRE ERC-20 has 6 decimals; 1 token base unit = 1000 chain units (`EXT_TO_CHAIN`) |
| amount bound | deposits refuse `amount > u64::MAX / 1000` |

## 2. RH deposit -> Cadre chain (BridgeUnlockV2)

`CadreBridge.Deposited(uint64 indexed nonce, address indexed sender, bytes32 indexed
cadreRecipient, uint64 amount, address refundTo)`; state readable with `deposits(nonce)`
(`sender, amount, refundTo, blockNumber, recipient, refunded`) and `depositNonce()` (next
nonce; nonces are dense from 0, one counter per bridge). `refundTo != 0` marks an order
deposit (only the storefront can make one).

Replay key on the chain: `(domain = 2, nonce)`. Unlock amount: `amount x 1000` chain units
to `cadreRecipient` (a 32-byte chain account).

### 2.1 Plain deposit message (attesters sign with ed25519 for BridgeUnlockV2)

    "cadre:deposit:v2"      16 bytes ASCII
    domain                  u8  (the bridge's domain, 2 on testnet)
    chain_id                u64 LE = 46630
    bridge                  [20] CadreBridge address
    nonce                   u64 LE
    recipient               [32]
    amount                  u64 LE (token base units, 6 dp)

93 bytes. `CadreBridge.depositMessage(nonce)` returns exactly these bytes.

### 2.2 Order deposit message

    "cadre:deposit-order:v2"  22 bytes ASCII
    domain u8 = 2 || chain_id u64 LE || bridge [20] || nonce u64 LE || recipient [32] ||
    amount u64 LE || body_hash [32]

`body_hash = sha256(PostJob body)` (section 4). `CadreStorefront.depositOrderMessage(nonce)`
returns exactly these bytes; the full body is in `OrderBody(uint64 indexed nonce, bytes body)`.

### 2.3 Order job id

    job_id = sha256("cadre:order-job:v2" || domain u8 || nonce u64 LE)

The chain's rule (BRIDGE-V2.md 3.2): no chain id and no bridge address, because the domain
already separates deployments. Stored in `getOrder(nonce).jobId` and emitted in
`OrderPlaced`. The v1 Solana rule (`sha256("pearl-sol:order-job:v1" || nonce LE)`) would
collide across domains, so RH order jobs MUST use this one. The order job's Pearl header
(`header_id = recipient`, `terms_hash`) is built by the chain exactly as for a Solana order.

## 3. Cadre chain -> RH withdrawal (BridgeLockV2)

### 3.1 Withdrawal entry v2 (90 bytes, appended to the one withdrawal log)

    offset  len  field
    0       1    version   = 0x02
    1       1    domain    = 2 for RH
    2       8    nonce     u64 LE, per-domain dense counter (domain 2 starts at 0)
    10      32   from      chain account that locked
    42      32   recipient EVM address left padded: 12 zero bytes || address [20]
    74      8    amount    u64 LE, token base units (6 dp) = chain amount / 1000
    82      8    height    u64 LE, block height of the BridgeLockV2

The RH bridge refuses: length != 90, version != 0x02, domain != its own, a recipient whose top
12 bytes are not zero, amount 0, nonce below `minWithdrawalNonce` (immutable, 0 on the
first deploy), an already processed nonce.

### 3.2 MMR leaf and proof

Leaf element = Commonware keyless `Append(entry)` encoding: `0x01 || varint(len) || entry`;
for a v2 entry that is `0x01 0x5a || entry` (92 bytes). Proof = the codec encoding returned
by node RPC `getWithdrawalProof`: `varint leaves || varint inactive_peaks || varint count ||
count x 32-byte digests`, verified against `withdraw_root` of a final header (SHA-256 MMR,
BackwardFold bagging; `src/lib/Sha256Mmr.sol` is a line-for-line port of
crates/pearl-verify-sbf/src/mmr.rs). `location` is the entry's leaf index in the whole log
(shared by every domain; inside one block the chain appends all v1 entries in tx order,
then all v2 entries in tx order).

`CadreBridge.releaseProof(bytes entry, uint64 location, bytes proof, bytes32 headerHash)`.

### 3.3 Signer release (EIP-712, secp256k1)

    domain   name "CadreBridge", version "2", chainId, verifyingContract = CadreBridge
    Release(uint64 nonce,address recipient,uint64 amount)

`nonce` is the domain-2 withdrawal nonce of the entry, `amount` in token base units.
`CadreBridge.release(nonce, recipient, amount, bytes[] sigs)`. Signer and proof paths share
one processed bitmap (`isProcessed(nonce)`): a withdrawal pays once whichever path is used.

### 3.4 Refund of an order deposit (EIP-712)

    Refund(uint64 nonce,address buyer,uint64 amount)      same domain as 3.3

`nonce` is the RH deposit nonce, `buyer` = `deposits(nonce).refundTo`, `amount` =
`deposits(nonce).amount`. Refused unless the deposit is an order deposit and the storefront
says the order is Pending or Expired; at most once. Signers must never sign a refund for a
deposit they signed an unlock for, and vice versa (the chain cannot see RH).

### 3.5 Signature bundles (all contracts)

`bytes[] sigs`: 65-byte `r || s || v` (or 64-byte EIP-2098), at least `threshold` of them,
each recovering to a configured signer, sorted by strictly ascending signer address.

## 4. PostJob body (the storefront's order job, SPEC 4.1 order, LE)

    m u32 || n u32 || k u32 || config [52] || hash_a [32] || hash_b [32] ||
    data_uri_len u16 || data_uri || W u64 || P u64 || deadline u64 || max_blocks u32 ||
    evaluator_count u8 || evaluators [32]* || threshold u8

Same layout as the Solana storefront. `W = (tokens - P_tok) x 1000`, `P = P_tok x 1000`,
`P_tok = tokens x protocolFeeBps / 10000`, so `W + P == amount x 1000` (the chain only
needs `W + P <= amount x 1000`: it debits W + P from the unlocked recipient). `deadline` is
RELATIVE (blocks after the unlock height). Emitted verbatim in `OrderBody`.

## 5. Final headers (CadreHeaderOracle, EIP-712)

    domain   name "CadreHeaderOracle", version "1", chainId, verifyingContract = oracle
    FinalHeader(uint64 cadreChainId,bytes32 blockHash,uint64 height,bytes32 txRoot,bytes32 stateRoot,bytes32 withdrawRoot)

`cadreChainId` is the Cadre genesis chain_id (constructor immutable). `postFinalHeader(bytes
header256, bytes32 blockHash, sigs)` reads height (u64 LE at 4), tx_root (120), state_root
(152), withdraw_root (184) from the raw 256-byte header; `postFinal(...)` takes the fields.
Attesters sign only FINAL headers; two hashes at one height are refused.

## 6. Storefront results (EIP-712)

    domain   name "CadreStorefront", version "1", chainId, verifyingContract = storefront
    Result(bytes32 jobId,bytes32 deliverer,bytes32 resultHash)
    Expired(bytes32 jobId)

`deliverer` is the 32-byte chain account. These replace the Solana ed25519
`pearl-sol:result:v1` / `pearl-sol:expired:v1` messages on the RH side.
