// ChainIO over an in-process LiteSVM instance, for tests (no validator, no ports).

import { Keypair, PublicKey, Transaction, TransactionInstruction } from "@solana/web3.js";
import { FailedTransactionMetadata, LiteSVM } from "./litesvm-web3.ts";
import type { ChainIO, RawAccount } from "../src/chain.ts";

export class LiteSvmChain implements ChainIO {
  readonly name: string;
  readonly svm: LiteSVM;
  constructor(name: string, svm: LiteSVM) {
    this.name = name;
    this.svm = svm;
  }
  async getAccounts(keys: PublicKey[]): Promise<(RawAccount | null)[]> {
    return keys.map((k) => {
      const a = this.svm.getAccount(k);
      // LiteSVM reports a never-touched or emptied address as a zero-lamport account.
      if (!a || (a.lamports === 0 && a.data.length === 0)) return null;
      // getBalance is exact (bigint); AccountInfo.lamports is a JS number and rounds near 1e18.
      return { lamports: this.svm.getBalance(k) ?? 0n, owner: a.owner, data: new Uint8Array(a.data) };
    });
  }
  async send(ixs: TransactionInstruction[], feePayer: Keypair, extra: Keypair[] = []): Promise<string> {
    const tx = new Transaction({ feePayer: feePayer.publicKey, recentBlockhash: this.svm.latestBlockhash() }).add(...ixs);
    tx.sign(feePayer, ...extra);
    const r = this.svm.sendTransaction(tx);
    this.svm.expireBlockhash();
    if (r instanceof FailedTransactionMetadata) {
      throw new Error(`${this.name} tx failed: ${r.err()} ${r.meta().logs().join(" | ")}`);
    }
    return Buffer.from(r.signature()).toString("hex");
  }
}
