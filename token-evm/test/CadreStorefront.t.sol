// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Fixture} from "./utils/Fixture.sol";
import {CadreStorefront} from "../src/CadreStorefront.sol";
import {CadreBridge} from "../src/CadreBridge.sol";
import {SignerSet} from "../src/lib/SignerSet.sol";
import {CadreFormats} from "../src/lib/CadreFormats.sol";
import {ICadreDesk} from "../src/interfaces/ICadre.sol";
import {CadreV4Desk} from "../src/CadreV4Desk.sol";
import {Ownable} from "solady/auth/Ownable.sol";

contract CadreStorefrontTest is Fixture {
    address buyer;
    bytes32 constant RECIPIENT = bytes32(uint256(0xbeef));

    function setUp() public {
        setUpStack();
        buyer = makeAddr("buyer");
        vm.deal(buyer, 100 ether);
    }

    function _buy(uint256 eth) internal returns (uint64 nonce) {
        vm.prank(buyer);
        nonce = store.buyJob{value: eth}(RECIPIENT, spec(), 0);
    }

    function _finalize(uint64 nonce) internal {
        CadreStorefront.Order memory o = store.getOrder(nonce);
        bytes[] memory s = sign(firstN(storeKeys, 2), store.resultDigest(o.jobId, bytes32(uint256(9)), bytes32(uint256(8))));
        vm.prank(buyer);
        store.finalizeOrder(nonce, bytes32(uint256(9)), bytes32(uint256(8)), "ipfs://r", s);
    }

    function _expire(uint64 nonce) internal {
        CadreStorefront.Order memory o = store.getOrder(nonce);
        bytes[] memory s = sign(firstN(storeKeys, 2), store.expiredDigest(o.jobId));
        vm.prank(buyer);
        store.expireOrder(nonce, s);
    }

    function _refund(uint64 nonce) internal {
        CadreBridge.Deposit memory d = bridge.deposits(nonce);
        bytes[] memory s = sign(firstN(signerKeys, 2), bridge.refundDigest(nonce, d.refundTo, d.amount));
        bridge.refundDeposit(nonce, s);
    }

    function test_buyJobPlacesOrderAndDeposit() public {
        uint256 bal = buyer.balance;
        vm.recordLogs();
        uint64 nonce = _buy(1 ether);
        assertEq(nonce, 0);
        CadreStorefront.Order memory o = store.getOrder(nonce);
        assertEq(o.buyer, buyer);
        assertEq(o.status, 1);
        assertEq(o.share, 0.1 ether);
        assertEq(o.ethSpent, 0.9 ether);
        assertEq(buyer.balance, bal - 1 ether);
        assertGt(o.tokens, 0);
        // 0.9 ETH at 1e6 CADRE/ETH on a 100 ETH pool, 0.3% fee: about 889k CADRE
        assertApproxEqRel(uint256(o.tokens), 889_000 * 1e6, 0.01e18);
        assertEq(o.workerPay + o.protocolFee, uint256(o.tokens) * 1000);
        assertEq(o.protocolFee, uint256(o.tokens) * 500 / 10_000 * 1000);
        assertEq(o.jobId, CadreFormats.orderJobId(2, 0));
        CadreBridge.Deposit memory d = bridge.deposits(0);
        assertEq(d.sender, address(store));
        assertEq(d.refundTo, buyer);
        assertEq(d.amount, o.tokens);
        assertEq(d.recipient, RECIPIENT);
        assertEq(token.balanceOf(address(bridge)), o.tokens);
        assertEq(token.balanceOf(address(store)), 0);
        assertEq(store.bbLocked(), 0.1 ether);
        assertEq(address(store).balance, 0.1 ether);
        // body event = body hash = the PostJob body of the spec with W and P
        bytes memory body = store.postJobBody(spec(), o.workerPay, o.protocolFee);
        assertEq(sha256(body), o.bodyHash);
        assertEq(
            store.depositOrderMessage(0),
            CadreFormats.depositOrderMessage(2, uint64(block.chainid), address(bridge), 0, RECIPIENT, o.tokens, o.bodyHash)
        );
    }

    function test_finalizeThenShareToBuyback() public {
        uint64 n = _buy(1 ether);
        _finalize(n);
        assertEq(store.getOrder(n).status, 2);
        store.settleShare(n);
        assertEq(store.bbLocked(), 0);
        assertEq(store.bbAccrued(), 0.1 ether);
        vm.expectRevert(CadreStorefront.AlreadySettled.selector);
        store.settleShare(n);
        uint256 rb = token.balanceOf(address(reserve));
        uint256 got = store.crankBuyback(type(uint256).max, 0);
        assertGt(got, 0);
        assertEq(token.balanceOf(address(reserve)) - rb, got);
        assertEq(address(store).balance, 0);
        assertEq(store.bbToReserve(), got);
        // interval
        vm.expectRevert(CadreStorefront.TooSoon.selector);
        store.crankBuyback(type(uint256).max, 0);
    }

    function test_buybackBurnsAboveCap() public {
        uint64 n = _buy(1 ether);
        _finalize(n);
        store.settleShare(n);
        uint256 rb = token.balanceOf(address(reserve));
        CadreStorefront.Buyback memory b;
        (b.bps, b.dest, b.reserve, b.cranker, b.maxPerCall, b.minBuy, b.minInterval) = store.buyback();
        vm.prank(admin);
        store.setBuyback(b, uint64(rb + 1000));
        uint256 supply = token.totalSupply();
        uint256 got = store.crankBuyback(type(uint256).max, 0);
        assertEq(token.balanceOf(address(reserve)), rb + 1000);
        assertEq(store.bbBurned(), got - 1000);
        assertEq(token.totalSupply(), supply - (got - 1000));
    }

    function test_heldSharesNotCrankable() public {
        _buy(1 ether);
        vm.expectRevert(CadreStorefront.NothingToBuy.selector);
        store.crankBuyback(type(uint256).max, 0);
    }

    function test_openCrankCannotThrottle() public {
        uint64 n = _buy(1 ether);
        _finalize(n);
        store.settleShare(n);
        vm.expectRevert(CadreStorefront.PartialSpend.selector);
        store.crankBuyback(0.01 ether, 0);
    }

    function test_expireRefundsShare() public {
        uint64 n = _buy(1 ether);
        _expire(n);
        uint256 bal = buyer.balance;
        store.settleShare(n);
        assertEq(buyer.balance, bal + 0.1 ether);
        assertEq(store.bbRefunded(), 0.1 ether);
        // an expired order may still be refunded over the bridge, once
        _refund(n);
        store.markRefunded(n);
        assertEq(store.getOrder(n).status, 4);
        vm.expectRevert(CadreStorefront.AlreadySettled.selector);
        store.settleShare(n);
    }

    function test_refundFlow() public {
        uint64 n = _buy(1 ether);
        uint64 tokens = store.getOrder(n).tokens;
        _refund(n);
        assertEq(token.balanceOf(buyer), tokens);
        assertEq(token.balanceOf(address(bridge)), 0);
        store.markRefunded(n);
        uint256 bal = buyer.balance;
        store.settleShare(n);
        assertEq(buyer.balance, bal + 0.1 ether);
        // refunded once
        CadreBridge.Deposit memory d = bridge.deposits(n);
        bytes[] memory s = sign(firstN(signerKeys, 2), bridge.refundDigest(n, d.refundTo, d.amount));
        vm.expectRevert(CadreBridge.AlreadyRefunded.selector);
        bridge.refundDeposit(n, s);
        // a refunded order cannot be finalized
        CadreStorefront.Order memory o = store.getOrder(n);
        bytes[] memory fs = sign(firstN(storeKeys, 2), store.resultDigest(o.jobId, 0, 0));
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.NotPending.selector);
        store.finalizeOrder(n, 0, 0, "", fs);
    }

    /// Refund vs finalize exclusivity: a finalized order's deposit can never be refunded.
    function test_finalizedNotRefundable() public {
        uint64 n = _buy(1 ether);
        _finalize(n);
        CadreBridge.Deposit memory d = bridge.deposits(n);
        bytes[] memory s = sign(firstN(signerKeys, 2), bridge.refundDigest(n, d.refundTo, d.amount));
        vm.expectRevert(CadreBridge.NotRefundable.selector);
        bridge.refundDeposit(n, s);
    }

    function test_plainDepositsNotRefundable() public {
        address u = makeAddr("u");
        vm.prank(admin);
        token.transfer(u, 5e6);
        vm.startPrank(u);
        token.approve(address(bridge), 5e6);
        uint64 n = bridge.deposit(5e6, RECIPIENT);
        vm.stopPrank();
        bytes[] memory s = sign(firstN(signerKeys, 2), bridge.refundDigest(n, u, 5e6));
        vm.expectRevert(CadreBridge.NotAnOrder.selector);
        bridge.refundDeposit(n, s);
        // and the reserve's mining deposits neither
        uint64 m = reserve.fundMining(1e6);
        bytes[] memory s2 = sign(firstN(signerKeys, 2), bridge.refundDigest(m, address(reserve), 1e6));
        vm.expectRevert(CadreBridge.NotAnOrder.selector);
        bridge.refundDeposit(m, s2);
    }

    function test_markRefundedNeedsBridgeRecord() public {
        uint64 n = _buy(1 ether);
        vm.expectRevert(CadreStorefront.NotRefunded.selector);
        store.markRefunded(n);
    }

    function test_finalizeCallerAndSigs() public {
        uint64 n = _buy(1 ether);
        CadreStorefront.Order memory o = store.getOrder(n);
        bytes[] memory s = sign(firstN(storeKeys, 2), store.resultDigest(o.jobId, bytes32(uint256(1)), 0));
        vm.prank(makeAddr("stranger"));
        vm.expectRevert(Ownable.Unauthorized.selector);
        store.finalizeOrder(n, bytes32(uint256(1)), 0, "", s);
        // bridge signers are not storefront attesters
        bytes[] memory w = sign(firstN(signerKeys, 2), store.resultDigest(o.jobId, bytes32(uint256(1)), 0));
        vm.prank(buyer);
        vm.expectRevert(SignerSet.BadSignature.selector);
        store.finalizeOrder(n, bytes32(uint256(1)), 0, "", w);
        // a different result than signed
        vm.prank(buyer);
        vm.expectRevert(SignerSet.BadSignature.selector);
        store.finalizeOrder(n, bytes32(uint256(2)), 0, "", s);
        // an attester may call
        vm.prank(vm.addr(storeKeys[2]));
        store.finalizeOrder(n, bytes32(uint256(1)), 0, "", s);
        // an expiry signature does not finalize and a closed order cannot expire
        bytes[] memory e = sign(firstN(storeKeys, 2), store.expiredDigest(o.jobId));
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.NotPending.selector);
        store.expireOrder(n, e);
    }

    function test_specChecks() public {
        CadreStorefront.JobSpec memory s = spec();
        s.m = 0;
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.BadSpec.selector);
        store.buyJob{value: 1 ether}(RECIPIENT, s, 0);
        s = spec();
        s.evaluators[1] = s.evaluators[0];
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.BadSpec.selector);
        store.buyJob{value: 1 ether}(RECIPIENT, s, 0);
        s = spec();
        s.threshold = 3;
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.BadSpec.selector);
        store.buyJob{value: 1 ether}(RECIPIENT, s, 0);
        s = spec();
        s.config = new bytes(51);
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.BadSpec.selector);
        store.buyJob{value: 1 ether}(RECIPIENT, s, 0);
        s = spec();
        s.dataUri = new bytes(201);
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.UriTooLong.selector);
        store.buyJob{value: 1 ether}(RECIPIENT, s, 0);
        s = spec();
        s.deadlineBlocks = 10_001;
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.BadSpec.selector);
        store.buyJob{value: 1 ether}(RECIPIENT, s, 0);
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.BadSpec.selector);
        store.buyJob{value: 1 ether}(bytes32(0), spec(), 0);
    }

    function test_orderLimitsAndPause() public {
        CadreStorefront.JobSpec memory s = spec();
        vm.startPrank(buyer);
        vm.expectRevert(CadreStorefront.BelowMinOrder.selector);
        store.buyJob{value: 0.00001 ether}(RECIPIENT, s, 0);
        vm.expectRevert(CadreStorefront.AboveMaxOrder.selector);
        store.buyJob{value: 11 ether}(RECIPIENT, s, 0);
        vm.expectRevert(CadreV4Desk.MinOut.selector);
        store.buyJob{value: 1 ether}(RECIPIENT, s, 10_000_000 * 1e6);
        vm.stopPrank();
        vm.prank(admin);
        store.setPaused(true);
        vm.prank(buyer);
        vm.expectRevert(CadreStorefront.IsPaused.selector);
        store.buyJob{value: 1 ether}(RECIPIENT, s, 0);
    }

    function test_deskFreezesAfterFirstOrder() public {
        vm.prank(admin);
        store.setDesk(ICadreDesk(address(desk)));
        _buy(1 ether);
        vm.prank(admin);
        vm.expectRevert(CadreStorefront.Frozen.selector);
        store.setDesk(ICadreDesk(address(1)));
    }

    function test_reserveDestinationFreezes() public {
        uint64 n = _buy(1 ether);
        _finalize(n);
        store.settleShare(n);
        store.crankBuyback(type(uint256).max, 0);
        CadreStorefront.Buyback memory b;
        (b.bps, b.dest, b.reserve, b.cranker, b.maxPerCall, b.minBuy, b.minInterval) = store.buyback();
        b.reserve = makeAddr("evil");
        vm.prank(admin);
        vm.expectRevert(CadreStorefront.Frozen.selector);
        store.setBuyback(b, 0);
    }

    function test_adminOnly() public {
        vm.expectRevert(Ownable.Unauthorized.selector);
        store.setPaused(true);
        CadreStorefront.Config memory c;
        vm.expectRevert(Ownable.Unauthorized.selector);
        store.setConfig(c);
    }

    function test_strangersCannotSendEth() public {
        vm.deal(address(this), 1 ether);
        (bool ok,) = address(store).call{value: 1}("");
        assertFalse(ok);
    }

    /// Accounting under many orders with random outcomes: the storefront holds exactly the
    /// unsettled shares plus accrued buyback ETH, and the bridge holds every unrefunded
    /// order's tokens.
    function testFuzz_ordersAccounting(uint8 count, uint256 seed) public {
        count = uint8(bound(count, 1, 12));
        uint256 locked;
        uint256 bridgeTokens;
        for (uint256 i = 0; i < count; i++) {
            uint256 eth = bound(uint256(keccak256(abi.encode(seed, i))), 0.001 ether, 2 ether);
            uint64 n = _buy(eth);
            CadreStorefront.Order memory o = store.getOrder(n);
            uint256 r = uint256(keccak256(abi.encode(seed, i, "o"))) % 4;
            bridgeTokens += o.tokens;
            if (r == 0) {
                locked += o.share;
            } else if (r == 1) {
                _finalize(n);
                store.settleShare(n);
            } else if (r == 2) {
                _expire(n);
                store.settleShare(n);
            } else {
                _refund(n);
                store.markRefunded(n);
                store.settleShare(n);
                bridgeTokens -= o.tokens;
            }
        }
        assertEq(store.bbLocked(), locked);
        assertEq(address(store).balance, locked + store.bbAccrued());
        assertEq(token.balanceOf(address(bridge)), bridgeTokens);
        assertEq(bridge.totalDeposited() - bridge.totalRefunded() - bridge.totalReleased(), bridgeTokens);
    }
}
