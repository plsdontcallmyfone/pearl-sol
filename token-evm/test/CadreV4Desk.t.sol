// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Fixture} from "./utils/Fixture.sol";
import {CadreV4Desk} from "../src/CadreV4Desk.sol";
import {Ownable} from "solady/auth/Ownable.sol";

contract CadreV4DeskTest is Fixture {
    address trader;

    function setUp() public {
        setUpStack();
        trader = makeAddr("trader");
        vm.deal(trader, 100 ether);
    }

    function test_poolSeeded() public view {
        uint256 q = desk.quoteExactIn(true, 1 ether);
        // ~1,000,000 CADRE per ETH less the 0.3% fee and the 1% impact of 1 ETH on 100 ETH
        assertApproxEqRel(q, 987_000 * 1e6, 0.01e18);
        assertApproxEqRel(address(manager).balance, 100 ether, 0.001e18);
    }

    function test_buyMatchesQuote() public {
        uint256 q = desk.quoteExactIn(true, 1 ether);
        vm.prank(trader);
        uint256 got = desk.buy{value: 1 ether}(0, trader);
        assertEq(token.balanceOf(trader), got);
        // full-range-only liquidity: the cp quote is the fill up to rounding
        assertApproxEqRel(got, q, 0.0001e18);
    }

    function test_sellMatchesQuote() public {
        vm.startPrank(trader);
        uint256 got = desk.buy{value: 2 ether}(0, trader);
        uint256 q = desk.quoteExactIn(false, got / 2);
        token.approve(address(desk), got);
        uint256 bal = trader.balance;
        uint256 eth = desk.sell(got / 2, 0, trader);
        vm.stopPrank();
        assertEq(trader.balance, bal + eth);
        assertApproxEqRel(eth, q, 0.0001e18);
    }

    function test_minOut() public {
        uint256 q = desk.quoteExactIn(true, 1 ether);
        vm.prank(trader);
        vm.expectRevert(CadreV4Desk.MinOut.selector);
        desk.buy{value: 1 ether}(q * 2, trader);
        vm.prank(trader);
        vm.expectRevert(CadreV4Desk.BadAmount.selector);
        desk.buy{value: 0}(0, trader);
    }

    function test_onlyManagerCallback() public {
        vm.expectRevert(CadreV4Desk.NotManager.selector);
        desk.unlockCallback(abi.encode(uint8(1), abi.encode(true, uint256(1), address(this))));
        vm.deal(address(this), 1 ether);
        (bool ok,) = address(desk).call{value: 1}("");
        assertFalse(ok);
    }

    function test_ownerOnlyLiquidity() public {
        vm.prank(trader);
        vm.expectRevert(Ownable.Unauthorized.selector);
        desk.addLiquidity{value: 1 ether}(1e12, 1e12);
        vm.prank(trader);
        vm.expectRevert(Ownable.Unauthorized.selector);
        desk.initializePool(1 << 96);
    }

    function test_feesGoToFixedRecipient() public {
        vm.prank(trader);
        desk.buy{value: 10 ether}(0, trader);
        vm.prank(trader);
        (uint256 eth,) = desk.collectFees();
        // 0.3% of 10 ETH, the desk is the only LP
        assertApproxEqRel(eth, 0.03 ether, 0.001e18);
        assertEq(address(reserve).balance, eth);
        assertEq(trader.balance, 90 ether);
    }

    function test_addLiquidityRefundsExtraEth() public {
        vm.startPrank(admin);
        token.approve(address(desk), 10_000_000 * 1e6);
        uint128 liq = desk.liquidityForAmounts(1 ether, 10_000_000 * 1e6);
        uint256 bal = admin.balance;
        (uint256 eth, uint256 toks) = desk.addLiquidity{value: 5 ether}(liq, 10_000_000 * 1e6);
        vm.stopPrank();
        assertApproxEqRel(eth, 1 ether, 0.001e18);
        assertLe(toks, 10_000_000 * 1e6);
        assertEq(admin.balance, bal - eth);
    }

    function testFuzz_roundTripLoses(uint256 ethIn) public {
        ethIn = bound(ethIn, 1e9, 20 ether);
        vm.startPrank(trader);
        uint256 got = desk.buy{value: ethIn}(0, trader);
        token.approve(address(desk), got);
        uint256 back = desk.sell(got, 0, trader);
        vm.stopPrank();
        assertLt(back, ethIn);
        assertEq(token.balanceOf(trader), 0);
    }
}
