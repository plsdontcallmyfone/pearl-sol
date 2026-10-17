// Minimal Solana interface the service needs, with a JSON-RPC implementation.
// Tests plug in an in-process LiteSVM implementation (test/litesvm-chain.ts).

import { ComputeBudgetProgram, Connection, Keypair, PublicKey, Transaction, TransactionInstruction } from "@solana/web3.js";
import type { Commitment } from "@solana/web3.js";

export interface RawAccount {
  lamports: bigint;
  owner: PublicKey;
  data: Uint8Array;
}

export interface ChainIO {
  getAccounts(keys: PublicKey[]): Promise<(RawAccount | null)[]>;
  /** Builds, signs, sends and confirms; returns the signature. Throws with logs on failure. */
  send(ixs: TransactionInstruction[], feePayer: Keypair, extraSigners?: Keypair[], cuLimit?: number): Promise<string>;
  /** Sends and confirms an already signed transaction. */
  sendRaw(tx: Uint8Array): Promise<string>;
  latestBlockhash(): Promise<string>;
}

export class RpcChain implements ChainIO {
  readonly conn: Connection;
  readonly commitment: Commitment;
  constructor(url: string, commitment: Commitment = "confirmed") {
    this.commitment = commitment;
    this.conn = new Connection(url, { commitment });
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
  private async confirm(sig: string, blockhash: string, lastValidBlockHeight: number): Promise<string> {
    const res = await this.conn.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, this.commitment);
    if (res.value.err) throw new Error(`tx ${sig} failed: ${JSON.stringify(res.value.err)}`);
    return sig;
  }
}
