// Minimal chain interface the relayer needs, with a JSON-RPC implementation.
// Tests plug in an in-process LiteSVM implementation (test/litesvm-chain.ts).

import { Connection, Keypair, PublicKey, Transaction, TransactionInstruction } from "@solana/web3.js";
import type { Commitment } from "@solana/web3.js";

export interface RawAccount {
  lamports: bigint;
  owner: PublicKey;
  data: Uint8Array;
}

export interface ChainIO {
  readonly name: string;
  /** All accounts read at one slot (null where missing). */
  getAccounts(keys: PublicKey[]): Promise<(RawAccount | null)[]>;
  /** Sends and confirms one transaction; returns its signature. Throws on failure. */
  send(ixs: TransactionInstruction[], feePayer: Keypair, extraSigners?: Keypair[]): Promise<string>;
}

const BATCH = 100; // getMultipleAccounts limit

export class RpcChain implements ChainIO {
  readonly name: string;
  readonly conn: Connection;
  readonly commitment: Commitment;
  readonly url: string;

  constructor(name: string, url: string, commitment: Commitment = "confirmed") {
    this.name = name;
    this.commitment = commitment;
    this.url = url;
    this.conn = new Connection(url, { commitment });
  }

  async getAccounts(keys: PublicKey[]): Promise<(RawAccount | null)[]> {
    // Raw JSON-RPC instead of Connection.getMultipleAccountsInfo: the escrow
    // holds about 1e18 lamports, above 2^53, and JSON.parse would round it. The
    // lamports fields are quoted before parsing so they stay exact as bigint.
    // One call answers at one slot; batches above 100 keys span calls, so
    // callers put the accounts that must be consistent in the first batch.
    const out: (RawAccount | null)[] = [];
    for (let i = 0; i < keys.length; i += BATCH) {
      const body = JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "getMultipleAccounts",
        params: [keys.slice(i, i + BATCH).map((k) => k.toBase58()), { encoding: "base64", commitment: this.commitment }],
      });
      const res = await fetch(this.url, { method: "POST", headers: { "content-type": "application/json" }, body });
      if (!res.ok) throw new Error(`${this.name} rpc http ${res.status}`);
      const text = (await res.text()).replace(/"(lamports|rentEpoch)":(\d+)/g, '"$1":"$2"');
      const j = JSON.parse(text);
      if (j.error) throw new Error(`${this.name} rpc: ${JSON.stringify(j.error)}`);
      for (const a of j.result.value as ({ lamports: string; owner: string; data: [string, string] } | null)[]) {
        out.push(a ? { lamports: BigInt(a.lamports), owner: new PublicKey(a.owner), data: Buffer.from(a.data[0], "base64") } : null);
      }
    }
    return out;
  }

  async send(ixs: TransactionInstruction[], feePayer: Keypair, extraSigners: Keypair[] = []): Promise<string> {
    const { blockhash, lastValidBlockHeight } = await this.conn.getLatestBlockhash(this.commitment);
    const tx = new Transaction({ feePayer: feePayer.publicKey, blockhash, lastValidBlockHeight }).add(...ixs);
    tx.sign(feePayer, ...extraSigners);
    const sig = await this.conn.sendRawTransaction(tx.serialize(), { preflightCommitment: this.commitment });
    const res = await this.conn.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, this.commitment);
    if (res.value.err) throw new Error(`${this.name} tx ${sig} failed: ${JSON.stringify(res.value.err)}`);
    return sig;
  }
}
