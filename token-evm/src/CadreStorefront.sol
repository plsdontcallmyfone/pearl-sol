// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {EIP712} from "solady/utils/EIP712.sol";
import {Ownable} from "solady/auth/Ownable.sol";
import {ReentrancyGuard} from "solady/utils/ReentrancyGuard.sol";
import {SafeTransferLib} from "solady/utils/SafeTransferLib.sol";
import {SignerSet} from "./lib/SignerSet.sol";
import {CadreFormats} from "./lib/CadreFormats.sol";
import {ICadreBridge, ICadreDesk, ICadreOrderSource} from "./interfaces/ICadre.sol";

interface IBurnable {
    function burn(uint256 amount) external;
}

/// Compute storefront on Robinhood Chain. Port of token/programs/storefront (phase 1:
/// signer finalize only; finalize by proof waits for the phase 2 light client).
///
/// One transaction (no draft step, EVM calldata has room for the spec): `buyJob` takes ETH,
/// holds `buybackBps` of it as the order's buyback share, buys CADRE with the rest on the v4
/// desk (caller minimum plus `maxSlippageBps` against the desk's quote), splits the tokens
/// into W (worker pay) and P (protocol fee, `protocolFeeBps`), deposits all of them into the
/// bridge with `depositFor(recipient, buyer)` and records the Order under the bridge nonce.
/// `OrderBody` carries the exact PostJob body (FORMATS.md 4) so the relayer submits it and
/// the attesters check sha256(body) == bodyHash.
///
/// - finalizeOrder / expireOrder: M-of-N attesters over EIP-712 Result(jobId, deliverer,
///   resultHash) / Expired(jobId). Caller: buyer, owner or an attester.
/// - refundable (ICadreOrderSource) lets the bridge refund a Pending or Expired order to its
///   buyer; markRefunded (anyone) then records it from the bridge's refund record.
/// - settleShare (anyone, once the order is closed): Finalized releases the share to the
///   buyback; Refunded or Expired pays it back to the buyer. Exactly once.
/// - crankBuyback: buys CADRE with the released buyback ETH, sends it to the reserve while
///   the reserve holds less than `bbReserveCap` (0 = no cap) and burns the rest (or burns
///   everything with destination burn). Permissionless unless a cranker is set.
contract CadreStorefront is ICadreOrderSource, EIP712, Ownable, ReentrancyGuard, SignerSet {
    using SafeTransferLib for address;

    uint8 public constant STATUS_PENDING = 1;
    uint8 public constant STATUS_FINALIZED = 2;
    uint8 public constant STATUS_EXPIRED = 3;
    uint8 public constant STATUS_REFUNDED = 4;
    uint8 public constant DEST_RESERVE = 0;
    uint8 public constant DEST_BURN = 1;
    uint256 public constant MAX_EVALUATORS = 8;
    uint256 public constant MAX_URI = 200;
    uint256 public constant CONFIG_LEN = 52;

    bytes32 public constant RESULT_TYPEHASH = keccak256("Result(bytes32 jobId,bytes32 deliverer,bytes32 resultHash)");
    bytes32 public constant EXPIRED_TYPEHASH = keccak256("Expired(bytes32 jobId)");

    struct JobSpec {
        uint32 m;
        uint32 n;
        uint32 k;
        bytes config; // 52 bytes
        bytes32 hashA;
        bytes32 hashB;
        bytes dataUri; // <= 200 bytes
        uint64 deadlineBlocks; // relative to the unlock height
        uint32 maxBlocks;
        bytes32[] evaluators; // 1..8 distinct nonzero chain keys
        uint8 threshold;
    }

    struct Config {
        uint16 protocolFeeBps;
        uint16 maxSlippageBps;
        uint128 minEthIn;
        uint128 maxEthIn;
        uint32 maxDim;
        uint32 maxBlocksCap;
        uint64 maxDeadlineBlocks;
    }

    struct Buyback {
        uint16 bps;
        uint8 dest;
        address reserve;
        address cranker; // zero = anyone
        uint128 maxPerCall; // 0 = no cap
        uint128 minBuy;
        uint64 minInterval;
    }

    struct Order {
        address buyer;
        uint8 status;
        bool shareSettled;
        uint64 tokens;
        uint64 workerPay; // chain units (x1000)
        uint64 protocolFee; // chain units
        uint128 ethSpent;
        uint128 share;
        bytes32 recipient;
        bytes32 jobId;
        bytes32 bodyHash;
        bytes32 deliverer;
        bytes32 resultHash;
    }

    address public immutable token;
    ICadreBridge public immutable bridge;
    ICadreDesk public desk;

    Config public config;
    Buyback public buyback;
    uint64 public bbReserveCap;
    bool public paused;

    mapping(uint64 => Order) internal _orders;

    uint64 public orders;
    uint256 public totalEth;
    uint256 public totalTokens;
    uint256 public bbLocked;
    uint256 public bbAccrued;
    uint256 public bbRefunded;
    uint256 public bbSpent;
    uint256 public bbTokens;
    uint256 public bbToReserve;
    uint256 public bbBurned;
    uint64 public bbBuys;
    uint64 public bbLastTs;

    event OrderPlaced(
        uint64 indexed nonce,
        bytes32 indexed jobId,
        address indexed buyer,
        bytes32 recipient,
        uint256 ethSpent,
        uint64 tokens,
        uint64 workerPay,
        uint64 protocolFee,
        bytes32 bodyHash
    );
    event OrderBody(uint64 indexed nonce, bytes body);
    event OrderFinalized(uint64 indexed nonce, bytes32 indexed jobId, bytes32 deliverer, bytes32 resultHash, string resultUri);
    event OrderExpired(uint64 indexed nonce, bytes32 indexed jobId);
    event OrderRefunded(uint64 indexed nonce, address indexed buyer, uint64 tokens);
    event BuybackShareHeld(uint64 indexed nonce, uint256 amount, uint256 totalLocked);
    event BuybackAccrued(uint64 indexed nonce, uint256 amount, uint256 totalAccrued);
    event BuybackShareRefunded(uint64 indexed nonce, address indexed buyer, uint256 amount, uint8 status);
    event BuybackExecuted(uint64 indexed buy, uint256 ethIn, uint256 tokensOut, uint256 toReserve, uint256 burned, uint256 quote);
    event ConfigUpdated(Config config);
    event BuybackConfigUpdated(Buyback buyback, uint64 reserveCap);
    event DeskChanged(address desk);
    event PausedChanged(bool paused);

    error IsPaused();
    error BadConfig();
    error BadSpec();
    error BelowMinOrder();
    error AboveMaxOrder();
    error OrderTooSmall();
    error Slippage();
    error MinTokensOut();
    error NotPending();
    error NotRefunded();
    error AlreadySettled();
    error TooSoon();
    error NothingToBuy();
    error PartialSpend();
    error Frozen();
    error UriTooLong();
    error Overflow();
    error BadCaller();

    constructor(
        address owner_,
        address token_,
        ICadreBridge bridge_,
        ICadreDesk desk_,
        Config memory config_,
        Buyback memory buyback_,
        uint64 reserveCap_,
        address[] memory attesters,
        uint8 threshold_
    ) {
        _initializeOwner(owner_);
        token = token_;
        bridge = bridge_;
        desk = desk_;
        _setConfig(config_);
        _setBuyback(buyback_, reserveCap_);
        _setSigners(attesters, threshold_);
    }

    /// ETH comes back from the desk (unspent input) only.
    receive() external payable {
        if (msg.sender != address(desk)) revert BadCaller();
    }

    function _domainNameAndVersion() internal pure override returns (string memory, string memory) {
        return ("CadreStorefront", "1");
    }

    // ---------------------------------------------------------------- admin

    function setConfig(Config calldata c) external onlyOwner {
        _setConfig(c);
    }

    function setBuyback(Buyback calldata b, uint64 reserveCap_) external onlyOwner {
        _setBuyback(b, reserveCap_);
    }

    function setAttesters(address[] calldata a, uint8 t) external onlyOwner {
        _setSigners(a, t);
    }

    function setPaused(bool p) external onlyOwner {
        paused = p;
        emit PausedChanged(p);
    }

    /// The desk freezes at the first order (as the Solana mint and bridge do, S-2): a new
    /// desk would receive every later buyer's ETH.
    function setDesk(ICadreDesk d) external onlyOwner {
        if (orders != 0 && d != desk) revert Frozen();
        desk = d;
        emit DeskChanged(address(d));
    }

    function _setConfig(Config memory c) internal {
        if (c.protocolFeeBps == 0 || c.protocolFeeBps >= 10_000) revert BadConfig();
        if (c.maxSlippageBps > 10_000) revert BadConfig();
        if (c.minEthIn == 0 || c.minEthIn > c.maxEthIn) revert BadConfig();
        config = c;
        emit ConfigUpdated(c);
    }

    function _setBuyback(Buyback memory b, uint64 cap) internal {
        if (b.bps >= 10_000) revert BadConfig();
        if (b.dest != DEST_RESERVE && b.dest != DEST_BURN) revert BadConfig();
        if (b.dest == DEST_RESERVE && b.reserve == address(0)) revert BadConfig();
        // The reserve destination freezes once a buyback reached it (S-2).
        if (bbToReserve != 0 && b.reserve != buyback.reserve) revert Frozen();
        buyback = b;
        bbReserveCap = cap;
        emit BuybackConfigUpdated(b, cap);
    }

    // ---------------------------------------------------------------- views

    function getOrder(uint64 nonce) external view returns (Order memory) {
        return _orders[nonce];
    }

    function resultDigest(bytes32 jobId, bytes32 deliverer, bytes32 resultHash) public view returns (bytes32) {
        return _hashTypedData(keccak256(abi.encode(RESULT_TYPEHASH, jobId, deliverer, resultHash)));
    }

    function expiredDigest(bytes32 jobId) public view returns (bytes32) {
        return _hashTypedData(keccak256(abi.encode(EXPIRED_TYPEHASH, jobId)));
    }

    function domainSeparator() external view returns (bytes32) {
        return _domainSeparator();
    }

    /// Chain-side attester message for order deposit `nonce` (FORMATS.md 2.2).
    function depositOrderMessage(uint64 nonce) external view returns (bytes memory) {
        Order storage o = _orders[nonce];
        return CadreFormats.depositOrderMessage(
            uint64(block.chainid), address(bridge), nonce, o.recipient, o.tokens, o.bodyHash
        );
    }

    function refundable(uint64 nonce, address buyer, uint64 amount) external view returns (bool) {
        Order storage o = _orders[nonce];
        return (o.status == STATUS_PENDING || o.status == STATUS_EXPIRED) && o.buyer == buyer && o.tokens == amount;
    }

    /// The PostJob body (FORMATS.md 4, SPEC 4.1 field order, little endian).
    function postJobBody(JobSpec calldata s, uint64 workerPay, uint64 protocolFee) public pure returns (bytes memory) {
        bytes memory evs;
        for (uint256 i = 0; i < s.evaluators.length; i++) {
            evs = abi.encodePacked(evs, s.evaluators[i]);
        }
        bytes memory head = abi.encodePacked(
            CadreFormats.le32(s.m),
            CadreFormats.le32(s.n),
            CadreFormats.le32(s.k),
            s.config,
            s.hashA,
            s.hashB,
            CadreFormats.le16(uint16(s.dataUri.length)),
            s.dataUri
        );
        return abi.encodePacked(
            head,
            CadreFormats.le64(workerPay),
            CadreFormats.le64(protocolFee),
            CadreFormats.le64(s.deadlineBlocks),
            CadreFormats.le32(s.maxBlocks),
            uint8(s.evaluators.length),
            evs,
            s.threshold
        );
    }

    function checkSpec(JobSpec calldata s) public view {
        Config memory c = config;
        if (s.m == 0 || s.m > c.maxDim || s.n == 0 || s.n > c.maxDim || s.k == 0 || s.k > c.maxDim) revert BadSpec();
        if (s.config.length != CONFIG_LEN) revert BadSpec();
        if (s.maxBlocks == 0 || s.maxBlocks > c.maxBlocksCap) revert BadSpec();
        if (s.deadlineBlocks == 0 || s.deadlineBlocks > c.maxDeadlineBlocks) revert BadSpec();
        if (s.dataUri.length > MAX_URI) revert UriTooLong();
        uint256 n = s.evaluators.length;
        if (n == 0 || n > MAX_EVALUATORS) revert BadSpec();
        if (s.threshold == 0 || s.threshold > n) revert BadSpec();
        for (uint256 i = 0; i < n; i++) {
            if (s.evaluators[i] == bytes32(0)) revert BadSpec();
            for (uint256 j = 0; j < i; j++) {
                if (s.evaluators[j] == s.evaluators[i]) revert BadSpec();
            }
        }
    }

    // ---------------------------------------------------------------- orders

    /// Buys a job: msg.value ETH, the chain `recipient` (poster of the job), the spec and the
    /// caller's minimum CADRE out. Returns the bridge deposit nonce (= the order's key).
    function buyJob(bytes32 recipient, JobSpec calldata spec, uint256 minTokensOut)
        external
        payable
        nonReentrant
        returns (uint64 nonce)
    {
        if (paused) revert IsPaused();
        if (recipient == bytes32(0)) revert BadSpec();
        Config memory c = config;
        if (msg.value < c.minEthIn) revert BelowMinOrder();
        if (msg.value > c.maxEthIn) revert AboveMaxOrder();
        checkSpec(spec);

        uint256 share = msg.value * buyback.bps / 10_000;
        uint256 jobEth = msg.value - share;
        if (jobEth == 0) revert OrderTooSmall();
        (uint256 got, uint256 spent) = _buy(jobEth, minTokensOut, c.maxSlippageBps);
        if (spent < jobEth) msg.sender.safeTransferETH(jobEth - spent);
        if (got > type(uint64).max / CadreFormats.EXT_TO_CHAIN) revert Overflow();
        uint64 tokens = uint64(got);
        uint64 p = uint64(got * c.protocolFeeBps / 10_000);
        uint64 w = tokens - p;
        if (w == 0 || p == 0) revert OrderTooSmall();

        token.safeApprove(address(bridge), got);
        nonce = bridge.depositFor(tokens, recipient, msg.sender);
        _record(nonce, recipient, spec, tokens, w, p, spent, share);
    }

    function _record(
        uint64 nonce,
        bytes32 recipient,
        JobSpec calldata spec,
        uint64 tokens,
        uint64 w,
        uint64 p,
        uint256 spent,
        uint256 share
    ) internal {
        bytes memory body = postJobBody(spec, w * CadreFormats.EXT_TO_CHAIN, p * CadreFormats.EXT_TO_CHAIN);
        Order storage o = _orders[nonce];
        o.buyer = msg.sender;
        o.status = STATUS_PENDING;
        o.tokens = tokens;
        o.workerPay = w * CadreFormats.EXT_TO_CHAIN;
        o.protocolFee = p * CadreFormats.EXT_TO_CHAIN;
        o.ethSpent = uint128(spent);
        o.share = uint128(share);
        o.recipient = recipient;
        o.jobId = CadreFormats.orderJobId(uint64(block.chainid), address(bridge), nonce);
        o.bodyHash = sha256(body);
        orders++;
        totalEth += spent;
        totalTokens += tokens;
        if (share > 0) {
            bbLocked += share;
            emit BuybackShareHeld(nonce, share, bbLocked);
        }
        emit OrderPlaced(nonce, o.jobId, msg.sender, recipient, spent, tokens, o.workerPay, o.protocolFee, o.bodyHash);
        emit OrderBody(nonce, body);
    }

    /// Spends up to `eth` on the desk; returns (tokens got, ETH spent) after the fill check
    /// (the reserve's: caller minimum, and the realised price no worse than `bps` from the
    /// desk's pre-trade quote).
    function _buy(uint256 eth, uint256 minTokensOut, uint16 bps) internal returns (uint256 got, uint256 spent) {
        ICadreDesk d = desk;
        uint256 quote = d.quoteExactIn(true, eth);
        uint256 floor_ = quote * (10_000 - bps) / 10_000;
        uint256 minOut = minTokensOut > floor_ ? minTokensOut : floor_;
        if (minOut == 0) minOut = 1;
        uint256 balBefore = address(this).balance;
        uint256 tokBefore = _tokenBalance();
        d.buy{value: eth}(minOut, address(this));
        spent = balBefore - address(this).balance;
        got = _tokenBalance() - tokBefore;
        if (got < minTokensOut || got == 0) revert MinTokensOut();
        if (got * eth * 10_000 < quote * spent * (10_000 - bps)) revert Slippage();
    }

    function _tokenBalance() internal view returns (uint256) {
        return SafeTransferLib.balanceOf(token, address(this));
    }

    function finalizeOrder(
        uint64 nonce,
        bytes32 deliverer,
        bytes32 resultHash,
        string calldata resultUri,
        bytes[] calldata sigs
    ) external {
        if (bytes(resultUri).length > MAX_URI) revert UriTooLong();
        Order storage o = _orders[nonce];
        _closeCheck(o);
        _checkSigs(resultDigest(o.jobId, deliverer, resultHash), sigs);
        o.status = STATUS_FINALIZED;
        o.deliverer = deliverer;
        o.resultHash = resultHash;
        emit OrderFinalized(nonce, o.jobId, deliverer, resultHash, resultUri);
    }

    function expireOrder(uint64 nonce, bytes[] calldata sigs) external {
        Order storage o = _orders[nonce];
        _closeCheck(o);
        _checkSigs(expiredDigest(o.jobId), sigs);
        o.status = STATUS_EXPIRED;
        emit OrderExpired(nonce, o.jobId);
    }

    function _closeCheck(Order storage o) internal view {
        if (o.status != STATUS_PENDING) revert NotPending();
        if (msg.sender != o.buyer && msg.sender != owner() && !isSigner[msg.sender]) revert Unauthorized();
    }

    /// Records a refund the bridge paid (bridge refundDeposit) for a Pending or Expired order.
    function markRefunded(uint64 nonce) external {
        Order storage o = _orders[nonce];
        if (o.status != STATUS_PENDING && o.status != STATUS_EXPIRED) revert NotPending();
        (bool refunded, address to, uint64 amount) = bridge.isRefunded(nonce);
        if (!refunded || to != o.buyer || amount != o.tokens) revert NotRefunded();
        o.status = STATUS_REFUNDED;
        emit OrderRefunded(nonce, o.buyer, o.tokens);
    }

    function settleShare(uint64 nonce) external nonReentrant {
        Order storage o = _orders[nonce];
        if (o.shareSettled) revert AlreadySettled();
        uint8 st = o.status;
        uint256 share = o.share;
        if (st == STATUS_FINALIZED) {
            o.shareSettled = true;
            bbLocked -= share;
            bbAccrued += share;
            emit BuybackAccrued(nonce, share, bbAccrued);
        } else if (st == STATUS_REFUNDED || st == STATUS_EXPIRED) {
            o.shareSettled = true;
            bbLocked -= share;
            bbRefunded += share;
            if (share > 0) o.buyer.safeTransferETH(share);
            emit BuybackShareRefunded(nonce, o.buyer, share, st);
        } else {
            revert NotPending();
        }
    }

    // ---------------------------------------------------------------- buyback

    /// ETH the buyback may spend now (held shares excluded).
    function buybackAvailable() public view returns (uint256) {
        uint256 bal = address(this).balance;
        return bal > bbLocked ? bal - bbLocked : 0;
    }

    function crankBuyback(uint256 maxEthIn, uint256 minTokensOut) external nonReentrant returns (uint256 got) {
        Buyback memory b = buyback;
        if (b.cranker != address(0) && msg.sender != b.cranker) revert Unauthorized();
        if (bbLastTs != 0 && block.timestamp < uint256(bbLastTs) + b.minInterval) revert TooSoon();
        uint256 full = buybackAvailable();
        if (b.maxPerCall != 0 && full > b.maxPerCall) full = b.maxPerCall;
        uint256 spend = full < maxEthIn ? full : maxEthIn;
        if (spend == 0 || spend < b.minBuy) revert NothingToBuy();
        // S-3: an open crank cannot throttle the buyback; only a set cranker spends less.
        if (spend != full && b.cranker == address(0)) revert PartialSpend();
        uint256 quote = desk.quoteExactIn(true, spend);
        uint256 spent;
        (got, spent) = _buy(spend, minTokensOut, config.maxSlippageBps);
        uint256 toReserve;
        if (b.dest == DEST_RESERVE) {
            uint256 rb = SafeTransferLib.balanceOf(token, b.reserve);
            uint256 cap = bbReserveCap;
            toReserve = cap == 0 ? got : (rb >= cap ? 0 : (got < cap - rb ? got : cap - rb));
            if (toReserve > 0) token.safeTransfer(b.reserve, toReserve);
        }
        uint256 toBurn = got - toReserve;
        if (toBurn > 0) IBurnable(token).burn(toBurn);
        bbLastTs = uint64(block.timestamp);
        bbBuys++;
        bbSpent += spent;
        bbTokens += got;
        bbToReserve += toReserve;
        bbBurned += toBurn;
        emit BuybackExecuted(bbBuys, spent, got, toReserve, toBurn, quote);
    }
}
