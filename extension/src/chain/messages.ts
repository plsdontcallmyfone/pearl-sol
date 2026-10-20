// Domain messages and derived ids (docs/RPC.md "Signatures", "Job ids").
// Attester / evaluator messages are RAW ed25519 over these bytes (no framing).

import { concat, u32, u64, utf8 } from "../core/bytes";
import { blake3, sha256 } from "../core/crypto";

export const RESERVED = {
  escrow: blake3(utf8("pearl-sol:escrow:v1")),
  pool: blake3(utf8("pearl-sol:mining-pool:v1")),
};

export const depositMessage = (solanaNonce: bigint, recipient: Uint8Array, amountSpl: bigint) =>
  concat(utf8("pearl-sol:deposit:v1"), u64(solanaNonce), recipient, u64(amountSpl));

export const depositOrderMessage = (solanaNonce: bigint, recipient: Uint8Array, amountSpl: bigint, postJobBody: Uint8Array) =>
  concat(utf8("pearl-sol:deposit-order:v1"), u64(solanaNonce), recipient, u64(amountSpl), sha256(postJobBody));

export const resultMessage = (jobId: Uint8Array, deliverer: Uint8Array, resultHash: Uint8Array) =>
  concat(utf8("pearl-sol:result:v1"), jobId, deliverer, resultHash);

/** RejectResult evaluator message: `"pearl-sol:reject:v1" || job_id || deliverer || result_hash` (raw ed25519). */
export const rejectMessage = (jobId: Uint8Array, deliverer: Uint8Array, resultHash: Uint8Array) =>
  concat(utf8("pearl-sol:reject:v1"), jobId, deliverer, resultHash);

/** ClearRoute release signature (raw ed25519 by the lien's release key):
 *  "pearl-sol:route-clear:v1" || chain_id u32 LE || account || route_nonce u64 LE. */
export const routeClearMessage = (chainId: number, account: Uint8Array, routeNonce: bigint) =>
  concat(utf8("pearl-sol:route-clear:v1"), u32(chainId), account, u64(routeNonce));

/** Post id (SPEC 4.6): blake3("pearl-sol:post:v1" || from || nonce u64 LE). */
export const postId = (from: Uint8Array, nonce: bigint) => blake3(concat(utf8("pearl-sol:post:v1"), from, u64(nonce)));

/** Coinbase receipt (97 bytes, the last tx_root leaf of a block; getBlock `coinbase.record`). */
export interface CoinbaseReceipt {
  height: bigint;
  miner: Uint8Array;
  fees: bigint;
  subsidy: bigint;
  jobId: Uint8Array;
  matureHeight: bigint;
}

export function decodeCoinbase(b: Uint8Array): CoinbaseReceipt {
  if (b.length !== 97 || b[0] !== 0) throw new Error("not a coinbase receipt (97 bytes, version 0)");
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const u = (o: number) => dv.getBigUint64(o, true);
  return { height: u(1), miner: b.slice(9, 41), fees: u(41), subsidy: u(49), jobId: b.slice(57, 89), matureHeight: u(89) };
}

/** PostJob tx: job_id = blake3(poster || tx nonce u64 LE). */
export const jobIdForPost = (poster: Uint8Array, nonce: bigint) => blake3(poster, u64(nonce));

/** Storefront order: job_id = sha256("pearl-sol:order-job:v1" || solana_nonce u64 LE). */
export const orderJobId = (solanaNonce: bigint) => sha256(utf8("pearl-sol:order-job:v1"), u64(solanaNonce));

/** terms_hash = blake3("pearl-sol:terms:v2" || poster || job_id || m || n || k || W || P || deadline_height || max_blocks). */
export function termsHash(t: {
  poster: Uint8Array; jobId: Uint8Array; m: number; n: number; k: number;
  workerPay: bigint; protocolFee: bigint; deadlineHeight: bigint; maxBlocks: number;
}): Uint8Array {
  return blake3(
    utf8("pearl-sol:terms:v2"), t.poster, t.jobId, u32(t.m), u32(t.n), u32(t.k),
    u64(t.workerPay), u64(t.protocolFee), u64(t.deadlineHeight), u32(t.maxBlocks),
  );
}

/**
 * Storefront order header terms (chain/state schema `order_terms_hash`):
 * blake3("pearl-sol:order-terms:v1" || poster || m u32 || n u32 || k u32 || max_blocks u32 || deadline_blocks u64).
 * Every field is known when the order is opened, so the buyer can commit hash_a / hash_b
 * against the order's Pearl header `poster || order_terms || 0 u64 || 0 u32`.
 * `deadline_blocks` is RELATIVE, as in the order's PostJob body.
 */
export function orderTermsHash(t: { poster: Uint8Array; m: number; n: number; k: number; maxBlocks: number; deadlineBlocks: bigint }): Uint8Array {
  if (t.poster.length !== 32) throw new Error("poster is 32 bytes");
  return blake3(utf8("pearl-sol:order-terms:v1"), t.poster, u32(t.m), u32(t.n), u32(t.k), u32(t.maxBlocks), u64(t.deadlineBlocks));
}
