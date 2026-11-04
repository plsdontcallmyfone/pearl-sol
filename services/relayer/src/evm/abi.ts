// The parts of the token-evm/ contracts the RH relayer and the header-oracle poster use
// (token-evm/src/{CadreBridge,CadreStorefront,CadreHeaderOracle}.sol, FORMATS.md).

import { parseAbi } from "viem";

export const bridgeAbi = parseAbi([
  "function token() view returns (address)",
  "function domain() view returns (uint8)",
  "function depositNonce() view returns (uint64)",
  "function releaseModes() view returns (uint8)",
  "function paused() view returns (bool)",
  "function headers() view returns (address)",
  "function threshold() view returns (uint8)",
  "function signers() view returns (address[])",
  "function minWithdrawalNonce() view returns (uint64)",
  "function deposits(uint64 nonce) view returns ((address sender, uint64 amount, address refundTo, uint64 blockNumber, bytes32 recipient, bool refunded))",
  "function isProcessed(uint64 nonce) view returns (bool)",
  "function release(uint64 nonce, address recipient, uint64 amount, bytes[] sigs)",
  "function releaseProof(bytes entry, uint64 location, bytes proof, bytes32 headerHash)",
  "function refundDeposit(uint64 nonce, bytes[] sigs)",
  "event Deposited(uint64 indexed nonce, address indexed sender, bytes32 indexed cadreRecipient, uint64 amount, address refundTo)",
]);

export const erc20Abi = parseAbi([
  "function balanceOf(address) view returns (uint256)",
  "function approve(address spender, uint256 amount) returns (bool)",
  "function transfer(address to, uint256 amount) returns (bool)",
]);

export const storefrontAbi = parseAbi([
  "function threshold() view returns (uint8)",
  "function signers() view returns (address[])",
  "function owner() view returns (address)",
  "function getOrder(uint64 nonce) view returns ((address buyer, uint8 status, bool shareSettled, uint64 tokens, uint64 workerPay, uint64 protocolFee, uint128 ethSpent, uint128 share, bytes32 recipient, bytes32 jobId, bytes32 bodyHash, bytes32 deliverer, bytes32 resultHash))",
  "function finalizeOrder(uint64 nonce, bytes32 deliverer, bytes32 resultHash, string resultUri, bytes[] sigs)",
  "function expireOrder(uint64 nonce, bytes[] sigs)",
  "function markRefunded(uint64 nonce)",
  "function settleShare(uint64 nonce)",
  "event OrderBody(uint64 indexed nonce, bytes body)",
]);

export const oracleAbi = parseAbi([
  "function cadreChainId() view returns (uint64)",
  "function threshold() view returns (uint8)",
  "function signers() view returns (address[])",
  "function latestFinal() view returns (uint64, bytes32)",
  "function hashAtHeight(uint64) view returns (bytes32)",
  "function isFinal(bytes32) view returns (bool)",
  "function roots(bytes32) view returns (uint64 height, bytes32 txRoot, bytes32 stateRoot, bytes32 withdrawRoot)",
  "function postFinalHeader(bytes header, bytes32 blockHash, bytes[] sigs)",
]);

/** Storefront order status (CadreStorefront STATUS_*). 0 = no order at that nonce. */
export const ORDER = { NONE: 0, PENDING: 1, FINALIZED: 2, EXPIRED: 3, REFUNDED: 4 } as const;
/** CadreBridge release modes. */
export const MODE_SIGNERS = 1;
export const MODE_PROOF = 2;
