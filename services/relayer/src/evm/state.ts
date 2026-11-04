// RH relayer state: its own JSON file (RH_STATE_FILE), never the Solana relayer's.
// Same restart-safety rules as src/state.ts: cursors, in-flight unlocks, the per-nonce
// unlock / refund choice saved BEFORE anything is signed.

import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

export interface EvmInFlight {
  txHash: string;
  txNonce: string;
  at: number;
}

export interface EvmRelease {
  recipient: string;
  amountExt: string;
  height: number;
  via: "signers" | "proof";
  tx: string;
}

export interface EvmHeld {
  reason: "protocol_fee_below_min" | "order_not_pending" | "order_body_mismatch";
  jobId: string;
  buyer: string;
  recipient: string;
  amountExt: string;
  protocolFee: string;
  minProtocolFeeEffective: string;
  since: number;
}

export interface EvmRefund {
  buyer: string;
  amountExt: string;
  reason: EvmHeld["reason"];
  at: number;
  /** refundDeposit tx, once it landed. */
  tx?: string;
  marked?: string;
  shareSettled?: string;
}

export interface EvmState {
  /** Every RH deposit below this nonce is processed AND final on the chain (or refunded). */
  deposit: bigint;
  /** Every domain withdrawal below this nonce is processed on RH (at the finality block). */
  withdrawal: bigint;
  inflight: Record<string, EvmInFlight>;
  /** Order deposit nonces still to finalize / expire on the storefront. */
  orders: string[];
  /** Releases this relayer sent, by withdrawal nonce (the last 1000). */
  released: Record<string, EvmRelease>;
  needsRefund: Record<string, EvmHeld>;
  signedUnlock: Record<string, number>;
  refunds: Record<string, EvmRefund>;
  /** Order results this relayer sent, by deposit nonce: tx hash. */
  settled: Record<string, string>;
}

export const emptyEvmState = (): EvmState => ({
  deposit: 0n, withdrawal: 0n, inflight: {}, orders: [], released: {}, needsRefund: {}, signedUnlock: {}, refunds: {}, settled: {},
});

export function loadEvmState(file: string): EvmState {
  if (!existsSync(file)) return emptyEvmState();
  const j = JSON.parse(readFileSync(file, "utf8"));
  if (j.side !== "rh") throw new Error(`${file} is not an RH relayer state file (side ${JSON.stringify(j.side)}); refusing to share a state file across sides`);
  return {
    deposit: BigInt(j.deposit ?? 0),
    withdrawal: BigInt(j.withdrawal ?? 0),
    inflight: j.inflight ?? {},
    orders: j.orders ?? [],
    released: j.released ?? {},
    needsRefund: j.needsRefund ?? {},
    signedUnlock: j.signedUnlock ?? {},
    refunds: j.refunds ?? {},
    settled: j.settled ?? {},
  };
}

export function saveEvmState(file: string, s: EvmState): void {
  const tmp = `${file}.tmp`;
  const out = { side: "rh", ...s, deposit: s.deposit.toString(), withdrawal: s.withdrawal.toString() };
  writeFileSync(tmp, JSON.stringify(out, null, 2) + "\n");
  renameSync(tmp, file);
}
