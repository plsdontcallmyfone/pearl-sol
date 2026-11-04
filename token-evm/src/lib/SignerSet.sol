// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {ECDSA} from "solady/utils/ECDSA.sol";

/// M-of-N secp256k1 signer set (1..10 distinct keys). A signature bundle passes when it
/// holds at least `threshold` signatures, each recovering to a configured signer, in
/// strictly ascending signer-address order (which also makes them distinct).
abstract contract SignerSet {
    uint256 public constant MAX_SIGNERS = 10;

    address[] internal _signers;
    mapping(address => bool) public isSigner;
    uint8 public threshold;

    event SignersChanged(address[] signers, uint8 threshold);

    error BadSignerSet();
    error BadThreshold();
    error NotEnoughSignatures();
    error BadSignature();

    function signers() external view returns (address[] memory) {
        return _signers;
    }

    function _setSigners(address[] memory s, uint8 t) internal {
        if (s.length == 0 || s.length > MAX_SIGNERS) revert BadSignerSet();
        if (t == 0 || t > s.length) revert BadThreshold();
        for (uint256 i = 0; i < _signers.length; i++) {
            isSigner[_signers[i]] = false;
        }
        for (uint256 i = 0; i < s.length; i++) {
            if (s[i] == address(0) || isSigner[s[i]]) revert BadSignerSet();
            isSigner[s[i]] = true;
        }
        _signers = s;
        threshold = t;
        emit SignersChanged(s, t);
    }

    function _checkSigs(bytes32 digest, bytes[] calldata sigs) internal view {
        if (sigs.length < threshold) revert NotEnoughSignatures();
        address last;
        for (uint256 i = 0; i < sigs.length; i++) {
            address a = ECDSA.tryRecoverCalldata(digest, sigs[i]);
            if (a <= last || !isSigner[a]) revert BadSignature();
            last = a;
        }
    }
}
