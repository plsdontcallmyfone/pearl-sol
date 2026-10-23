// LiteSVM with the @solana/web3.js API these tests were written against (litesvm 0.3.3).
// litesvm 1.x speaks @solana/kit (string addresses, kit transactions and EncodedAccount). This
// keeps the 0.3.3 wrapper's exact behaviour (PublicKey in, AccountInfo out, web3.js transactions
// sent as their wire bytes) on top of 1.x's native binding, which has the same byte-level methods.
// Why 1.x: 0.3.3's linux-x64 binding corrupted memory (LiteSVM/litesvm#171, the x87 state the
// SBPF JIT left behind; cleared after every transaction since 1.4.0, LiteSVM/litesvm#398).
// Copies of this file: services/{relayer,storefront-x402}/test/litesvm-web3.ts; JS copies in
// token/tests/{mandate,reserve,storefront,town}/litesvm-web3.cjs and research/pump-litesvm.

import { PublicKey, Transaction, VersionedTransaction } from "@solana/web3.js";
import * as base from "litesvm";
import type { Account as NativeAccount, Clock, FailedTransactionMetadata, TransactionMetadata } from "litesvm";

export { Clock, FailedTransactionMetadata, TransactionMetadata } from "litesvm";

export interface AccountInfoBytes {
  executable: boolean;
  owner: PublicKey;
  lamports: number;
  data: Uint8Array;
  rentEpoch?: number;
}

// The byte-level native methods of litesvm 1.x (LiteSVM.inner), the same ones 0.3.3 wrapped.
interface Native {
  minimumBalanceForRentExemption(dataLen: bigint): bigint;
  getAccount(address: Uint8Array): NativeAccount | null;
  setAccount(address: Uint8Array, account: NativeAccount): void;
  getBalance(address: Uint8Array): bigint | null;
  latestBlockhash(): string;
  airdrop(address: Uint8Array, lamports: bigint): TransactionMetadata | FailedTransactionMetadata | null;
  addProgramFromFile(programId: Uint8Array, path: string): void;
  addProgram(programId: Uint8Array, bytes: Uint8Array): void;
  getSigverify(): boolean;
  sendLegacyTransaction(raw: Uint8Array): TransactionMetadata | FailedTransactionMetadata;
  sendVersionedTransaction(raw: Uint8Array): TransactionMetadata | FailedTransactionMetadata;
  expireBlockhash(): void;
  warpToSlot(slot: bigint): void;
  getClock(): Clock;
  setClock(clock: Clock): void;
}

// rentEpoch comes out as a JS number, so u64::MAX (what Agave 4 gives rent-exempt accounts)
// reads back as 2^64; clamp it so an account read with getAccount can be written back.
const U64_MAX = (1n << 64n) - 1n;
function rentEpochU64(e: number | undefined): bigint {
  const v = BigInt(e || 0);
  return v > U64_MAX ? U64_MAX : v;
}

function toAccountInfo(acc: NativeAccount): AccountInfoBytes {
  return {
    executable: acc.executable(),
    owner: new PublicKey(acc.owner()),
    lamports: Number(acc.lamports()),
    data: acc.data(),
    rentEpoch: Number(acc.rentEpoch()),
  };
}

export class LiteSVM {
  readonly inner: Native;
  constructor() {
    const inner = (new base.LiteSVM() as unknown as { inner?: Native }).inner;
    if (!inner || typeof inner.sendLegacyTransaction !== "function") {
      throw new Error("litesvm-web3: unexpected litesvm version (no native binding on LiteSVM.inner)");
    }
    this.inner = inner;
  }
  minimumBalanceForRentExemption(dataLen: bigint): bigint {
    return this.inner.minimumBalanceForRentExemption(dataLen);
  }
  getAccount(address: PublicKey): AccountInfoBytes | null {
    const a = this.inner.getAccount(address.toBytes());
    return a === null ? null : toAccountInfo(a);
  }
  setAccount(address: PublicKey, acc: AccountInfoBytes): void {
    this.inner.setAccount(
      address.toBytes(),
      new base.Account(BigInt(acc.lamports), acc.data, acc.owner.toBytes(), acc.executable, rentEpochU64(acc.rentEpoch)),
    );
  }
  getBalance(address: PublicKey): bigint | null {
    return this.inner.getBalance(address.toBytes());
  }
  latestBlockhash(): string {
    return this.inner.latestBlockhash();
  }
  airdrop(address: PublicKey, lamports: bigint): TransactionMetadata | FailedTransactionMetadata | null {
    return this.inner.airdrop(address.toBytes(), lamports);
  }
  addProgramFromFile(programId: PublicKey, path: string): void {
    this.inner.addProgramFromFile(programId.toBytes(), path);
  }
  addProgram(programId: PublicKey, bytes: Uint8Array): void {
    this.inner.addProgram(programId.toBytes(), bytes);
  }
  sendTransaction(tx: Transaction | VersionedTransaction): TransactionMetadata | FailedTransactionMetadata {
    if (tx instanceof Transaction) {
      const raw = tx.serialize({ requireAllSignatures: true, verifySignatures: this.inner.getSigverify() });
      return this.inner.sendLegacyTransaction(raw);
    }
    return this.inner.sendVersionedTransaction(tx.serialize());
  }
  expireBlockhash(): void {
    this.inner.expireBlockhash();
  }
  warpToSlot(slot: bigint): void {
    this.inner.warpToSlot(slot);
  }
  getClock(): Clock {
    return this.inner.getClock();
  }
  setClock(clock: Clock): void {
    this.inner.setClock(clock);
  }
}
