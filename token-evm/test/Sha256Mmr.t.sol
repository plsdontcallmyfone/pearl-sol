// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Test} from "forge-std/Test.sol";
import {Sha256Mmr} from "../src/lib/Sha256Mmr.sol";

contract MmrHarness {
    function check(bytes memory proof, bytes memory element, uint64 loc, bytes32 root) external view returns (uint8) {
        return Sha256Mmr.check(proof, element, loc, root);
    }

    function verify(bytes memory proof, bytes memory element, uint64 loc, bytes32 root) external view {
        Sha256Mmr.verifyInclusion(proof, element, loc, root);
    }

    function appendElement(bytes memory entry) external pure returns (bytes memory) {
        return Sha256Mmr.appendElement(entry);
    }

    function readVarint(bytes memory b) external pure returns (bool ok, uint64 v, uint256 next) {
        return Sha256Mmr.readVarint(b, 0);
    }
}

/// The Solidity MMR against vectors written by token-evm/vectors-gen from commonware-storage
/// 2026.9.0 itself (every case also checked there by crates/pearl-verify-sbf), plus the two
/// real devnet withdrawal proofs.
contract Sha256MmrTest is Test {
    MmrHarness h;

    function setUp() public {
        h = new MmrHarness();
    }

    function test_commonwareVectors() public view {
        string memory j = vm.readFile("test/vectors/mmr.json");
        uint256 n = vm.parseJsonUint(j, ".count");
        uint256[] memory locs = vm.parseJsonUintArray(j, ".loc");
        bool[] memory valid = vm.parseJsonBoolArray(j, ".valid");
        bytes32[] memory roots = vm.parseJsonBytes32Array(j, ".root");
        bytes[] memory proofs = vm.parseJsonBytesArray(j, ".proof");
        bytes[] memory els = vm.parseJsonBytesArray(j, ".element");
        assertEq(locs.length, n);
        uint256 good;
        for (uint256 i = 0; i < n; i++) {
            uint8 e = h.check(proofs[i], els[i], uint64(locs[i]), roots[i]);
            if (valid[i]) {
                assertEq(e, 0, string.concat("valid case refused: ", vm.toString(i)));
                good++;
            } else {
                assertTrue(e != 0, string.concat("invalid case accepted: ", vm.toString(i)));
            }
        }
        assertEq(good, 300);
    }

    function test_devnetWithdrawals() public view {
        string memory j = vm.readFile("test/vectors/devnet-withdrawals.json");
        for (uint256 i = 0; i < 2; i++) {
            string memory k = string.concat(".withdrawals[", vm.toString(i), "]");
            bytes memory entry = vm.parseJsonBytes(j, string.concat(k, ".entry"));
            bytes memory el = vm.parseJsonBytes(j, string.concat(k, ".element"));
            bytes memory proof = vm.parseJsonBytes(j, string.concat(k, ".proof"));
            bytes32 root = vm.parseJsonBytes32(j, string.concat(k, ".withdraw_root"));
            uint64 loc = uint64(vm.parseJsonUint(j, string.concat(k, ".loc")));
            assertEq(entry.length, 88);
            assertEq(h.appendElement(entry), el);
            h.verify(proof, el, loc, root);
            el[el.length - 1] ^= 0x01;
            assertEq(h.check(proof, el, loc, root), 3);
        }
    }

    function test_appendElementV2() public view {
        bytes memory e = new bytes(90);
        bytes memory el = h.appendElement(e);
        assertEq(el.length, 92);
        assertEq(uint8(el[0]), 0x01);
        assertEq(uint8(el[1]), 90);
        bytes memory big = new bytes(200);
        bytes memory el2 = h.appendElement(big);
        assertEq(uint8(el2[1]), 0xc8);
        assertEq(uint8(el2[2]), 0x01);
        assertEq(el2.length, 203);
    }

    function test_varintCanonical() public view {
        (bool ok, uint64 v,) = h.readVarint(hex"ac02");
        assertTrue(ok);
        assertEq(v, 300);
        (ok,,) = h.readVarint(hex"8000"); // non-canonical zero continuation
        assertFalse(ok);
        (ok,,) = h.readVarint(hex"80");
        assertFalse(ok);
        (ok, v,) = h.readVarint(hex"ffffffffffffffffff01");
        assertTrue(ok);
        assertEq(v, type(uint64).max);
        (ok,,) = h.readVarint(hex"ffffffffffffffffff02");
        assertFalse(ok);
    }

    function testFuzz_garbageNeverAccepted(bytes memory proof, bytes memory el, uint64 loc, bytes32 root) public view {
        assertTrue(h.check(proof, el, loc, root) != 0);
    }

    function test_revertsWithErrors() public {
        vm.expectRevert(Sha256Mmr.MmrFormat.selector);
        h.verify(hex"01", hex"00", 0, bytes32(0));
        // one leaf, zero inactive, zero digests: shape ok, root wrong
        vm.expectRevert(Sha256Mmr.MmrRoot.selector);
        h.verify(hex"010000", hex"00", 0, bytes32(uint256(1)));
        vm.expectRevert(Sha256Mmr.MmrProof.selector);
        h.verify(hex"010000", hex"00", 1, bytes32(uint256(1)));
    }

    /// A one-leaf MMR: root = sha256(1 BE || sha256(0 BE || element)).
    function test_singleLeafByHand() public view {
        bytes memory el = hex"0103aabbcc";
        bytes32 leaf = sha256(abi.encodePacked(uint64(0), el));
        bytes32 root = sha256(abi.encodePacked(uint64(1), leaf));
        assertEq(h.check(hex"010000", el, 0, root), 0);
    }
}
