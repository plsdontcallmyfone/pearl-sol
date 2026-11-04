// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Fixture} from "./utils/Fixture.sol";
import {CadreReserve} from "../src/CadreReserve.sol";
import {CadreBridge} from "../src/CadreBridge.sol";
import {Ownable} from "solady/auth/Ownable.sol";

contract CadreReserveTest is Fixture {
    function setUp() public {
        setUpStack();
        vm.warp(1_800_000_000);
    }

    function _limits(bool paused, uint64 perCall, uint64 interval, uint64 window, uint64 perWindow) internal {
        vm.prank(admin);
        reserve.setFundLimits(CadreReserve.FundLimits(paused, perCall, interval, window, perWindow));
    }

    function test_fundMiningDepositsToPool() public {
        uint256 before = reserve.reserveBalance();
        address anyone = makeAddr("anyone");
        vm.prank(anyone);
        uint64 n = reserve.fundMining(5_000_000 * 1e6);
        CadreBridge.Deposit memory d = bridge.deposits(n);
        assertEq(d.sender, address(reserve));
        assertEq(d.recipient, miningPool);
        assertEq(d.amount, 5_000_000 * 1e6);
        assertEq(d.refundTo, address(0));
        assertEq(reserve.reserveBalance(), before - 5_000_000 * 1e6);
        assertEq(reserve.totalFunded(), 5_000_000 * 1e6);
        assertEq(reserve.fundCalls(), 1);
    }

    function test_defaultLimitsMatchSolana() public view {
        (bool p, uint64 perCall, uint64 interval, uint64 window, uint64 perWindow) = reserve.fund();
        assertFalse(p);
        assertEq(perCall, 10_000_000_000_000);
        assertEq(interval, 3_600);
        assertEq(window, 86_400);
        assertEq(perWindow, 20_000_000_000_000);
    }

    function test_perCallCap() public {
        vm.expectRevert(CadreReserve.OverFundCap.selector);
        reserve.fundMining(10_000_000 * 1e6 + 1);
        reserve.fundMining(10_000_000 * 1e6);
    }

    function test_intervalAndWindow() public {
        reserve.fundMining(10_000_000 * 1e6);
        vm.expectRevert(CadreReserve.FundTooSoon.selector);
        reserve.fundMining(1);
        vm.warp(block.timestamp + 3_600);
        reserve.fundMining(10_000_000 * 1e6);
        vm.warp(block.timestamp + 3_600);
        vm.expectRevert(CadreReserve.OverWindowCap.selector);
        reserve.fundMining(1);
        // a new window opens 24 h after the first funding
        vm.warp(1_800_000_000 + 86_400);
        reserve.fundMining(10_000_000 * 1e6);
        assertEq(reserve.windowUsed(), 10_000_000 * 1e6);
    }

    function test_pauseAndZeroAndInsufficient() public {
        vm.expectRevert(CadreReserve.ZeroAmount.selector);
        reserve.fundMining(0);
        _limits(true, 0, 0, 0, 0);
        vm.expectRevert(CadreReserve.FundPaused.selector);
        reserve.fundMining(1);
        _limits(false, 0, 0, 0, 0);
        uint256 bal = reserve.reserveBalance();
        vm.expectRevert(CadreReserve.InsufficientReserve.selector);
        reserve.fundMining(uint64(bal + 1));
    }

    function test_destinationFreezesAtFirstFunding() public {
        vm.prank(admin);
        reserve.setDestination(address(bridge), bytes32(uint256(0x1234)));
        assertEq(reserve.miningPool(), bytes32(uint256(0x1234)));
        reserve.fundMining(1e6);
        vm.prank(admin);
        vm.expectRevert(CadreReserve.DestinationFrozen.selector);
        reserve.setDestination(address(bridge), miningPool);
        vm.prank(admin);
        vm.expectRevert(CadreReserve.DestinationFrozen.selector);
        reserve.setDestination(makeAddr("other-bridge"), bytes32(uint256(0x1234)));
    }

    function test_adminOnly() public {
        vm.expectRevert(Ownable.Unauthorized.selector);
        reserve.setFundLimits(CadreReserve.FundLimits(true, 0, 0, 0, 0));
        vm.expectRevert(Ownable.Unauthorized.selector);
        reserve.setDestination(address(bridge), bytes32(uint256(1)));
    }

    /// LP fees from desk trades go to the reserve, which spends the ETH buying CADRE.
    function test_feesFundBuyback() public {
        address trader = makeAddr("trader");
        vm.deal(trader, 50 ether);
        vm.startPrank(trader);
        for (uint256 i = 0; i < 5; i++) {
            uint256 got = desk.buy{value: 5 ether}(0, trader);
            token.approve(address(desk), got);
            desk.sell(got / 2, 0, trader);
        }
        vm.stopPrank();
        (uint256 eth, uint256 toks) = desk.collectFees();
        assertGt(eth, 0);
        assertGt(toks, 0);
        assertEq(address(reserve).balance, eth);
        uint256 rb = reserve.reserveBalance();
        uint256 got2 = reserve.crankBuyback(type(uint256).max, 0);
        assertGt(got2, 0);
        assertEq(reserve.reserveBalance(), rb + got2);
        assertEq(address(reserve).balance, 0);
        vm.expectRevert(CadreReserve.NothingToBuy.selector);
        reserve.crankBuyback(type(uint256).max, 0);
    }

    function test_buybackLimits() public {
        vm.deal(address(reserve), 2 ether);
        vm.prank(admin);
        reserve.setBuyLimits(CadreReserve.BuyLimits(makeAddr("cranker"), address(desk), 1 ether, 0.1 ether, 600, 300));
        vm.expectRevert(Ownable.Unauthorized.selector);
        reserve.crankBuyback(type(uint256).max, 0);
        vm.startPrank(makeAddr("cranker"));
        reserve.crankBuyback(type(uint256).max, 0);
        assertEq(address(reserve).balance, 1 ether);
        vm.expectRevert(CadreReserve.TooSoon.selector);
        reserve.crankBuyback(type(uint256).max, 0);
        vm.warp(block.timestamp + 600);
        vm.expectRevert(CadreReserve.NothingToBuy.selector);
        reserve.crankBuyback(0.05 ether, 0);
        vm.expectRevert();
        reserve.crankBuyback(1 ether, 10_000_000_000 * 1e6);
        vm.stopPrank();
    }

    function testFuzz_windowNeverExceeded(uint64[8] memory amounts, uint32[8] memory gaps) public {
        _limits(false, 0, 60, 86_400, 20_000_000 * 1e6);
        uint256 windowStartTs;
        uint256 windowSum;
        for (uint256 i = 0; i < 8; i++) {
            vm.warp(block.timestamp + 60 + uint256(gaps[i]) % 40_000);
            uint64 a = uint64(bound(amounts[i], 1, 15_000_000 * 1e6));
            bool fresh = windowStartTs == 0 || block.timestamp >= windowStartTs + 86_400;
            uint256 used = fresh ? 0 : windowSum;
            if (used + a > 20_000_000 * 1e6) {
                vm.expectRevert(CadreReserve.OverWindowCap.selector);
                reserve.fundMining(a);
            } else {
                reserve.fundMining(a);
                if (fresh) {
                    windowStartTs = block.timestamp;
                    windowSum = a;
                } else {
                    windowSum += a;
                }
                assertLe(windowSum, 20_000_000 * 1e6);
            }
        }
    }
}
