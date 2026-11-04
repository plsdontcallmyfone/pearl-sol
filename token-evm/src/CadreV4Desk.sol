// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Ownable} from "solady/auth/Ownable.sol";
import {SafeTransferLib} from "solady/utils/SafeTransferLib.sol";
import {IPoolManager} from "v4-core/interfaces/IPoolManager.sol";
import {IUnlockCallback} from "v4-core/interfaces/callback/IUnlockCallback.sol";
import {IHooks} from "v4-core/interfaces/IHooks.sol";
import {PoolKey} from "v4-core/types/PoolKey.sol";
import {PoolId, PoolIdLibrary} from "v4-core/types/PoolId.sol";
import {Currency} from "v4-core/types/Currency.sol";
import {BalanceDelta} from "v4-core/types/BalanceDelta.sol";
import {StateLibrary} from "v4-core/libraries/StateLibrary.sol";
import {TickMath} from "v4-core/libraries/TickMath.sol";
import {FullMath} from "v4-core/libraries/FullMath.sol";
import {FixedPoint96} from "v4-core/libraries/FixedPoint96.sol";
import {ICadreDesk} from "./interfaces/ICadre.sol";

/// Thin desk on the canonical Uniswap v4 PoolManager (RH testnet and mainnet
/// 0x8366a39cc670b4001a1121b8f6a443a643e40951) for the CADRE/ETH pool (currency0 = native
/// ETH, currency1 = CADRE, no hook). It replaces PumpSwap for the RH side: no
/// PositionManager, everything happens in `unlockCallback`.
///
/// - `initializePool` (owner) creates the pool at a start price.
/// - `addLiquidity` (owner) adds full-range liquidity owned by this desk. There is no
///   remove: the protocol LP is locked for good.
/// - `collectFees` (anyone) sends the position's accrued LP fees to `feeRecipient` (the
///   reserve, which spends the ETH on buybacks), fixed at deploy.
/// - `buy` / `sell` (anyone): exact-input swaps with a caller minimum; unspent input
///   (a fill stopped by the price limit) goes back to the caller.
/// - `quoteExactIn`: constant-product quote from the in-range liquidity and spot price
///   (exact for full-range-only liquidity without tick crossings; a floor otherwise), used
///   by the storefront and reserve for their slippage checks.
contract CadreV4Desk is ICadreDesk, IUnlockCallback, Ownable {
    using SafeTransferLib for address;
    using PoolIdLibrary for PoolKey;
    using StateLibrary for IPoolManager;

    uint8 internal constant ACT_SWAP = 1;
    uint8 internal constant ACT_ADD = 2;
    uint8 internal constant ACT_COLLECT = 3;

    IPoolManager public immutable manager;
    address public immutable token;
    address public immutable feeRecipient;
    uint24 public immutable fee;
    int24 public immutable tickSpacing;

    uint256 public totalFeesEth;
    uint256 public totalFeesToken;

    event PoolInitialized(bytes32 poolId, uint160 sqrtPriceX96, int24 tick);
    event LiquidityAdded(uint128 liquidity, uint256 eth, uint256 tokens);
    event FeesCollected(uint256 eth, uint256 tokens, address to);
    event Swapped(address indexed caller, bool zeroForOne, uint256 amountIn, uint256 amountOut, address to);

    error NotManager();
    error BadAmount();
    error MinOut();
    error Excess();

    constructor(address owner_, IPoolManager manager_, address token_, uint24 fee_, int24 tickSpacing_, address feeRecipient_) {
        _initializeOwner(owner_);
        manager = manager_;
        token = token_;
        fee = fee_;
        tickSpacing = tickSpacing_;
        feeRecipient = feeRecipient_;
    }

    receive() external payable {
        if (msg.sender != address(manager)) revert NotManager();
    }

    function poolKey() public view returns (PoolKey memory) {
        return PoolKey(Currency.wrap(address(0)), Currency.wrap(token), fee, tickSpacing, IHooks(address(0)));
    }

    function poolId() public view returns (bytes32) {
        return PoolId.unwrap(poolKey().toId());
    }

    function ticks() public view returns (int24 lower, int24 upper) {
        return (TickMath.minUsableTick(tickSpacing), TickMath.maxUsableTick(tickSpacing));
    }

    // ---------------------------------------------------------------- owner

    function initializePool(uint160 sqrtPriceX96) external onlyOwner returns (int24 tick) {
        tick = manager.initialize(poolKey(), sqrtPriceX96);
        emit PoolInitialized(poolId(), sqrtPriceX96, tick);
    }

    /// Adds `liquidity` full range. ETH comes from msg.value (excess returned), CADRE from
    /// the owner (allowance to this desk), at most `maxTokens`.
    function addLiquidity(uint128 liquidity, uint256 maxTokens) external payable onlyOwner returns (uint256 eth, uint256 tokens) {
        if (liquidity == 0) revert BadAmount();
        bytes memory r = manager.unlock(abi.encode(ACT_ADD, abi.encode(liquidity, msg.value, maxTokens, msg.sender)));
        (eth, tokens) = abi.decode(r, (uint256, uint256));
        if (msg.value > eth) msg.sender.safeTransferETH(msg.value - eth);
        emit LiquidityAdded(liquidity, eth, tokens);
    }

    // ---------------------------------------------------------------- anyone

    function collectFees() external returns (uint256 eth, uint256 tokens) {
        (eth, tokens) = abi.decode(manager.unlock(abi.encode(ACT_COLLECT, bytes(""))), (uint256, uint256));
        totalFeesEth += eth;
        totalFeesToken += tokens;
        emit FeesCollected(eth, tokens, feeRecipient);
    }

    /// ETH in (msg.value), CADRE out to `to`.
    function buy(uint256 minOut, address to) external payable returns (uint256 out) {
        out = _swap(true, msg.value, minOut, to);
    }

    /// CADRE in (pulled from the caller), ETH out to `to`.
    function sell(uint256 amountIn, uint256 minOut, address to) external returns (uint256 out) {
        token.safeTransferFrom(msg.sender, address(this), amountIn);
        out = _swap(false, amountIn, minOut, to);
    }

    function _swap(bool zeroForOne, uint256 amountIn, uint256 minOut, address to) internal returns (uint256 out) {
        if (amountIn == 0 || amountIn > uint256(type(int256).max)) revert BadAmount();
        bytes memory r = manager.unlock(abi.encode(ACT_SWAP, abi.encode(zeroForOne, amountIn, to)));
        uint256 spent;
        (spent, out) = abi.decode(r, (uint256, uint256));
        if (out < minOut || out == 0) revert MinOut();
        if (spent < amountIn) {
            uint256 back = amountIn - spent;
            if (zeroForOne) msg.sender.safeTransferETH(back);
            else token.safeTransfer(msg.sender, back);
        }
        emit Swapped(msg.sender, zeroForOne, spent, out, to);
    }

    // ---------------------------------------------------------------- v4 callback

    function unlockCallback(bytes calldata data) external returns (bytes memory) {
        if (msg.sender != address(manager)) revert NotManager();
        (uint8 act, bytes memory args) = abi.decode(data, (uint8, bytes));
        PoolKey memory key = poolKey();
        if (act == ACT_SWAP) {
            (bool zeroForOne, uint256 amountIn, address to) = abi.decode(args, (bool, uint256, address));
            BalanceDelta d = manager.swap(
                key,
                IPoolManager.SwapParams({
                    zeroForOne: zeroForOne,
                    amountSpecified: -int256(amountIn),
                    sqrtPriceLimitX96: zeroForOne ? TickMath.MIN_SQRT_PRICE + 1 : TickMath.MAX_SQRT_PRICE - 1
                }),
                ""
            );
            int128 a0 = d.amount0();
            int128 a1 = d.amount1();
            uint256 spent;
            uint256 out;
            if (zeroForOne) {
                spent = uint256(uint128(-a0));
                out = uint256(uint128(a1));
                _pay(key.currency0, spent);
                manager.take(key.currency1, to, out);
            } else {
                spent = uint256(uint128(-a1));
                out = uint256(uint128(a0));
                _pay(key.currency1, spent);
                manager.take(key.currency0, to, out);
            }
            return abi.encode(spent, out);
        }
        if (act == ACT_ADD) {
            (uint128 liquidity, uint256 maxEth, uint256 maxTokens, address payer) =
                abi.decode(args, (uint128, uint256, uint256, address));
            (int24 lo, int24 hi) = ticks();
            (BalanceDelta d,) = manager.modifyLiquidity(
                key, IPoolManager.ModifyLiquidityParams(lo, hi, int256(uint256(liquidity)), bytes32(0)), ""
            );
            uint256 eth = d.amount0() < 0 ? uint256(uint128(-d.amount0())) : 0;
            uint256 tokens = d.amount1() < 0 ? uint256(uint128(-d.amount1())) : 0;
            if (eth > maxEth || tokens > maxTokens) revert Excess();
            if (eth > 0) _pay(key.currency0, eth);
            if (tokens > 0) {
                manager.sync(key.currency1);
                token.safeTransferFrom(payer, address(manager), tokens);
                manager.settle();
            }
            return abi.encode(eth, tokens);
        }
        if (act == ACT_COLLECT) {
            (int24 lo, int24 hi) = ticks();
            (BalanceDelta d,) =
                manager.modifyLiquidity(key, IPoolManager.ModifyLiquidityParams(lo, hi, 0, bytes32(0)), "");
            uint256 eth = d.amount0() > 0 ? uint256(uint128(d.amount0())) : 0;
            uint256 tokens = d.amount1() > 0 ? uint256(uint128(d.amount1())) : 0;
            if (eth > 0) manager.take(key.currency0, feeRecipient, eth);
            if (tokens > 0) manager.take(key.currency1, feeRecipient, tokens);
            return abi.encode(eth, tokens);
        }
        revert BadAmount();
    }

    /// Pays `amount` of `c` the desk owes the manager (ETH it holds from msg.value, or
    /// CADRE it holds after pulling it from the seller).
    function _pay(Currency c, uint256 amount) internal {
        if (Currency.unwrap(c) == address(0)) {
            manager.settle{value: amount}();
        } else {
            manager.sync(c);
            token.safeTransfer(address(manager), amount);
            manager.settle();
        }
    }

    // ---------------------------------------------------------------- quotes

    function quoteExactIn(bool zeroForOne, uint256 amountIn) external view returns (uint256) {
        PoolId id = poolKey().toId();
        (uint160 sqrtP,,,) = manager.getSlot0(id);
        uint128 L = manager.getLiquidity(id);
        if (sqrtP == 0 || L == 0) return 0;
        uint256 x = FullMath.mulDiv(uint256(L), FixedPoint96.Q96, sqrtP); // ETH reserve
        uint256 y = FullMath.mulDiv(uint256(L), sqrtP, FixedPoint96.Q96); // CADRE reserve
        uint256 dx = amountIn * (1_000_000 - fee) / 1_000_000;
        return zeroForOne ? FullMath.mulDiv(y, dx, x + dx) : FullMath.mulDiv(x, dx, y + dx);
    }

    /// Full-range liquidity for `eth` and `tokens` at the current price (the smaller side
    /// binds), for the deploy script.
    function liquidityForAmounts(uint256 eth, uint256 tokens) external view returns (uint128) {
        (uint160 sqrtP,,,) = manager.getSlot0(poolKey().toId());
        (int24 lo, int24 hi) = ticks();
        uint160 sa = TickMath.getSqrtPriceAtTick(lo);
        uint160 sb = TickMath.getSqrtPriceAtTick(hi);
        uint256 l0 = FullMath.mulDiv(eth, FullMath.mulDiv(sqrtP, sb, FixedPoint96.Q96), sb - sqrtP);
        uint256 l1 = FullMath.mulDiv(tokens, FixedPoint96.Q96, sqrtP - sa);
        uint256 l = l0 < l1 ? l0 : l1;
        return l > type(uint128).max ? type(uint128).max : uint128(l);
    }
}
