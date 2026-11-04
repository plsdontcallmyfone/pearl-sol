// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Fixture} from "./utils/Fixture.sol";
import {CadreHeaderOracle} from "../src/CadreHeaderOracle.sol";
import {SignerSet} from "../src/lib/SignerSet.sol";
import {CadreFormats} from "../src/lib/CadreFormats.sol";
import {Sha256Mmr} from "../src/lib/Sha256Mmr.sol";
import {Ownable} from "solady/auth/Ownable.sol";

contract CadreHeaderOracleTest is Fixture {
    function setUp() public {
        setUpStack();
    }

    function _sigs(bytes32 h, uint64 height, bytes32 tx_, bytes32 st, bytes32 wr) internal view returns (bytes[] memory) {
        return sign(firstN(attesterKeys, 2), oracle.finalHeaderDigest(h, height, tx_, st, wr));
    }

    /// The real devnet final header: roots read from the 256 bytes match the fixture, and a
    /// real devnet withdrawal proves against it through the bridge's MMR check.
    struct Dev {
        bytes header;
        bytes32 h;
        uint64 height;
        bytes32 tx_;
        bytes32 st;
        bytes32 wr;
    }

    function _dev() internal view returns (Dev memory d) {
        string memory j = vm.readFile("test/vectors/devnet-withdrawals.json");
        d.header = vm.parseJsonBytes(j, ".withdrawals[0].header");
        d.h = vm.parseJsonBytes32(j, ".withdrawals[0].block_hash");
        d.height = uint64(vm.parseJsonUint(j, ".withdrawals[0].height"));
        d.tx_ = vm.parseJsonBytes32(j, ".withdrawals[0].tx_root");
        d.st = vm.parseJsonBytes32(j, ".withdrawals[0].state_root");
        d.wr = vm.parseJsonBytes32(j, ".withdrawals[0].withdraw_root");
    }

    function test_postRealDevnetHeader() public {
        Dev memory d = _dev();
        oracle.postFinalHeader(d.header, d.h, _sigs(d.h, d.height, d.tx_, d.st, d.wr));
        assertTrue(oracle.isFinal(d.h));
        (uint64 gh, bytes32 gt, bytes32 gs, bytes32 gw) = oracle.roots(d.h);
        assertEq(gh, d.height);
        assertEq(gt, d.tx_);
        assertEq(gs, d.st);
        assertEq(gw, d.wr);
        (uint64 lh, bytes32 lb) = oracle.latestFinal();
        assertEq(lh, d.height);
        assertEq(lb, d.h);
        string memory j = vm.readFile("test/vectors/devnet-withdrawals.json");
        assertEq(
            Sha256Mmr.check(
                vm.parseJsonBytes(j, ".withdrawals[0].proof"),
                vm.parseJsonBytes(j, ".withdrawals[0].element"),
                uint64(vm.parseJsonUint(j, ".withdrawals[0].loc")),
                gw
            ),
            0
        );
    }

    function test_headerBytesMustMatchSignedRoots() public {
        bytes memory header = new bytes(256);
        header[4] = 0x07; // height 7
        bytes32 h = keccak256("h");
        // signed with a different withdraw root than the header carries
        bytes[] memory s = _sigs(h, 7, 0, 0, bytes32(uint256(1)));
        vm.expectRevert(SignerSet.BadSignature.selector);
        oracle.postFinalHeader(header, h, s);
        vm.expectRevert(CadreHeaderOracle.BadHeader.selector);
        oracle.postFinalHeader(new bytes(255), h, s);
    }

    function test_thresholdAndOutsiders() public {
        bytes32 h = keccak256("x");
        bytes[] memory one = sign(firstN(attesterKeys, 1), oracle.finalHeaderDigest(h, 1, 0, 0, 0));
        vm.expectRevert(SignerSet.NotEnoughSignatures.selector);
        oracle.postFinal(h, 1, 0, 0, 0, one);
        // bridge signers are not oracle attesters
        bytes[] memory wrong = sign(firstN(signerKeys, 2), oracle.finalHeaderDigest(h, 1, 0, 0, 0));
        vm.expectRevert(SignerSet.BadSignature.selector);
        oracle.postFinal(h, 1, 0, 0, 0, wrong);
    }

    function test_conflictAndRepost() public {
        bytes32 a = keccak256("a");
        bytes32 b = keccak256("b");
        oracle.postFinal(a, 9, 0, 0, 0, _sigs(a, 9, 0, 0, 0));
        bytes[] memory sa = _sigs(a, 9, 0, 0, 0);
        vm.expectRevert(CadreHeaderOracle.AlreadyPosted.selector);
        oracle.postFinal(a, 9, 0, 0, 0, sa);
        bytes[] memory sb = _sigs(b, 9, 0, 0, 0);
        vm.expectRevert(abi.encodeWithSelector(CadreHeaderOracle.ConflictingHeader.selector, uint64(9), a));
        oracle.postFinal(b, 9, 0, 0, 0, sb);
    }

    function test_latestOnlyMovesUp() public {
        bytes32 a = keccak256("a");
        bytes32 b = keccak256("b");
        oracle.postFinal(a, 20, 0, 0, 0, _sigs(a, 20, 0, 0, 0));
        oracle.postFinal(b, 10, 0, 0, 0, _sigs(b, 10, 0, 0, 0));
        (uint64 lh, bytes32 lb) = oracle.latestFinal();
        assertEq(lh, 20);
        assertEq(lb, a);
        assertTrue(oracle.isFinal(b));
    }

    /// Attestations are bound to the Cadre chain id and to this oracle (EIP-712 domain).
    function test_replayAcrossChainsRefused() public {
        address[] memory aa = new address[](3);
        for (uint256 i = 0; i < 3; i++) aa[i] = vm.addr(attesterKeys[i]);
        CadreHeaderOracle otherChain = new CadreHeaderOracle(admin, CADRE_CHAIN_ID + 1, aa, 2);
        bytes32 h = keccak256("h");
        bytes[] memory forOther = sign(firstN(attesterKeys, 2), otherChain.finalHeaderDigest(h, 1, 0, 0, 0));
        vm.expectRevert(SignerSet.BadSignature.selector);
        oracle.postFinal(h, 1, 0, 0, 0, forOther);
        CadreHeaderOracle twin = new CadreHeaderOracle(admin, CADRE_CHAIN_ID, aa, 2);
        bytes[] memory forTwin = sign(firstN(attesterKeys, 2), twin.finalHeaderDigest(h, 1, 0, 0, 0));
        vm.expectRevert(SignerSet.BadSignature.selector);
        oracle.postFinal(h, 1, 0, 0, 0, forTwin);
    }

    function test_onlyOwnerSetsAttesters() public {
        address[] memory a = new address[](1);
        a[0] = address(1);
        vm.expectRevert(Ownable.Unauthorized.selector);
        oracle.setAttesters(a, 1);
        vm.prank(admin);
        oracle.setAttesters(a, 1);
        assertTrue(oracle.isSigner(address(1)));
        assertFalse(oracle.isSigner(vm.addr(attesterKeys[0])));
    }

    function testFuzz_unknownHashNotFinal(bytes32 h) public view {
        assertFalse(oracle.isFinal(h));
    }
}
