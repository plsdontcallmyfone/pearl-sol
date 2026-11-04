// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {Ownable} from "solady/auth/Ownable.sol";
import {ReentrancyGuard} from "solady/utils/ReentrancyGuard.sol";
import {SafeTransferLib} from "solady/utils/SafeTransferLib.sol";
import {ICadreBridge, ICadreDesk} from "./interfaces/ICadre.sol";

/// Mining reserve on Robinhood Chain. Port of token/programs/reserve.
///
/// - Holds CADRE (its LP share and buybacks). The only way tokens leave is `fundMining`:
///   PERMISSIONLESS and rate limited (as on Solana, SPEC 2.2), it deposits reserve tokens
///   into the bridge with the chain `miningPool` as recipient. Limits: `maxPerCall`,
///   `minInterval` between calls, `maxPerWindow` per `windowSecs`, admin pause.
///   `miningPool` and `bridge` freeze at the first funding.
/// - There is no pump.fun creator fee on RH: ETH arrives as the desk's LP fees
///   (`CadreV4Desk.collectFees` pays this contract) and is spent by `crankBuyback`, which
///   buys CADRE into the reserve within `maxBuyPerCall`, `minBuy`, `buyInterval`, the
///   caller's minimum and `maxSlippageBps` against the desk quote. Cranker-gated when set.
/// - Every limit is owner-editable; the owner cannot move reserve tokens anywhere else.
contract CadreReserve is Ownable, ReentrancyGuard {
    using SafeTransferLib for address;

    /// PROVISIONAL defaults, as on Solana (2026-10-19): 10M tokens per call, one call per
    /// hour, 20M per 24 h.
    uint64 public constant DEFAULT_MAX_PER_CALL = 10_000_000_000_000;
    uint64 public constant DEFAULT_MIN_INTERVAL = 3_600;
    uint64 public constant DEFAULT_WINDOW_SECS = 86_400;
    uint64 public constant DEFAULT_MAX_PER_WINDOW = 20_000_000_000_000;

    struct FundLimits {
        bool paused;
        uint64 maxPerCall; // 0 = no cap
        uint64 minInterval;
        uint64 windowSecs; // 0 = no window cap
        uint64 maxPerWindow; // 0 = no window cap
    }

    struct BuyLimits {
        address cranker; // zero = anyone
        address desk;
        uint128 maxBuyPerCall; // 0 = no cap
        uint128 minBuy;
        uint64 buyInterval;
        uint16 maxSlippageBps;
    }

    address public immutable token;
    ICadreBridge public bridge;
    bytes32 public miningPool;

    FundLimits public fund;
    BuyLimits public buy;

    uint256 public totalFunded;
    uint64 public fundCalls;
    uint64 public fundLastTs;
    uint64 public windowStart;
    uint64 public windowUsed;

    uint256 public buySpent;
    uint256 public buyTokens;
    uint64 public buys;
    uint64 public lastBuyTs;

    event MiningFunded(
        uint64 indexed nonce,
        uint64 amount,
        bytes32 recipient,
        uint256 totalFunded,
        address caller,
        uint256 reserveLeft,
        uint64 windowStart,
        uint64 windowUsed
    );
    event FundLimitsUpdated(FundLimits limits);
    event BuyLimitsUpdated(BuyLimits limits);
    event DestinationUpdated(address bridge, bytes32 miningPool);
    event BuybackExecuted(uint64 indexed buy, uint256 ethIn, uint256 tokensOut, uint256 quote);

    error FundPaused();
    error ZeroAmount();
    error OverFundCap();
    error FundTooSoon();
    error OverWindowCap();
    error InsufficientReserve();
    error BadConfig();
    error DestinationFrozen();
    error TooSoon();
    error NothingToBuy();
    error Slippage();

    constructor(address owner_, address token_, ICadreBridge bridge_, bytes32 miningPool_, BuyLimits memory buy_) {
        _initializeOwner(owner_);
        token = token_;
        _setDestination(address(bridge_), miningPool_);
        fund = FundLimits(false, DEFAULT_MAX_PER_CALL, DEFAULT_MIN_INTERVAL, DEFAULT_WINDOW_SECS, DEFAULT_MAX_PER_WINDOW);
        emit FundLimitsUpdated(fund);
        _setBuy(buy_);
    }

    /// LP fees and any donation.
    receive() external payable {}

    // ---------------------------------------------------------------- admin

    function setFundLimits(FundLimits calldata l) external onlyOwner {
        fund = l;
        emit FundLimitsUpdated(l);
    }

    function setBuyLimits(BuyLimits calldata l) external onlyOwner {
        _setBuy(l);
    }

    function setDestination(address bridge_, bytes32 miningPool_) external onlyOwner {
        _setDestination(bridge_, miningPool_);
    }

    function _setDestination(address bridge_, bytes32 pool) internal {
        if (totalFunded != 0 && (bridge_ != address(bridge) || pool != miningPool)) revert DestinationFrozen();
        if (bridge_ == address(0) || pool == bytes32(0)) revert BadConfig();
        bridge = ICadreBridge(bridge_);
        miningPool = pool;
        emit DestinationUpdated(bridge_, pool);
    }

    function _setBuy(BuyLimits memory l) internal {
        if (l.maxSlippageBps > 10_000) revert BadConfig();
        buy = l;
        emit BuyLimitsUpdated(l);
    }

    // ---------------------------------------------------------------- fund_mining

    function reserveBalance() public view returns (uint256) {
        return SafeTransferLib.balanceOf(token, address(this));
    }

    /// Window use after funding `amount` now (reverts past the interval or the window cap).
    function _windowCheck(uint64 amount) internal view returns (uint64 used) {
        FundLimits memory f = fund;
        if (fundLastTs != 0 && block.timestamp < uint256(fundLastTs) + f.minInterval) revert FundTooSoon();
        if (f.windowSecs == 0 || f.maxPerWindow == 0) return 0;
        bool fresh = windowStart == 0 || block.timestamp >= uint256(windowStart) + f.windowSecs;
        uint256 next = uint256(fresh ? 0 : windowUsed) + amount;
        if (next > f.maxPerWindow) revert OverWindowCap();
        return uint64(next);
    }

    function fundMining(uint64 amount) external nonReentrant returns (uint64 nonce) {
        FundLimits memory f = fund;
        if (f.paused) revert FundPaused();
        if (amount == 0) revert ZeroAmount();
        if (f.maxPerCall != 0 && amount > f.maxPerCall) revert OverFundCap();
        uint64 used = _windowCheck(amount);
        if (reserveBalance() < amount) revert InsufficientReserve();
        token.safeApprove(address(bridge), amount);
        nonce = bridge.deposit(amount, miningPool);
        totalFunded += amount;
        if (f.windowSecs > 0 && (windowStart == 0 || block.timestamp >= uint256(windowStart) + f.windowSecs)) {
            windowStart = uint64(block.timestamp);
        }
        windowUsed = used;
        fundLastTs = uint64(block.timestamp);
        fundCalls++;
        emit MiningFunded(nonce, amount, miningPool, totalFunded, msg.sender, reserveBalance(), windowStart, used);
    }

    // ---------------------------------------------------------------- buyback

    /// Buys CADRE into the reserve with up to `maxEthIn` of the ETH held here.
    function crankBuyback(uint256 maxEthIn, uint256 minTokensOut) external nonReentrant returns (uint256 got) {
        BuyLimits memory b = buy;
        if (b.cranker != address(0) && msg.sender != b.cranker) revert Unauthorized();
        if (lastBuyTs != 0 && block.timestamp < uint256(lastBuyTs) + b.buyInterval) revert TooSoon();
        uint256 spend = address(this).balance;
        if (b.maxBuyPerCall != 0 && spend > b.maxBuyPerCall) spend = b.maxBuyPerCall;
        if (spend > maxEthIn) spend = maxEthIn;
        if (spend == 0 || spend < b.minBuy) revert NothingToBuy();
        ICadreDesk d = ICadreDesk(b.desk);
        uint256 quote = d.quoteExactIn(true, spend);
        uint256 floor_ = quote * (10_000 - b.maxSlippageBps) / 10_000;
        uint256 minOut = minTokensOut > floor_ ? minTokensOut : floor_;
        uint256 balBefore = address(this).balance;
        uint256 tokBefore = reserveBalance();
        d.buy{value: spend}(minOut == 0 ? 1 : minOut, address(this));
        uint256 spent = balBefore - address(this).balance;
        got = reserveBalance() - tokBefore;
        if (got == 0 || got < minTokensOut) revert Slippage();
        if (got * spend * 10_000 < quote * spent * (10_000 - b.maxSlippageBps)) revert Slippage();
        lastBuyTs = uint64(block.timestamp);
        buys++;
        buySpent += spent;
        buyTokens += got;
        emit BuybackExecuted(buys, spent, got, quote);
    }
}
