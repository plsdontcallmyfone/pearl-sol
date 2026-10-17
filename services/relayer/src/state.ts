// Relayer state persisted to one JSON file (atomic rename). Restart-safe: a new
// process resumes from the cursors, keeps watching its in-flight unlocks and
// tracked orders, and remembers what it released.

import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

export interface InFlight {
  /** Agent-chain tx hash of the BridgeUnlock. */
  txHash: string;
  /** Relayer account nonce the tx used. */
  txNonce: string;
  /** ms since epoch of the (last) submission. */
  at: number;
}

export interface ReleaseRecord {
  recipient: string;
  amountSpl: string;
  height: number;
  blockHash: string;
  sig: string;
}

export interface RelayerState {
  /** Every Solana deposit below this nonce is processed AND final on the chain. */
  deposit: bigint;
  /** Every chain withdrawal below this nonce is released on Solana. */
  withdrawal: bigint;
  /** BridgeUnlock txs submitted and not yet final, by Solana deposit nonce. */
  inflight: Record<string, InFlight>;
  /** Storefront order deposit nonces whose Order is still pending on Solana. */
  orders: string[];
  /** Releases this relayer signed, by withdrawal nonce (the last 1000). */
  released: Record<string, ReleaseRecord>;
}

export const emptyState = (): RelayerState => ({ deposit: 0n, withdrawal: 0n, inflight: {}, orders: [], released: {} });

export function loadState(file: string): RelayerState {
  if (!existsSync(file)) return emptyState();
  const j = JSON.parse(readFileSync(file, "utf8"));
  return {
    deposit: BigInt(j.deposit ?? 0),
    withdrawal: BigInt(j.withdrawal ?? 0),
    inflight: j.inflight ?? {},
    orders: j.orders ?? [],
    released: j.released ?? {},
  };
}

export function saveState(file: string, s: RelayerState): void {
  const tmp = `${file}.tmp`;
  const out = { ...s, deposit: s.deposit.toString(), withdrawal: s.withdrawal.toString() };
  writeFileSync(tmp, JSON.stringify(out, null, 2) + "\n");
  renameSync(tmp, file);
}
