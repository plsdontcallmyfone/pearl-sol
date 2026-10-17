// Solana JSON-RPC (the handful of methods the wallet needs).

import { b58, b64, fromB64 } from "../core/bytes";
import { jsonRpc } from "../chain/rpc";

export interface SolAccount {
  lamports: number;
  owner: string;
  data: Uint8Array;
  executable: boolean;
}

const toAcc = (v: any): SolAccount | null =>
  v ? { lamports: v.lamports, owner: v.owner, data: fromB64(v.data[0]), executable: v.executable } : null;

export class SolanaRpc {
  constructor(public url: string, public commitment: "processed" | "confirmed" | "finalized" = "confirmed") {}
  call<T>(method: string, params: unknown[] = []) {
    return jsonRpc<T>(this.url, method, params);
  }
  async balance(addr: string): Promise<bigint> {
    const r = await this.call<{ value: number }>("getBalance", [addr, { commitment: this.commitment }]);
    return BigInt(r.value);
  }
  async account(addr: Uint8Array | string): Promise<SolAccount | null> {
    const a = typeof addr === "string" ? addr : b58(addr);
    const r = await this.call<{ value: any }>("getAccountInfo", [a, { encoding: "base64", commitment: this.commitment }]);
    return toAcc(r.value);
  }
  async accounts(addrs: (Uint8Array | string)[]): Promise<(SolAccount | null)[]> {
    if (addrs.length === 0) return [];
    const r = await this.call<{ value: any[] }>("getMultipleAccounts", [addrs.map((a) => (typeof a === "string" ? a : b58(a))), { encoding: "base64", commitment: this.commitment }]);
    return r.value.map(toAcc);
  }
  async blockhash(): Promise<string> {
    const r = await this.call<{ value: { blockhash: string } }>("getLatestBlockhash", [{ commitment: this.commitment }]);
    return r.value.blockhash;
  }
  async send(tx: Uint8Array): Promise<string> {
    return this.call<string>("sendTransaction", [b64(tx), { encoding: "base64", preflightCommitment: this.commitment }]);
  }
  async statuses(sigs: string[]) {
    const r = await this.call<{ value: ({ confirmationStatus: string; err: unknown; slot: number } | null)[] }>("getSignatureStatuses", [sigs, { searchTransactionHistory: true }]);
    return r.value;
  }
  async confirm(sig: string, timeoutMs = 30_000): Promise<void> {
    const end = Date.now() + timeoutMs;
    while (Date.now() < end) {
      const [s] = await this.statuses([sig]);
      if (s?.err) throw new Error(`transaction failed: ${JSON.stringify(s.err)}`);
      if (s && (s.confirmationStatus === "confirmed" || s.confirmationStatus === "finalized")) return;
      await new Promise((r) => setTimeout(r, 600));
    }
    throw new Error("not confirmed in time; check activity later");
  }
  async signatures(addr: string, limit = 20) {
    return this.call<{ signature: string; slot: number; err: unknown; blockTime: number | null; memo: string | null }[]>("getSignaturesForAddress", [addr, { limit, commitment: this.commitment }]);
  }
  async health(): Promise<boolean> {
    try {
      await this.call<string>("getHealth");
      return true;
    } catch {
      return false;
    }
  }
}
