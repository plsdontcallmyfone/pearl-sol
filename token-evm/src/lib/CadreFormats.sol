// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

/// Byte layouts shared with the Cadre chain (BridgeUnlockV2 / BridgeLockV2), the relayer and
/// the attesters. Every integer is little endian, as on the chain and the Solana side.
/// token-evm/FORMATS.md is the written spec; test/vectors/formats.json is an independent
/// Rust encoding of the same bytes.
library CadreFormats {
    /// Bridge domains: 1 = Solana (v1 messages and entries), 2 = Robinhood Chain.
    uint8 internal constant DOMAIN_SOLANA = 1;
    uint8 internal constant DOMAIN_RH = 2;

    uint8 internal constant ENTRY_VERSION_V2 = 0x02;
    uint256 internal constant ENTRY_V2_LEN = 90;

    /// 1 token base unit (6 dp) = 1000 chain base units (9 dp).
    uint64 internal constant EXT_TO_CHAIN = 1000;

    bytes internal constant DEPOSIT_TAG = "cadre:deposit:v2";
    bytes internal constant DEPOSIT_ORDER_TAG = "cadre:deposit-order:v2";
    bytes internal constant ORDER_JOB_TAG = "cadre:order-job:v2";

    error BadEntry();

    struct Entry {
        uint8 domain;
        uint64 nonce;
        bytes32 from;
        bytes32 recipient;
        uint64 amount;
        uint64 height;
    }

    function le16(uint16 v) internal pure returns (bytes2) {
        return bytes2(uint16((v >> 8) | (v << 8)));
    }

    function le32(uint32 v) internal pure returns (bytes4) {
        v = ((v & 0xff00ff00) >> 8) | ((v & 0x00ff00ff) << 8);
        v = (v >> 16) | (v << 16);
        return bytes4(v);
    }

    function le64(uint64 v) internal pure returns (bytes8) {
        v = ((v & 0xff00ff00ff00ff00) >> 8) | ((v & 0x00ff00ff00ff00ff) << 8);
        v = ((v & 0xffff0000ffff0000) >> 16) | ((v & 0x0000ffff0000ffff) << 16);
        v = (v >> 32) | (v << 32);
        return bytes8(v);
    }

    /// Reads a little-endian u64 at `off` of `b`.
    function readLe64(bytes memory b, uint256 off) internal pure returns (uint64 v) {
        for (uint256 i = 0; i < 8; i++) {
            v |= uint64(uint8(b[off + i])) << uint64(8 * i);
        }
    }

    function readBytes32(bytes memory b, uint256 off) internal pure returns (bytes32 v) {
        assembly {
            v := mload(add(add(b, 32), off))
        }
    }

    /// Chain-side attester message for a plain RH deposit (signed with ed25519 by the
    /// attesters for BridgeUnlockV2; nothing on the EVM side verifies it):
    /// "cadre:deposit:v2" || domain u8 || chain_id u64 || bridge [20] || nonce u64 ||
    /// recipient [32] || amount u64.
    function depositMessage(uint64 chainId, address bridge, uint64 nonce, bytes32 recipient, uint64 amount)
        internal
        pure
        returns (bytes memory)
    {
        return abi.encodePacked(
            DEPOSIT_TAG, DOMAIN_RH, le64(chainId), bridge, le64(nonce), recipient, le64(amount)
        );
    }

    /// Order deposit: the same fields, a different tag, then sha256(PostJob body).
    function depositOrderMessage(
        uint64 chainId,
        address bridge,
        uint64 nonce,
        bytes32 recipient,
        uint64 amount,
        bytes32 bodyHash
    ) internal pure returns (bytes memory) {
        return abi.encodePacked(
            DEPOSIT_ORDER_TAG, DOMAIN_RH, le64(chainId), bridge, le64(nonce), recipient, le64(amount), bodyHash
        );
    }

    /// Chain job id of the order paid by RH deposit `nonce`:
    /// sha256("cadre:order-job:v2" || domain u8 || chain_id u64 || bridge [20] || nonce u64).
    function orderJobId(uint64 chainId, address bridge, uint64 nonce) internal pure returns (bytes32) {
        return sha256(abi.encodePacked(ORDER_JOB_TAG, DOMAIN_RH, le64(chainId), bridge, le64(nonce)));
    }

    /// Parses a v2 withdrawal entry: 0x02 || domain u8 || nonce u64 || from [32] ||
    /// recipient [32] || amount u64 || height u64 (90 bytes).
    function parseEntry(bytes memory e) internal pure returns (Entry memory r) {
        if (e.length != ENTRY_V2_LEN || uint8(e[0]) != ENTRY_VERSION_V2) revert BadEntry();
        r.domain = uint8(e[1]);
        r.nonce = readLe64(e, 2);
        r.from = readBytes32(e, 10);
        r.recipient = readBytes32(e, 42);
        r.amount = readLe64(e, 74);
        r.height = readLe64(e, 82);
    }

    function encodeEntry(Entry memory r) internal pure returns (bytes memory) {
        return abi.encodePacked(
            ENTRY_VERSION_V2, r.domain, le64(r.nonce), r.from, r.recipient, le64(r.amount), le64(r.height)
        );
    }

    /// An EVM address as a 32-byte chain recipient (left padded with zeros).
    function toBytes32(address a) internal pure returns (bytes32) {
        return bytes32(uint256(uint160(a)));
    }

    /// The address in a 32-byte recipient, or address(0) if the top 12 bytes are not zero.
    function toAddress(bytes32 r) internal pure returns (address) {
        if (uint256(r) >> 160 != 0) return address(0);
        return address(uint160(uint256(r)));
    }
}
