// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {EIP712} from "solady/utils/EIP712.sol";
import {Ownable} from "solady/auth/Ownable.sol";
import {SignerSet} from "./lib/SignerSet.sol";
import {CadreFormats} from "./lib/CadreFormats.sol";
import {ICadreHeaders} from "./interfaces/ICadre.sol";

/// Phase 1 header source for the RH side: M-of-N secp256k1 attesters sign FINAL Cadre
/// headers (EIP-712 `FinalHeader`, bound to the Cadre chain id), and anyone posts them.
/// No blake3 on chain: the attesters vouch that `blockHash` is the hash of a final header
/// with these roots (the same trust as the Solana bridge's signer mode). Only final
/// headers are posted; one post covers every earlier withdrawal (the log is append-only).
/// Two different hashes at one height are refused (an attester equivocation stops here).
contract CadreHeaderOracle is ICadreHeaders, EIP712, Ownable, SignerSet {
    bytes32 public constant FINAL_HEADER_TYPEHASH = keccak256(
        "FinalHeader(uint64 cadreChainId,bytes32 blockHash,uint64 height,bytes32 txRoot,bytes32 stateRoot,bytes32 withdrawRoot)"
    );

    uint256 internal constant HEADER_LEN = 256;
    uint256 internal constant OFF_HEIGHT = 4;
    uint256 internal constant OFF_TX_ROOT = 120;
    uint256 internal constant OFF_STATE_ROOT = 152;
    uint256 internal constant OFF_WITHDRAW_ROOT = 184;

    struct Record {
        uint64 height;
        bool final_;
        bytes32 txRoot;
        bytes32 stateRoot;
        bytes32 withdrawRoot;
    }

    /// The Cadre chain these headers belong to (genesis chain_id).
    uint64 public immutable cadreChainId;
    mapping(bytes32 => Record) internal _records;
    mapping(uint64 => bytes32) public hashAtHeight;
    uint64 public latestHeight;
    bytes32 public latestHash;

    event FinalHeaderPosted(
        bytes32 indexed blockHash, uint64 indexed height, bytes32 txRoot, bytes32 stateRoot, bytes32 withdrawRoot
    );

    error BadHeader();
    error AlreadyPosted();
    error ConflictingHeader(uint64 height, bytes32 existing);

    constructor(address owner_, uint64 cadreChainId_, address[] memory attesters, uint8 threshold_) {
        _initializeOwner(owner_);
        cadreChainId = cadreChainId_;
        _setSigners(attesters, threshold_);
    }

    function _domainNameAndVersion() internal pure override returns (string memory, string memory) {
        return ("CadreHeaderOracle", "1");
    }

    function setAttesters(address[] calldata attesters, uint8 threshold_) external onlyOwner {
        _setSigners(attesters, threshold_);
    }

    function domainSeparator() external view returns (bytes32) {
        return _domainSeparator();
    }

    function finalHeaderDigest(bytes32 blockHash, uint64 height, bytes32 txRoot, bytes32 stateRoot, bytes32 withdrawRoot)
        public
        view
        returns (bytes32)
    {
        return _hashTypedData(
            keccak256(abi.encode(FINAL_HEADER_TYPEHASH, cadreChainId, blockHash, height, txRoot, stateRoot, withdrawRoot))
        );
    }

    /// Posts a final header from its 256 raw bytes (roots read at the SPEC 3.1 offsets).
    function postFinalHeader(bytes calldata header, bytes32 blockHash, bytes[] calldata sigs) external {
        if (header.length != HEADER_LEN) revert BadHeader();
        bytes memory h = header;
        _post(
            blockHash,
            CadreFormats.readLe64(h, OFF_HEIGHT),
            CadreFormats.readBytes32(h, OFF_TX_ROOT),
            CadreFormats.readBytes32(h, OFF_STATE_ROOT),
            CadreFormats.readBytes32(h, OFF_WITHDRAW_ROOT),
            sigs
        );
    }

    /// Posts a final header from its fields.
    function postFinal(
        bytes32 blockHash,
        uint64 height,
        bytes32 txRoot,
        bytes32 stateRoot,
        bytes32 withdrawRoot,
        bytes[] calldata sigs
    ) external {
        _post(blockHash, height, txRoot, stateRoot, withdrawRoot, sigs);
    }

    function _post(
        bytes32 blockHash,
        uint64 height,
        bytes32 txRoot,
        bytes32 stateRoot,
        bytes32 withdrawRoot,
        bytes[] calldata sigs
    ) internal {
        if (blockHash == bytes32(0)) revert BadHeader();
        if (_records[blockHash].final_) revert AlreadyPosted();
        bytes32 existing = hashAtHeight[height];
        if (existing != bytes32(0)) revert ConflictingHeader(height, existing);
        _checkSigs(finalHeaderDigest(blockHash, height, txRoot, stateRoot, withdrawRoot), sigs);
        _records[blockHash] = Record(height, true, txRoot, stateRoot, withdrawRoot);
        hashAtHeight[height] = blockHash;
        if (latestHash == bytes32(0) || height > latestHeight) {
            latestHeight = height;
            latestHash = blockHash;
        }
        emit FinalHeaderPosted(blockHash, height, txRoot, stateRoot, withdrawRoot);
    }

    function isFinal(bytes32 blockHash) external view returns (bool) {
        return _records[blockHash].final_;
    }

    function roots(bytes32 blockHash)
        external
        view
        returns (uint64 height, bytes32 txRoot, bytes32 stateRoot, bytes32 withdrawRoot)
    {
        Record storage r = _records[blockHash];
        return (r.height, r.txRoot, r.stateRoot, r.withdrawRoot);
    }

    function latestFinal() external view returns (uint64, bytes32) {
        return (latestHeight, latestHash);
    }
}
