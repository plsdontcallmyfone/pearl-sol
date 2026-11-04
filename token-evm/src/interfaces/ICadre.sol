// SPDX-License-Identifier: ISC
pragma solidity 0.8.26;

/// Final Cadre chain headers as the RH side sees them. Phase 1: CadreHeaderOracle
/// (attester-signed). Phase 2: a Stylus light client with the same interface.
interface ICadreHeaders {
    function isFinal(bytes32 blockHash) external view returns (bool);
    function roots(bytes32 blockHash)
        external
        view
        returns (uint64 height, bytes32 txRoot, bytes32 stateRoot, bytes32 withdrawRoot);
    function latestFinal() external view returns (uint64 height, bytes32 blockHash);
}

/// What CadreBridge asks the order source (CadreStorefront) before a refund.
interface ICadreOrderSource {
    /// True iff deposit `nonce` is an order that may be refunded now (Pending or Expired)
    /// to `buyer` for `amount` tokens.
    function refundable(uint64 nonce, address buyer, uint64 amount) external view returns (bool);
}

interface ICadreBridge {
    function deposit(uint64 amount, bytes32 cadreRecipient) external returns (uint64 nonce);
    function depositFor(uint64 amount, bytes32 cadreRecipient, address refundTo) external returns (uint64 nonce);
    function depositNonce() external view returns (uint64);
    function domain() external view returns (uint8);
    function isRefunded(uint64 nonce) external view returns (bool, address buyer, uint64 amount);
}

/// CadreV4Desk as seen by the storefront and the reserve. zeroForOne = ETH in, CADRE out.
interface ICadreDesk {
    function buy(uint256 minOut, address to) external payable returns (uint256 out);
    function sell(uint256 amountIn, uint256 minOut, address to) external returns (uint256 out);
    function quoteExactIn(bool zeroForOne, uint256 amountIn) external view returns (uint256 out);
}
