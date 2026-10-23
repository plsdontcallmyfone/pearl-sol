// ChainIO over an in-process LiteSVM instance, for tests (no validator, no ports).

import { ComputeBudgetProgram, Keypair, PublicKey, Transaction, TransactionInstruction, VersionedTransaction } from "@solana/web3.js";
import { FailedTransactionMetadata, LiteSVM } from "./litesvm-web3.ts";
import type { ChainIO, RawAccount } from "../src/chain.ts";

export class LiteSvmChain implements ChainIO {
  readonly svm: LiteSVM;
  sent: { cu: number; logs: string[] }[] = [];
  constructor(svm: LiteSVM) {
    this.svm = svm;
  }
  async getAccounts(keys: PublicKey[]): Promise<(RawAccount | null)[]> {
    return keys.map((k) => {
      const a = this.svm.getAccount(k);
      if (!a || (a.lamports === 0 && a.data.length === 0)) return null;
      return { lamports: this.svm.getBalance(k) ?? 0n, owner: a.owner, data: new Uint8Array(a.data) };
    });
  }
  private result(r: ReturnType<LiteSVM["sendTransaction"]>): string {
    this.svm.expireBlockhash();
    if (r instanceof FailedTransactionMetadata) throw new Error(`tx failed: ${r.err()} ${r.meta().logs().join(" | ")}`);
    this.sent.push({ cu: Number(r.computeUnitsConsumed()), logs: r.logs() });
    return Buffer.from(r.signature()).toString("hex");
  }
  async send(ixs: TransactionInstruction[], feePayer: Keypair, extra: Keypair[] = [], cuLimit = 400_000): Promise<string> {
    const tx = new Transaction({ feePayer: feePayer.publicKey, recentBlockhash: this.svm.latestBlockhash() })
      .add(ComputeBudgetProgram.setComputeUnitLimit({ units: cuLimit }), ...ixs);
    tx.sign(feePayer, ...extra);
    return this.result(this.svm.sendTransaction(tx));
  }
  async sendRaw(raw: Uint8Array): Promise<string> {
    return this.result(this.svm.sendTransaction(VersionedTransaction.deserialize(raw)));
  }
  async latestBlockhash(): Promise<string> {
    return this.svm.latestBlockhash();
  }
}
