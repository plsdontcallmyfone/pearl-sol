// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

import {EIP712} from "solady/utils/EIP712.sol";
import {Ownable} from "solady/auth/Ownable.sol";
import {SafeTransferLib} from "solady/utils/SafeTransferLib.sol";
import {SignerSet} from "./lib/SignerSet.sol";
import {Sha256Mmr} from "./lib/Sha256Mmr.sol";
import {CadreFormats} from "./lib/CadreFormats.sol";
import {ICadreHeaders, ICadreOrderSource, ICadreBridge} from "./interfaces/ICadre.sol";

/// Robinhood Chain side of the Cadre bridge (domain 2). Port of token/programs/bridge.
///
/// - `deposit` / `depositFor` lock CADRE in this contract (lock/unlock, nothing minted)
///   under a dense nonce from 0. The relayer unlocks `amount x 1000` chain units on the
///   Cadre chain with a BridgeUnlockV2 (domain 2), FORMATS.md 2.
/// - `release` pays a Cadre withdrawal (BridgeLockV2, domain 2) once M-of-N secp256k1
///   signers signed the EIP-712 `Release(nonce, recipient, amount)`.
/// - `releaseProof` pays the same withdrawal from a Commonware MMR inclusion proof of its
///   v2 withdrawal entry (version 0x02, domain 2 only) against `withdrawRoot` of a final
///   header in `headers` (ICadreHeaders). Same processed bitmap: paid once either way.
/// - `refundDeposit` returns an order deposit (made by the order source, the storefront)
///   to its buyer while the order is Pending or Expired, with M-of-N signatures over
///   EIP-712 `Refund(nonce, buyer, amount)`. At most once per deposit. The chain cannot see
///   RH, so "never both unlocked and refunded" rests on the signers signing at most one of
///   the two per nonce (as on Solana, SPEC 5.1).
/// - Admin (owner, two-step handover): signer set, release modes + header source, pause,
///   the order source (frozen after its first order deposit).
contract CadreBridge is ICadreBridge, EIP712, Ownable, SignerSet {
    using SafeTransferLib for address;

    uint8 public constant MODE_SIGNERS = 1;
    uint8 public constant MODE_PROOF = 2;
    uint8 public constant DOMAIN = CadreFormats.DOMAIN_RH;
    uint64 internal constant MAX_AMOUNT = type(uint64).max / 1000;

    bytes32 public constant RELEASE_TYPEHASH = keccak256("Release(uint64 nonce,address recipient,uint64 amount)");
    bytes32 public constant REFUND_TYPEHASH = keccak256("Refund(uint64 nonce,address buyer,uint64 amount)");

    struct Deposit {
        address sender;
        uint64 amount;
        address refundTo; // nonzero only for order deposits (order source)
        uint64 blockNumber;
        bytes32 recipient;
        bool refunded;
    }

    address public immutable token;
    /// Withdrawal nonces below this are refused (set to the chain's next domain-2 withdrawal
    /// nonce when a bridge is redeployed, so an older bridge's withdrawals cannot replay).
    uint64 public immutable minWithdrawalNonce;

    ICadreHeaders public headers;
    uint8 public releaseModes;
    bool public paused;
    address public orderSource;
    bool public orderSourceFrozen;

    uint64 public depositNonce;
    mapping(uint64 => Deposit) internal _deposits;
    mapping(uint256 => uint256) internal _processed;

    uint256 public totalDeposited;
    uint256 public totalReleased;
    uint256 public totalRefunded;

    event Deposited(
        uint64 indexed nonce, address indexed sender, bytes32 indexed cadreRecipient, uint64 amount, address refundTo
    );
    event Released(uint64 indexed nonce, address indexed recipient, uint64 amount, bool viaProof);
    event Refunded(uint64 indexed nonce, address indexed buyer, uint64 amount);
    event ReleaseModesChanged(uint8 modes, address headers);
    event PausedChanged(bool paused);
    event OrderSourceChanged(address orderSource);

    error Paused();
    error ZeroAmount();
    error AmountTooLarge();
    error BadRecipient();
    error ModeDisabled();
    error BadModes();
    error AlreadyProcessed();
    error NonceTooLow();
    error NotFinal();
    error WrongDomain();
    error NotAnOrder();
    error AlreadyRefunded();
    error NotRefundable();
    error OrderSourceFrozen();

    constructor(
        address owner_,
        address token_,
        address[] memory signers_,
        uint8 threshold_,
        uint64 minWithdrawalNonce_
    ) {
        _initializeOwner(owner_);
        token = token_;
        minWithdrawalNonce = minWithdrawalNonce_;
        _setSigners(signers_, threshold_);
        releaseModes = MODE_SIGNERS;
    }

    function _domainNameAndVersion() internal pure override returns (string memory, string memory) {
        return ("CadreBridge", "2");
    }

    // ---------------------------------------------------------------- admin

    function setSigners(address[] calldata s, uint8 t) external onlyOwner {
        _setSigners(s, t);
    }

    function setReleaseModes(uint8 modes, address headers_) external onlyOwner {
        if (modes == 0 || modes & ~(MODE_SIGNERS | MODE_PROOF) != 0) revert BadModes();
        if (modes & MODE_PROOF != 0 && headers_ == address(0)) revert BadModes();
        releaseModes = modes;
        headers = ICadreHeaders(headers_);
        emit ReleaseModesChanged(modes, headers_);
    }

    function setPaused(bool p) external onlyOwner {
        paused = p;
        emit PausedChanged(p);
    }

    function setOrderSource(address s) external onlyOwner {
        if (orderSourceFrozen && s != orderSource) revert OrderSourceFrozen();
        orderSource = s;
        emit OrderSourceChanged(s);
    }

    // ---------------------------------------------------------------- views

    function domainSeparator() external view returns (bytes32) {
        return _domainSeparator();
    }

    function deposits(uint64 nonce) external view returns (Deposit memory) {
        return _deposits[nonce];
    }

    function isProcessed(uint64 nonce) public view returns (bool) {
        return _processed[nonce >> 8] & (uint256(1) << (nonce & 0xff)) != 0;
    }

    function isRefunded(uint64 nonce) external view returns (bool, address, uint64) {
        Deposit storage d = _deposits[nonce];
        return (d.refunded, d.refundTo, d.amount);
    }

    function releaseDigest(uint64 nonce, address recipient, uint64 amount) public view returns (bytes32) {
        return _hashTypedData(keccak256(abi.encode(RELEASE_TYPEHASH, nonce, recipient, amount)));
    }

    function refundDigest(uint64 nonce, address buyer, uint64 amount) public view returns (bytes32) {
        return _hashTypedData(keccak256(abi.encode(REFUND_TYPEHASH, nonce, buyer, amount)));
    }

    /// The chain-side message the attesters sign (ed25519) for deposit `nonce`
    /// (FORMATS.md 2.1 / 2.2). Order deposits need the PostJob body hash, so they are
    /// built by the storefront's `depositOrderMessage`.
    function depositMessage(uint64 nonce) external view returns (bytes memory) {
        Deposit storage d = _deposits[nonce];
        return CadreFormats.depositMessage(uint64(block.chainid), address(this), nonce, d.recipient, d.amount);
    }

    // ---------------------------------------------------------------- deposits

    /// Locks `amount` CADRE base units from the caller for `cadreRecipient` on the chain.
    function deposit(uint64 amount, bytes32 cadreRecipient) external returns (uint64) {
        return _deposit(amount, cadreRecipient, address(0));
    }

    /// As `deposit`; the order source (storefront) also names the order's buyer, the only
    /// address a refund can ever pay. Anyone else must pass refundTo = 0.
    function depositFor(uint64 amount, bytes32 cadreRecipient, address refundTo) external returns (uint64) {
        if (refundTo != address(0)) {
            if (msg.sender != orderSource || orderSource == address(0)) revert NotAnOrder();
            if (!orderSourceFrozen) orderSourceFrozen = true;
        }
        return _deposit(amount, cadreRecipient, refundTo);
    }

    function _deposit(uint64 amount, bytes32 recipient, address refundTo) internal returns (uint64 nonce) {
        if (paused) revert Paused();
        if (amount == 0) revert ZeroAmount();
        if (amount > MAX_AMOUNT) revert AmountTooLarge();
        if (recipient == bytes32(0)) revert BadRecipient();
        token.safeTransferFrom(msg.sender, address(this), amount);
        nonce = depositNonce++;
        _deposits[nonce] = Deposit(msg.sender, amount, refundTo, uint64(block.number), recipient, false);
        totalDeposited += amount;
        emit Deposited(nonce, msg.sender, recipient, amount, refundTo);
    }

    // ---------------------------------------------------------------- releases

    /// Signer mode: pays Cadre withdrawal `nonce` (domain 2) to `recipient`.
    function release(uint64 nonce, address recipient, uint64 amount, bytes[] calldata sigs) external {
        if (releaseModes & MODE_SIGNERS == 0) revert ModeDisabled();
        _checkSigs(releaseDigest(nonce, recipient, amount), sigs);
        _pay(nonce, recipient, amount, false);
    }

    /// Proof mode: `entry` is the 90-byte v2 withdrawal entry, `location` its leaf index in
    /// the withdrawal log, `proof` the codec-encoded MMR proof (node RPC
    /// getWithdrawalProof), `headerHash` a final header whose withdrawRoot covers it.
    /// Anyone may submit; funds go to the entry's recipient only.
    function releaseProof(bytes calldata entry, uint64 location, bytes calldata proof, bytes32 headerHash) external {
        if (releaseModes & MODE_PROOF == 0) revert ModeDisabled();
        ICadreHeaders h = headers;
        if (!h.isFinal(headerHash)) revert NotFinal();
        (,,, bytes32 withdrawRoot) = h.roots(headerHash);
        bytes memory e = entry;
        CadreFormats.Entry memory w = CadreFormats.parseEntry(e);
        if (w.domain != DOMAIN) revert WrongDomain();
        address recipient = CadreFormats.toAddress(w.recipient);
        Sha256Mmr.verifyInclusion(proof, Sha256Mmr.appendElement(e), location, withdrawRoot);
        _pay(w.nonce, recipient, w.amount, true);
    }

    function _pay(uint64 nonce, address recipient, uint64 amount, bool viaProof) internal {
        if (paused) revert Paused();
        if (amount == 0) revert ZeroAmount();
        if (recipient == address(0)) revert BadRecipient();
        if (nonce < minWithdrawalNonce) revert NonceTooLow();
        uint256 word = nonce >> 8;
        uint256 bit = uint256(1) << (nonce & 0xff);
        if (_processed[word] & bit != 0) revert AlreadyProcessed();
        _processed[word] |= bit;
        totalReleased += amount;
        token.safeTransfer(recipient, amount);
        emit Released(nonce, recipient, amount, viaProof);
    }

    // ---------------------------------------------------------------- refunds

    function refundDeposit(uint64 nonce, bytes[] calldata sigs) external {
        if (paused) revert Paused();
        Deposit storage d = _deposits[nonce];
        if (d.amount == 0 || d.refundTo == address(0)) revert NotAnOrder();
        if (d.refunded) revert AlreadyRefunded();
        if (!ICadreOrderSource(d.sender).refundable(nonce, d.refundTo, d.amount)) revert NotRefundable();
        _checkSigs(refundDigest(nonce, d.refundTo, d.amount), sigs);
        d.refunded = true;
        totalRefunded += d.amount;
        token.safeTransfer(d.refundTo, d.amount);
        emit Refunded(nonce, d.refundTo, d.amount);
    }
}
