// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Fixture} from "./utils/Fixture.sol";
import {CadreBridge} from "../src/CadreBridge.sol";
import {SignerSet} from "../src/lib/SignerSet.sol";
import {Sha256Mmr} from "../src/lib/Sha256Mmr.sol";
import {CadreFormats} from "../src/lib/CadreFormats.sol";
import {Ownable} from "solady/auth/Ownable.sol";

contract CadreBridgeTest is Fixture {
    address alice;
    address bob;

    function setUp() public {
        setUpStack();
        alice = makeAddr("alice");
        bob = makeAddr("bob");
        vm.prank(admin);
        token.transfer(alice, 10_000_000 * 1e6);
        vm.prank(alice);
        token.approve(address(bridge), type(uint256).max);
    }

    function _deposit(uint64 amount) internal returns (uint64) {
        vm.prank(alice);
        return bridge.deposit(amount, bytes32(uint256(0xabc)));
    }

    function _relSigs(uint64 nonce, address to, uint64 amount) internal view returns (bytes[] memory) {
        return sign(firstN(signerKeys, 2), bridge.releaseDigest(nonce, to, amount));
    }

    function _entry(uint8 domain, uint64 nonce, address to, uint64 amount) internal pure returns (bytes memory) {
        return CadreFormats.encodeEntry(
            CadreFormats.Entry(domain, nonce, bytes32(uint256(7)), CadreFormats.toBytes32(to), amount, 42)
        );
    }

    // ---------------------------------------------------------------- deposits

    function test_depositRecordsAndLocks() public {
        uint64 n0 = _deposit(5e6);
        uint64 n1 = _deposit(7e6);
        assertEq(n0, 0);
        assertEq(n1, 1);
        assertEq(bridge.depositNonce(), 2);
        assertEq(token.balanceOf(address(bridge)), 12e6);
        CadreBridge.Deposit memory d = bridge.deposits(1);
        assertEq(d.sender, alice);
        assertEq(d.amount, 7e6);
        assertEq(d.recipient, bytes32(uint256(0xabc)));
        assertEq(d.refundTo, address(0));
        assertEq(bridge.totalDeposited(), 12e6);
    }

    function test_depositRefusals() public {
        vm.startPrank(alice);
        vm.expectRevert(CadreBridge.ZeroAmount.selector);
        bridge.deposit(0, bytes32(uint256(1)));
        vm.expectRevert(CadreBridge.BadRecipient.selector);
        bridge.deposit(1, bytes32(0));
        vm.expectRevert(CadreBridge.AmountTooLarge.selector);
        bridge.deposit(type(uint64).max / 1000 + 1, bytes32(uint256(1)));
        // only the order source may name a refund address
        vm.expectRevert(CadreBridge.NotAnOrder.selector);
        bridge.depositFor(1, bytes32(uint256(1)), alice);
        vm.stopPrank();
    }

    function test_depositMessageMatchesFormat() public {
        uint64 n = _deposit(2e6);
        assertEq(
            bridge.depositMessage(n),
            abi.encodePacked(
                "cadre:deposit:v2", uint8(2), CadreFormats.le64(uint64(block.chainid)), address(bridge),
                CadreFormats.le64(n), bytes32(uint256(0xabc)), CadreFormats.le64(2e6)
            )
        );
    }

    // ---------------------------------------------------------------- signer release

    function test_releaseWithThreshold() public {
        _deposit(10e6);
        bridge.release(3, bob, 4e6, _relSigs(3, bob, 4e6));
        assertEq(token.balanceOf(bob), 4e6);
        assertTrue(bridge.isProcessed(3));
        assertEq(bridge.totalReleased(), 4e6);
    }

    function test_releaseAllThreeSigners() public {
        _deposit(10e6);
        bridge.release(0, bob, 1e6, sign(signerKeys, bridge.releaseDigest(0, bob, 1e6)));
        assertEq(token.balanceOf(bob), 1e6);
    }

    function test_releaseNoDoubleRelease() public {
        _deposit(10e6);
        bytes[] memory s = _relSigs(1, bob, 1e6);
        bridge.release(1, bob, 1e6, s);
        vm.expectRevert(CadreBridge.AlreadyProcessed.selector);
        bridge.release(1, bob, 1e6, s);
        // a different amount or recipient for the same nonce is also refused
        bytes[] memory s2 = _relSigs(1, alice, 2e6);
        vm.expectRevert(CadreBridge.AlreadyProcessed.selector);
        bridge.release(1, alice, 2e6, s2);
    }

    function test_releaseBelowThreshold() public {
        _deposit(10e6);
        bytes[] memory one = sign(firstN(signerKeys, 1), bridge.releaseDigest(0, bob, 1e6));
        vm.expectRevert(SignerSet.NotEnoughSignatures.selector);
        bridge.release(0, bob, 1e6, one);
    }

    function test_releaseDuplicateSignatureRefused() public {
        _deposit(10e6);
        bytes[] memory one = sign(firstN(signerKeys, 1), bridge.releaseDigest(0, bob, 1e6));
        bytes[] memory dup = new bytes[](2);
        dup[0] = one[0];
        dup[1] = one[0];
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, bob, 1e6, dup);
    }

    function test_releaseUnsortedRefused() public {
        _deposit(10e6);
        bytes[] memory s = _relSigs(0, bob, 1e6);
        (s[0], s[1]) = (s[1], s[0]);
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, bob, 1e6, s);
    }

    function test_releaseOutsiderRefused() public {
        _deposit(10e6);
        (, uint256 k) = makeAddrAndKey("outsider");
        uint256[] memory ks = new uint256[](2);
        ks[0] = signerKeys[0];
        ks[1] = k;
        bytes[] memory s = sign(ks, bridge.releaseDigest(0, bob, 1e6));
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, bob, 1e6, s);
    }

    function test_releaseTamperedFieldsRefused() public {
        _deposit(10e6);
        bytes[] memory s = _relSigs(0, bob, 1e6);
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, bob, 2e6, s);
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, alice, 1e6, s);
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(1, bob, 1e6, s);
    }

    /// A release signed for another bridge (other address or chain) is refused: the EIP-712
    /// domain binds chainId and verifyingContract.
    function test_replayAcrossDomainsRefused() public {
        _deposit(10e6);
        address[] memory sa = new address[](3);
        for (uint256 i = 0; i < 3; i++) sa[i] = vm.addr(signerKeys[i]);
        CadreBridge other = new CadreBridge(admin, address(token), 2, sa, 2, 0);
        bytes[] memory forOther = sign(firstN(signerKeys, 2), other.releaseDigest(0, bob, 1e6));
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, bob, 1e6, forOther);

        bytes32 here = bridge.releaseDigest(0, bob, 1e6);
        vm.chainId(4663);
        assertTrue(bridge.releaseDigest(0, bob, 1e6) != here);
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, bob, 1e6, sign(firstN(signerKeys, 2), here));
    }

    /// A refund signature cannot be used as a release (different type hash) and vice versa.
    function test_refundSigIsNotAReleaseSig() public {
        _deposit(10e6);
        bytes[] memory s = sign(firstN(signerKeys, 2), bridge.refundDigest(0, bob, 1e6));
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, bob, 1e6, s);
    }

    function test_releaseModeDisabled() public {
        _deposit(10e6);
        vm.prank(admin);
        bridge.setReleaseModes(2, address(oracle));
        bytes[] memory s = _relSigs(0, bob, 1e6);
        vm.expectRevert(CadreBridge.ModeDisabled.selector);
        bridge.release(0, bob, 1e6, s);
    }

    function test_pauseStopsEverything() public {
        _deposit(10e6);
        vm.prank(admin);
        bridge.setPaused(true);
        bytes[] memory s = _relSigs(0, bob, 1e6);
        vm.expectRevert(CadreBridge.Paused.selector);
        bridge.release(0, bob, 1e6, s);
        vm.prank(alice);
        vm.expectRevert(CadreBridge.Paused.selector);
        bridge.deposit(1, bytes32(uint256(1)));
        vm.prank(admin);
        bridge.setPaused(false);
        bridge.release(0, bob, 1e6, s);
    }

    function test_releaseCannotExceedVault() public {
        _deposit(1e6);
        bytes[] memory s = _relSigs(0, bob, 2e6);
        vm.expectRevert();
        bridge.release(0, bob, 2e6, s);
    }

    function test_minWithdrawalNonce() public {
        address[] memory sa = new address[](3);
        for (uint256 i = 0; i < 3; i++) sa[i] = vm.addr(signerKeys[i]);
        CadreBridge b2 = new CadreBridge(admin, address(token), 2, sa, 2, 100);
        vm.prank(admin);
        token.transfer(address(b2), 10e6);
        bytes[] memory s = sign(firstN(signerKeys, 2), b2.releaseDigest(99, bob, 1e6));
        vm.expectRevert(CadreBridge.NonceTooLow.selector);
        b2.release(99, bob, 1e6, s);
        b2.release(100, bob, 1e6, sign(firstN(signerKeys, 2), b2.releaseDigest(100, bob, 1e6)));
    }

    // ---------------------------------------------------------------- proof release

    function test_releaseProofOneLeaf() public {
        _deposit(10e6);
        bytes memory e = _entry(2, 0, bob, 3e6);
        (bytes memory proof, bytes32 root) = oneLeaf(Sha256Mmr.appendElement(e));
        bytes32 h = postRoot(root, 10);
        bridge.releaseProof(e, 0, proof, h);
        assertEq(token.balanceOf(bob), 3e6);
        assertTrue(bridge.isProcessed(0));
        vm.expectRevert(CadreBridge.AlreadyProcessed.selector);
        bridge.releaseProof(e, 0, proof, h);
    }

    /// Proof and signer paths share the processed bitmap: one pays, the other is refused.
    function test_proofAndSignerExclusive() public {
        _deposit(10e6);
        bytes memory e = _entry(2, 5, bob, 3e6);
        (bytes memory proof, bytes32 root) = oneLeaf(Sha256Mmr.appendElement(e));
        bytes32 h = postRoot(root, 10);
        bridge.release(5, bob, 3e6, _relSigs(5, bob, 3e6));
        vm.expectRevert(CadreBridge.AlreadyProcessed.selector);
        bridge.releaseProof(e, 0, proof, h);

        bytes memory e2 = _entry(2, 6, bob, 1e6);
        (bytes memory p2, bytes32 r2) = oneLeaf(Sha256Mmr.appendElement(e2));
        bytes32 h2 = postRoot(r2, 11);
        bridge.releaseProof(e2, 0, p2, h2);
        bytes[] memory s = _relSigs(6, bob, 1e6);
        vm.expectRevert(CadreBridge.AlreadyProcessed.selector);
        bridge.release(6, bob, 1e6, s);
    }

    /// Only domain 2 entries: a Solana-domain (1) entry under a final root is refused.
    function test_releaseProofWrongDomain() public {
        _deposit(10e6);
        for (uint8 d = 0; d < 4; d++) {
            if (d == 2) continue;
            bytes memory e = _entry(d, 0, bob, 1e6);
            (bytes memory proof, bytes32 root) = oneLeaf(Sha256Mmr.appendElement(e));
            bytes32 h = postRoot(root, 20 + d);
            vm.expectRevert(CadreBridge.WrongDomain.selector);
            bridge.releaseProof(e, 0, proof, h);
        }
    }

    /// The domain is per deployment: a bridge on domain 3 refuses domain 2 entries and pays
    /// its own; domains outside 2..=15 cannot be configured.
    function test_domainIsPerDeployment() public {
        address[] memory sa = new address[](3);
        for (uint256 i = 0; i < 3; i++) sa[i] = vm.addr(signerKeys[i]);
        vm.expectRevert(CadreBridge.BadDomain.selector);
        new CadreBridge(admin, address(token), 1, sa, 2, 0);
        vm.expectRevert(CadreBridge.BadDomain.selector);
        new CadreBridge(admin, address(token), 16, sa, 2, 0);
        vm.startPrank(admin);
        CadreBridge b3 = new CadreBridge(admin, address(token), 3, sa, 2, 0);
        b3.setReleaseModes(2, address(oracle));
        token.transfer(address(b3), 10e6);
        vm.stopPrank();
        bytes memory e2 = _entry(2, 0, bob, 1e6);
        (bytes memory p2, bytes32 r2) = oneLeaf(Sha256Mmr.appendElement(e2));
        bytes32 h2 = postRoot(r2, 60);
        vm.expectRevert(CadreBridge.WrongDomain.selector);
        b3.releaseProof(e2, 0, p2, h2);
        bytes memory e3 = _entry(3, 0, bob, 1e6);
        (bytes memory p3, bytes32 r3) = oneLeaf(Sha256Mmr.appendElement(e3));
        bytes32 h3 = postRoot(r3, 61);
        b3.releaseProof(e3, 0, p3, h3);
        assertEq(token.balanceOf(bob), 1e6);
        assertEq(uint8(b3.depositMessage(0)[16]), 3);
    }

    function test_releaseProofBadVersionOrLength() public {
        _deposit(10e6);
        bytes memory e = _entry(2, 0, bob, 1e6);
        e[0] = 0x01;
        (bytes memory proof, bytes32 root) = oneLeaf(Sha256Mmr.appendElement(e));
        bytes32 h = postRoot(root, 30);
        vm.expectRevert(CadreFormats.BadEntry.selector);
        bridge.releaseProof(e, 0, proof, h);
        // a v1 (88-byte) entry is refused by length
        bytes memory v1 = new bytes(88);
        (bytes memory p1, bytes32 r1) = oneLeaf(Sha256Mmr.appendElement(v1));
        bytes32 h1 = postRoot(r1, 31);
        vm.expectRevert(CadreFormats.BadEntry.selector);
        bridge.releaseProof(v1, 0, p1, h1);
    }

    function test_releaseProofNonEvmRecipientRefused() public {
        _deposit(10e6);
        bytes memory e = CadreFormats.encodeEntry(
            CadreFormats.Entry(2, 0, bytes32(0), bytes32(uint256(1) << 200), 1e6, 1)
        );
        (bytes memory proof, bytes32 root) = oneLeaf(Sha256Mmr.appendElement(e));
        bytes32 h = postRoot(root, 40);
        vm.expectRevert(CadreBridge.BadRecipient.selector);
        bridge.releaseProof(e, 0, proof, h);
    }

    function test_releaseProofNotFinalOrWrongRoot() public {
        _deposit(10e6);
        bytes memory e = _entry(2, 0, bob, 1e6);
        (bytes memory proof, bytes32 root) = oneLeaf(Sha256Mmr.appendElement(e));
        vm.expectRevert(CadreBridge.NotFinal.selector);
        bridge.releaseProof(e, 0, proof, keccak256("unknown"));
        bytes32 h = postRoot(bytes32(uint256(root) ^ 1), 50);
        vm.expectRevert(Sha256Mmr.MmrRoot.selector);
        bridge.releaseProof(e, 0, proof, h);
    }

    function test_releaseProofModeOff() public {
        vm.prank(admin);
        bridge.setReleaseModes(1, address(0));
        bytes memory e = _entry(2, 0, bob, 1e6);
        vm.expectRevert(CadreBridge.ModeDisabled.selector);
        bridge.releaseProof(e, 0, hex"010000", bytes32(0));
    }

    /// Real multi-leaf commonware proofs: the generated v2 entries carry domain 2.
    function test_releaseProofCommonwareVector() public {
        vm.prank(admin);
        token.transfer(address(bridge), 100e6);
        string memory j = vm.readFile("test/vectors/mmr.json");
        bytes[] memory proofs = vm.parseJsonBytesArray(j, ".proof");
        bytes[] memory els = vm.parseJsonBytesArray(j, ".element");
        bytes32[] memory roots = vm.parseJsonBytes32Array(j, ".root");
        uint256[] memory locs = vm.parseJsonUintArray(j, ".loc");
        bool[] memory valid = vm.parseJsonBoolArray(j, ".valid");
        uint256 paid;
        for (uint256 i = 0; i < valid.length && paid < 12; i++) {
            if (!valid[i]) continue;
            if (_payVector(els[i], proofs[i], roots[i], uint64(locs[i]), uint64(1000 + i))) paid++;
        }
        assertEq(paid, 12);
    }

    function _payVector(bytes memory el, bytes memory proof, bytes32 root, uint64 loc, uint64 height)
        internal
        returns (bool)
    {
        bytes memory e = new bytes(el.length - 2);
        for (uint256 k = 0; k < e.length; k++) e[k] = el[k + 2];
        CadreFormats.Entry memory w = CadreFormats.parseEntry(e);
        if (bridge.isProcessed(w.nonce)) return false;
        bytes32 h = postRoot(root, height);
        address to = CadreFormats.toAddress(w.recipient);
        uint256 before = token.balanceOf(to);
        bridge.releaseProof(e, loc, proof, h);
        assertEq(token.balanceOf(to) - before, w.amount);
        return true;
    }

    // ---------------------------------------------------------------- admin

    function test_adminOnly() public {
        address[] memory s = new address[](1);
        s[0] = alice;
        vm.expectRevert(Ownable.Unauthorized.selector);
        bridge.setSigners(s, 1);
        vm.expectRevert(Ownable.Unauthorized.selector);
        bridge.setReleaseModes(1, address(0));
        vm.expectRevert(Ownable.Unauthorized.selector);
        bridge.setPaused(true);
        vm.expectRevert(Ownable.Unauthorized.selector);
        bridge.setOrderSource(alice);
    }

    function test_badModesAndSignerSets() public {
        vm.startPrank(admin);
        vm.expectRevert(CadreBridge.BadModes.selector);
        bridge.setReleaseModes(0, address(oracle));
        vm.expectRevert(CadreBridge.BadModes.selector);
        bridge.setReleaseModes(4, address(oracle));
        vm.expectRevert(CadreBridge.BadModes.selector);
        bridge.setReleaseModes(2, address(0));
        address[] memory s = new address[](2);
        s[0] = alice;
        s[1] = alice;
        vm.expectRevert(SignerSet.BadSignerSet.selector);
        bridge.setSigners(s, 1);
        s[1] = bob;
        vm.expectRevert(SignerSet.BadThreshold.selector);
        bridge.setSigners(s, 3);
        vm.expectRevert(SignerSet.BadThreshold.selector);
        bridge.setSigners(s, 0);
        address[] memory eleven = new address[](11);
        for (uint256 i = 0; i < 11; i++) eleven[i] = address(uint160(i + 1));
        vm.expectRevert(SignerSet.BadSignerSet.selector);
        bridge.setSigners(eleven, 1);
        vm.stopPrank();
    }

    /// After a signer-set change the old signers no longer count.
    function test_signerRotation() public {
        _deposit(10e6);
        (uint256[] memory nk, address[] memory na) = _keys("new-signer", 2);
        vm.prank(admin);
        bridge.setSigners(na, 2);
        bytes[] memory old = _relSigs(0, bob, 1e6);
        vm.expectRevert(SignerSet.BadSignature.selector);
        bridge.release(0, bob, 1e6, old);
        bridge.release(0, bob, 1e6, sign(nk, bridge.releaseDigest(0, bob, 1e6)));
    }

    function test_orderSourceFreezes() public {
        // the storefront is the order source; after its first order deposit it is frozen
        vm.prank(address(store));
        token.approve(address(bridge), 1);
        vm.prank(admin);
        token.transfer(address(store), 1);
        vm.prank(address(store));
        bridge.depositFor(1, bytes32(uint256(1)), alice);
        vm.prank(admin);
        vm.expectRevert(CadreBridge.OrderSourceFrozen.selector);
        bridge.setOrderSource(alice);
    }

    function testFuzz_releaseAnyNonce(uint64 nonce, uint64 amount) public {
        amount = uint64(bound(amount, 1, 10_000_000 * 1e6));
        _deposit(uint64(10_000_000 * 1e6));
        bridge.release(nonce, bob, amount, _relSigs(nonce, bob, amount));
        assertTrue(bridge.isProcessed(nonce));
        assertEq(token.balanceOf(bob), amount);
        if (nonce > 0) assertFalse(bridge.isProcessed(nonce - 1));
        if (nonce < type(uint64).max) assertFalse(bridge.isProcessed(nonce + 1));
    }
}
