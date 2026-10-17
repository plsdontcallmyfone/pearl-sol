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
