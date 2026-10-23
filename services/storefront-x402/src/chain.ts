// Minimal Solana interface the service needs, with a JSON-RPC implementation.
// Tests plug in an in-process LiteSVM implementation (test/litesvm-chain.ts).

import { ComputeBudgetProgram, Connection, Keypair, PublicKey, Transaction, TransactionInstruction } from "@solana/web3.js";
import type { Commitment } from "@solana/web3.js";
import { createHash } from "node:crypto";

export interface RawAccount {
  lamports: bigint;
  owner: PublicKey;
  data: Uint8Array;
}

/** An open OrderDraft of the service wallet (storefront `["draft", buyer, draft_id]`). */
export interface DraftInfo {
  address: string;
  draftId: bigint;
  recipient: string;
  lamports: bigint;
}

export interface ChainIO {
  getAccounts(keys: PublicKey[]): Promise<(RawAccount | null)[]>;
  /** Builds, signs, sends and confirms; returns the signature. Throws with logs on failure. */
  send(ixs: TransactionInstruction[], feePayer: Keypair, extraSigners?: Keypair[], cuLimit?: number): Promise<string>;
  /** Sends and confirms an already signed transaction. */
  sendRaw(tx: Uint8Array): Promise<string>;
  latestBlockhash(): Promise<string>;
  /** Every OrderDraft `buyer` holds under `program` (getProgramAccounts); optional (LiteSVM cannot list). */
  listDrafts?(buyer: PublicKey, program: PublicKey): Promise<DraftInfo[]>;
}

export const DRAFT_DISC = createHash("sha256").update("account:OrderDraft").digest().subarray(0, 8);

/** Bounds on every RPC call: no request and no confirmation waits forever (an operator tool that
 *  hung after its work kept the storefront stopped for 20 minutes on devnet, 2026-10-23). */
export interface RpcLimits {
  /** One HTTP request to the RPC (default 30 s). */
  requestMs?: number;
  /** Waiting for a sent tx to reach the commitment (default 90 s, about a blockhash's life). */
  confirmMs?: number;
  /** Status poll period (default 1 s). */
  pollMs?: number;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
export function bs58(b: Uint8Array): string {
  let n = BigInt("0x" + (Buffer.from(b).toString("hex") || "0"));
  let s = "";
  while (n > 0n) {
    s = B58[Number(n % 58n)] + s;
    n /= 58n;
  }
  for (const x of b) {
    if (x !== 0) break;
    s = "1" + s;
  }
  return s;
}

export class RpcChain implements ChainIO {
  readonly conn: Connection;
  readonly commitment: Commitment;
  readonly limits: Required<RpcLimits>;
  constructor(url: string, commitment: Commitment = "confirmed", limits: RpcLimits = {}) {
    this.commitment = commitment;
    this.limits = { requestMs: limits.requestMs ?? 30_000, confirmMs: limits.confirmMs ?? 90_000, pollMs: limits.pollMs ?? 1_000 };
    const requestMs = this.limits.requestMs;
    // every HTTP call bounded; no websocket is opened (confirmation polls getSignatureStatuses),
    // so nothing keeps a finished process alive
    this.conn = new Connection(url, {
      commitment,
      fetch: (input: any, init?: any) => fetch(input, { ...init, signal: AbortSignal.timeout(requestMs) }),
    });
  }
  async listDrafts(buyer: PublicKey, program: PublicKey): Promise<DraftInfo[]> {
    const r = await this.conn.getProgramAccounts(program, {
      commitment: this.commitment,
      filters: [{ memcmp: { offset: 0, bytes: bs58(DRAFT_DISC) } }, { memcmp: { offset: 8, bytes: buyer.toBase58() } }],
    });
    return r.map((a) => ({
      address: a.pubkey.toBase58(),
      draftId: Buffer.from(a.account.data).readBigUInt64LE(40),
      recipient: new PublicKey(a.account.data.subarray(48, 80)).toBase58(),
      lamports: BigInt(a.account.lamports),
    }));
  }
  async getAccounts(keys: PublicKey[]): Promise<(RawAccount | null)[]> {
    const out: (RawAccount | null)[] = [];
    for (let i = 0; i < keys.length; i += 100) {
      const got = await this.conn.getMultipleAccountsInfo(keys.slice(i, i + 100), this.commitment);
      for (const a of got) out.push(a ? { lamports: BigInt(a.lamports), owner: a.owner, data: new Uint8Array(a.data) } : null);
    }
    return out;
  }
  async send(ixs: TransactionInstruction[], feePayer: Keypair, extra: Keypair[] = [], cuLimit = 400_000): Promise<string> {
    const { blockhash, lastValidBlockHeight } = await this.conn.getLatestBlockhash(this.commitment);
    const tx = new Transaction({ feePayer: feePayer.publicKey, blockhash, lastValidBlockHeight })
      .add(ComputeBudgetProgram.setComputeUnitLimit({ units: cuLimit }), ...ixs);
    tx.sign(feePayer, ...extra);
    return this.confirm(await this.conn.sendRawTransaction(tx.serialize(), { preflightCommitment: this.commitment }), blockhash, lastValidBlockHeight);
  }
  async sendRaw(raw: Uint8Array): Promise<string> {
    const { blockhash, lastValidBlockHeight } = await this.conn.getLatestBlockhash(this.commitment);
    return this.confirm(await this.conn.sendRawTransaction(raw, { preflightCommitment: this.commitment }), blockhash, lastValidBlockHeight);
  }
  async latestBlockhash(): Promise<string> {
    return (await this.conn.getLatestBlockhash(this.commitment)).blockhash;
  }
  /**
   * Polls the signature's status until it reaches the commitment, fails, its blockhash expires,
   * or confirmMs passes; it always returns or throws in bounded time. (web3.js confirmTransaction
   * waits on a websocket subscription that can outlive the work and has no wall-clock bound.)
   */
  private async confirm(sig: string, _blockhash: string, lastValidBlockHeight: number): Promise<string> {
    const deadline = Date.now() + this.limits.confirmMs;
    const want = this.commitment === "finalized" ? ["finalized"] : this.commitment === "processed" ? ["processed", "confirmed", "finalized"] : ["confirmed", "finalized"];
    let polls = 0;
    let lastErr = "";
    for (;;) {
      try {
        const st = (await this.conn.getSignatureStatuses([sig])).value[0];
        if (st?.err) throw Object.assign(new Error(`tx ${sig} failed: ${JSON.stringify(st.err)}`), { final: true });
        if (st && want.includes(st.confirmationStatus ?? "")) return sig;
        if (!st && ++polls % 5 === 0 && (await this.conn.getBlockHeight(this.commitment)) > lastValidBlockHeight) {
          throw Object.assign(new Error(`tx ${sig} expired: blockhash past block height ${lastValidBlockHeight} and not seen`), { final: true });
        }
      } catch (e) {
        if ((e as any).final) throw e;
        lastErr = (e as Error).message.slice(0, 200); // an RPC hiccup (429, timeout): poll again
      }
      if (Date.now() >= deadline) throw new Error(`tx ${sig} not confirmed (${this.commitment}) within ${this.limits.confirmMs} ms${lastErr ? `; last RPC error: ${lastErr}` : ""}`);
      await sleep(this.limits.pollMs);
    }
  }
}
